-- crates/server/src/hostexec/default_scripts/gh.lua
-- Default gh argument transform: allow read-only operations, collaborative
-- actions (commenting, PR editing), and PR creation. Block destructive
-- operations. Destructive operations (PR merge, close, delete) are
-- workflow-only via remote_repo through builderd or dedicated handlers.

-- Help block appended to every rejection so the caller sees exactly what is
-- allowed and how to pass comment/PR body text (the #1 source of failed
-- retries). gh runs on the HOST via host-exec, so it cannot read files from
-- the worker filesystem — but stdin IS forwarded (gh is a bidi command).
local HELP = [[

--- gh via ur host-exec ---
gh runs on the HOST, not inside your container. Consequence:
  * The host gh CANNOT read files from your worker filesystem, so
    `--body-file <path>` / `--input <path>` fail with "no such file or
    directory". Use the "-" (stdin) form instead — stdin IS forwarded.
Pass body text INLINE, or via stdin:
    gh pr comment <pr> --body "$(cat body.md)"
    gh pr create --title "..." --body "$(cat body.md)"
    gh pr edit <pr> --body "$(cat body.md)"
    gh pr comment <pr> --body-file - < body.md

Inline PR review comments (nested comments[] needs --input, not -f/-F):
    gh api /repos/<o>/<r>/pulls/<n>/reviews -X POST --input - <<'JSON'
    {"commit_id":"<sha>","event":"COMMENT","body":"...",
     "comments":[{"path":"a.go","line":42,"side":"RIGHT","body":"..."}]}
    JSON
A single inline comment can also use flat fields on
/repos/<o>/<r>/pulls/<n>/comments (-f path=... -F line=... -f side=RIGHT).
Endpoints may be written with or without the leading slash.

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

-- Normalize an endpoint to the "/repos/..." form the patterns above expect.
-- `gh api` accepts a bare path ("repos/o/r/..."), a rooted path
-- ("/repos/o/r/..."), and a full URL ("https://api.github.com/repos/o/r/...").
-- Without this, a caller omitting the leading slash is rejected against an
-- allowlist that actually permits the endpoint.
local function normalize_endpoint(endpoint)
    -- Strip scheme + host from full URLs.
    endpoint = endpoint:gsub("^%a[%w+.-]*://[^/]*", "")
    -- Drop query string and fragment so anchored patterns still match.
    endpoint = endpoint:gsub("[?#].*$", "")
    -- Collapse a leading run of slashes to exactly one.
    endpoint = endpoint:gsub("^/+", "")
    return "/" .. endpoint
end

-- Check if an API endpoint matches an allowed comment/review pattern
local function is_comment_endpoint(endpoint)
    local normalized = normalize_endpoint(endpoint)
    for _, pattern in ipairs(comment_endpoint_patterns) do
        if normalized:match(pattern) then
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

-- Flags accepted by `gh api` that consume a following value. Any of these must
-- have its value skipped when hunting for the endpoint, otherwise a call like
-- `gh api -f a=b /repos/...` mistakes "a=b" for the endpoint and the real
-- endpoint is never validated. (The `--flag=value` spelling is a single arg
-- starting with "-", so it is skipped naturally.)
local api_value_flags = {
    ["-X"] = true, ["--method"] = true,
    ["-R"] = true, ["--repo"] = true,
    ["-C"] = true,
    ["-f"] = true, ["--field"] = true,
    ["-F"] = true, ["--raw-field"] = true,
    ["-H"] = true, ["--header"] = true,
    ["-q"] = true, ["--jq"] = true,
    ["-t"] = true, ["--template"] = true,
    ["-p"] = true, ["--preview"] = true,
    ["--input"] = true,
    ["--hostname"] = true,
    ["--cache"] = true,
}

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
            elseif found_api and api_value_flags[a] then
                -- skip flag and its value
                skip_next = true
            elseif found_api and a:sub(1, 1) ~= "-" then
                return a
            end
        end
    end
    return nil
end

-- Extract the value of --input, if present. Returns nil when absent.
local function extract_input_value(args)
    for i = 1, #args do
        local a = args[i]
        if a == "--input" then
            if i + 1 <= #args then
                return args[i + 1]
            end
            return nil
        end
        if a:sub(1, 8) == "--input=" then
            return a:sub(9)
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

    -- Reject --body-file / -F <path> for pr subcommands: gh runs on the host
    -- and cannot read worker files, so a worker path fails with a confusing
    -- "no such file or directory". The stdin form ("-") is fine because gh is
    -- a bidi host-exec command and stdin is forwarded.
    if top == "pr" then
        for i = 1, #args do
            local a = args[i]
            local value = nil
            if a == "--body-file" or a == "-F" then
                value = args[i + 1]
            elseif a:sub(1, 12) == "--body-file=" then
                value = a:sub(13)
            end
            if value ~= nil and value ~= "-" then
                fail("blocked flag: " .. a .. " " .. value .. " (gh runs on the host and cannot read worker files; use --body \"$(cat file)\" or pipe it: " .. a .. " - < file)")
            end
        end
    end

    -- Special handling for "gh api": validate method + endpoint
    if top == "api" then
        local method = extract_method(args)

        if method == "DELETE" then
            fail("blocked: gh api with DELETE method is not allowed")
        end

        -- --input <path> cannot work: gh runs on the host and cannot read the
        -- worker filesystem. --input - reads forwarded stdin and is the
        -- supported way to send a nested JSON body (e.g. a review's
        -- comments[] array, which flat -f/-F fields cannot express).
        local input_value = extract_input_value(args)
        if input_value ~= nil and input_value ~= "-" then
            fail("blocked flag: --input " .. input_value .. " (gh runs on the host and cannot read worker files; pipe it instead: --input - < " .. input_value .. ")")
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
