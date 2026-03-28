-- crates/server/src/hostexec/default_scripts/git.lua
-- Default git argument transform: blocks sandbox-escape flags, rewrites -C for workers

function transform(command, args, working_dir, worker_context)
    local blocked_exact = {
        ["--git-dir"] = true,
        ["--work-tree"] = true,
    }
    local blocked_prefix = {
        "--git-dir=",
        "--work-tree=",
    }
    local blocked_config_keys = {
        "core.worktree",
    }

    local i = 1
    while i <= #args do
        local arg = args[i]

        -- Handle -C: rewrite if worker_context allows, block otherwise
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
        elseif blocked_exact[arg] then
            error("blocked flag: " .. arg)
        else
            for _, prefix in ipairs(blocked_prefix) do
                if arg:sub(1, #prefix) == prefix then
                    error("blocked flag: " .. arg)
                end
            end

            -- Check -c key=value for blocked config keys
            if arg == "-c" and i + 1 <= #args then
                local config_val = args[i + 1]:lower()
                for _, key in ipairs(blocked_config_keys) do
                    if config_val:sub(1, #key) == key:lower() then
                        error("blocked config key: " .. key)
                    end
                end
            end
            if arg:sub(1, 2) == "-c" and #arg > 2 then
                local config_val = arg:sub(3):lower()
                for _, key in ipairs(blocked_config_keys) do
                    if config_val:sub(1, #key) == key:lower() then
                        error("blocked config key: " .. key)
                    end
                end
            end

            i = i + 1
        end
    end

    -- Prepend ticket ID to commit messages when worker_context has a process_id
    if worker_context ~= nil and worker_context.process_id ~= "" then
        local ticket_id = worker_context.process_id
        local prefix = "[" .. ticket_id .. "] "
        local i2 = 1
        while i2 <= #args do
            if args[i2] == "commit" then
                -- Found a commit subcommand — look for -m flag
                local j = i2 + 1
                while j <= #args do
                    if args[j] == "-m" and j + 1 <= #args then
                        local msg = args[j + 1]
                        if msg:sub(1, #prefix) ~= prefix then
                            args[j + 1] = prefix .. msg
                        end
                        break
                    end
                    j = j + 1
                end
                break
            end
            i2 = i2 + 1
        end
    end

    return { command = command, args = args, working_dir = working_dir, env = { GIT_EDITOR = "true" } }
end
