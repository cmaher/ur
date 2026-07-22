-- crates/server/src/hostexec/default_scripts/gh.lua
-- Default gh argument transform: allow read-only operations, collaborative
-- actions (commenting, PR editing), and PR creation. Block destructive
-- operations. Destructive operations (PR merge, close, delete) are
-- workflow-only via remote_repo through builderd or dedicated handlers.

-- Help block appended to every rejection so the caller sees exactly what is
-- allowed and how to pass comment/PR body text (the #1 source of failed
-- retries). gh runs on the HOST via host-exec, so it cannot read files from
-- the worker filesystem and stdin is not forwarded.
local HELP = [[

--- gh via ur host-exec ---
gh runs on the HOST, not inside your container. Two consequences:
  * The host gh CANNOT read files from your worker filesystem, so
    `--body-file <path>` fails with "no such file or directory".
  * stdin is NOT forwarded, so `--body-file -` fails with "Body cannot be blank".
Pass body text INLINE — your shell expands it locally and forwards the text:
    gh pr comment <pr> --body "$(cat body.md)"
    gh pr create --title "..." --body "$(cat body.md)"
    gh pr edit <pr> --body "$(cat body.md)"

Allowed (read + collaborative; destructive ops are workflow-only):
  gh pr view|checks|list|status|diff    read-only PR inspection
  gh pr comment                         post a PR/issue comment
  gh pr edit                            edit your own PR
  gh pr create                          open a PR
  gh pr review --comment                review comment only
                                        (--approve/--request-changes blocked)
  gh run view|list                      CI run status and logs
  gh api <endpoint>                     GET always; POST/PATCH only to
                                        comment/review endpoints
Blocked: gh pr merge|close|delete and any other subcommand.]]

-- Raise a rejection with the standard help block appended. Level 0 keeps the
-- message clean (no "input:N:" position prefix).
local function fail(msg)
    error(msg .. "\n" .. HELP, 0)
end

-- Allowed subcommand pairs: top-level command -> set of allowed subcommands
local allowed_subcommands = {
    ["pr"]  = {
        ["view"] = true, ["checks"] = true, ["list"] = true,
        ["status"] = true, ["diff"] = true,
        ["comment"] = true, ["edit"] = true, ["create"] = true,
        ["review"] = "flag_gated",
    },
    ["run"] = { ["view"] = true, ["list"] = true },
    ["api"] = true,  -- special: method + endpoint validation below
}

-- Comment/review API endpoint patterns that allow POST/PATCH
-- These match GitHub REST API paths for issue comments, PR comments,
-- and PR review comments.
local comment_endpoint_patterns = {
    "^/repos/[^/]+/[^/]+/issues/%d+/comments",
    "^/repos/[^/]+/[^/]+/pulls/%d+/comments",
    "^/repos/[^/]+/[^/]+/pulls/%d+/reviews/%d+/comments",
    "^/repos/[^/]+/[^/]+/pulls/%d+/reviews$",
    "^/repos/[^/]+/[^/]+/issues/comments/%d+$",
    "^/repos/[^/]+/[^/]+/pulls/comments/%d+$",
    "^/repos/[^/]+/[^/]+/pulls/%d+/comments/%d+/replies$",
}

-- Check if an API endpoint matches an allowed comment/review pattern
local function is_comment_endpoint(endpoint)
    for _, pattern in ipairs(comment_endpoint_patterns) do
        if endpoint:match(pattern) then
            return true
        end
    end
    return false
end

-- Extract the HTTP method from args (default is GET)
local function extract_method(args)
    for i = 1, #args do
        local a = args[i]
        if a == "-X" or a == "--method" then
            if i + 1 <= #args then
                return args[i + 1]:upper()
            end
        end
        if a:sub(1, 9) == "--method=" then
            return a:sub(10):upper()
        end
    end
    return "GET"
end

-- Extract the API endpoint (first positional arg after "api")
local function extract_api_endpoint(args)
    local found_api = false
    local skip_next = false
    for i = 1, #args do
        if skip_next then
            skip_next = false
        else
            local a = args[i]
            if a == "api" then
                found_api = true
            elseif found_api and (a == "-X" or a == "--method" or a == "-R" or a == "--repo" or a == "-C") then
                -- skip flag and its value
                skip_next = true
            elseif found_api and a:sub(1, 1) ~= "-" then
                return a
            end
        end
    end
    return nil
