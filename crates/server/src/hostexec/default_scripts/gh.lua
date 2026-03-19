-- crates/server/src/hostexec/default_scripts/gh.lua
-- Default gh argument transform: allow read-only operations and collaborative
-- actions (commenting, PR editing), block destructive operations.
-- Destructive operations (PR create, merge, close, delete) are workflow-only
-- via remote_repo through builderd or dedicated handlers.

-- Allowed subcommand pairs: top-level command -> set of allowed subcommands
local allowed_subcommands = {
    ["pr"]  = {
        ["view"] = true, ["checks"] = true, ["list"] = true,
        ["status"] = true, ["diff"] = true,
        ["comment"] = true, ["edit"] = true,
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
        error("blocked: gh requires a subcommand")
    end

    local top = positionals[1]
    local allowed = allowed_subcommands[top]

    if allowed == nil then
        error("blocked: gh " .. top .. " is not allowed")
    end

    -- Special handling for "gh api": validate method + endpoint
    if top == "api" then
        local method = extract_method(args)

        if method == "DELETE" then
            error("blocked: gh api with DELETE method is not allowed")
        end

        if method == "POST" or method == "PATCH" or method == "PUT" then
            local endpoint = extract_api_endpoint(args)
            if endpoint == nil then
                error("blocked: gh api write request requires an endpoint")
            end
            if not is_comment_endpoint(endpoint) then
                error("blocked: gh api " .. method .. " to " .. endpoint .. " is not allowed (only comment/review endpoints permitted)")
            end
        end
        -- GET (default) is allowed, fall through
    elseif type(allowed) == "table" then
        -- Check that the subcommand is in the allowed set
        if #positionals < 2 then
            error("blocked: gh " .. top .. " requires a subcommand")
        end
        local sub = positionals[2]
        if not allowed[sub] then
            error("blocked: gh " .. top .. " " .. sub .. " is not allowed (read-only access only)")
        end
    end

    -- Handle -C flag: rewrite if worker_context allows, block otherwise
    local i = 1
    while i <= #args do
        local arg = args[i]

        if arg == "-C" then
            if worker_context == nil then
                error("blocked flag: -C")
            end
            if i + 1 > #args then
                error("blocked flag: -C (missing path argument)")
            end
            local path_arg = args[i + 1]
            -- Extract final path component (strip trailing slashes, take last segment)
            local stripped = path_arg:gsub("/+$", "")
            local final_component = stripped:match("([^/]+)$") or stripped
            if final_component == worker_context.project_key or final_component == "workspace" then
                args[i + 1] = worker_context.slot_path
                i = i + 2
            else
                error("blocked flag: -C (path '" .. path_arg .. "' does not match project key or 'workspace')")
            end
        else
            i = i + 1
        end
    end

    return { command = command, args = args, working_dir = working_dir }
end
