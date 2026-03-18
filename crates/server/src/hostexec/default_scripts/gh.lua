-- crates/server/src/hostexec/default_scripts/gh.lua
-- Default gh argument transform: whitelist read-only operations, block writes.
-- Write operations (PR create, merge) are workflow-only via remote_repo through builderd.

-- Allowed subcommand pairs: top-level command -> set of allowed subcommands
local allowed_subcommands = {
    ["pr"]  = { ["view"] = true, ["checks"] = true, ["list"] = true, ["status"] = true, ["diff"] = true },
    ["run"] = { ["view"] = true, ["list"] = true },
    ["api"] = true,  -- special: allowed but only GET requests
}

-- HTTP methods that imply a write operation for "gh api"
local write_methods = {
    ["POST"]   = true,
    ["PUT"]    = true,
    ["PATCH"]  = true,
    ["DELETE"] = true,
}

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
        error("blocked: gh " .. top .. " is not allowed (read-only access only)")
    end

    -- Special handling for "gh api": block write HTTP methods
    if top == "api" then
        for i = 1, #args do
            local a = args[i]
            if a == "-X" or a == "--method" then
                if i + 1 <= #args then
                    local method = args[i + 1]:upper()
                    if write_methods[method] then
                        error("blocked: gh api with -X " .. method .. " is not allowed (read-only access only)")
                    end
                end
            end
            -- Also check --method=VALUE form
            if a:sub(1, 9) == "--method=" then
                local method = a:sub(10):upper()
                if write_methods[method] then
                    error("blocked: gh api with method " .. method .. " is not allowed (read-only access only)")
                end
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