end

function transform(command, args, working_dir, worker_context)
    -- Extract positional arguments, skipping global flags
    local positionals = {}
    local j = 1
    while j <= #args and #positionals < 2 do
        local a = args[j]
        if a == "--" then
            break
        elseif a:sub(1, 1) == "-" then
            -- Global flags that consume the next argument
            if a == "-R" or a == "--repo" or a == "-C" then
                j = j + 2
            else
                j = j + 1
            end
        else
            positionals[#positionals + 1] = a
            j = j + 1
        end
    end

    if #positionals == 0 then
        fail("blocked: gh requires a subcommand")
    end

    local top = positionals[1]
    local allowed = allowed_subcommands[top]

    if allowed == nil then
        fail("blocked: gh " .. top .. " is not allowed")
    end

    -- Reject --body-file / -F for pr subcommands: gh runs on the host and
    -- cannot read worker files, and stdin is not forwarded, so both the path
    -- and stdin ("-") forms fail with confusing errors. Steer to inline --body.
    if top == "pr" then
        for _, a in ipairs(args) do
            if a == "--body-file" or a == "-F" or a:sub(1, 12) == "--body-file=" then
                fail("blocked flag: " .. a .. " (gh runs on the host and cannot read worker files; use --body \"$(cat file)\" instead)")
            end
        end
    end

    -- Special handling for "gh api": validate method + endpoint
    if top == "api" then
        local method = extract_method(args)

        if method == "DELETE" then
            fail("blocked: gh api with DELETE method is not allowed")
        end

        if method == "POST" or method == "PATCH" or method == "PUT" then
            local endpoint = extract_api_endpoint(args)
            if endpoint == nil then
                fail("blocked: gh api write request requires an endpoint")
            end
            if not is_comment_endpoint(endpoint) then
                fail("blocked: gh api " .. method .. " to " .. endpoint .. " is not allowed (only comment/review endpoints permitted)")
            end
        end
        -- GET (default) is allowed, fall through
    elseif type(allowed) == "table" then
        -- Check that the subcommand is in the allowed set
        if #positionals < 2 then
            fail("blocked: gh " .. top .. " requires a subcommand")
        end
        local sub = positionals[2]
        if not allowed[sub] then
            fail("blocked: gh " .. top .. " " .. sub .. " is not allowed (read-only access only)")
        end

        -- Handle flag-gated subcommands
        if allowed[sub] == "flag_gated" then
            -- Scan args for blocked flags
            for _, a in ipairs(args) do
                if a == "--approve" or a == "-a" or a == "--request-changes" or a == "-r" then
                    fail("blocked: gh pr review --approve/--request-changes is not allowed")
                end
            end
            -- Require --comment or -c
            local has_comment = false
            for _, a in ipairs(args) do
                if a == "--comment" or a == "-c" then
                    has_comment = true
                    break
                end
            end
            if not has_comment then
                fail("blocked: gh pr review requires --comment flag")
            end
        end
    end

    -- Handle -C flag: rewrite if worker_context allows, block otherwise
    local i = 1
    while i <= #args do
        local arg = args[i]

        if arg == "-C" then
            if worker_context == nil then
                fail("blocked flag: -C")
            end
            if i + 1 > #args then
                fail("blocked flag: -C (missing path argument)")
            end
            local path_arg = args[i + 1]
            -- Extract final path component (strip trailing slashes, take last segment)
            local stripped = path_arg:gsub("/+$", "")
            local final_component = stripped:match("([^/]+)$") or stripped
            if final_component == worker_context.project_key or final_component == "workspace" then
                args[i + 1] = worker_context.slot_path
                i = i + 2
            else
                fail("blocked flag: -C (path '" .. path_arg .. "' does not match project key or 'workspace')")
            end
        else
            i = i + 1
        end
    end

    return { command = command, args = args, working_dir = working_dir }
end
