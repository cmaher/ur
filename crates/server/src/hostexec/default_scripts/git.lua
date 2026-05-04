-- crates/server/src/hostexec/default_scripts/git.lua
-- Default git argument transform: blocks sandbox-escape flags, rewrites -C for workers

function transform(command, args, working_dir, worker_context)
    local blocked_exact = {
        ["--git-dir"] = true,
        ["--work-tree"] = true,
        ["--no-verify"] = true,
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

    -- always_blocked: subcommands blocked for all workers unconditionally
    local always_blocked = {
        ["worktree"] = true,
    }
    -- branch_locked: subcommands blocked in pool mode (branch is set) or when context is absent (safety default)
    local branch_locked = {
        ["checkout"] = true,
        ["switch"] = true,
    }
    -- Find the git subcommand: first positional arg (skip global flags and their values)
    local global_flags_with_value = {
        ["-C"] = true,
        ["-c"] = true,
    }
    local sub_i = 1
    while sub_i <= #args do
        local a = args[sub_i]
        if global_flags_with_value[a] then
            sub_i = sub_i + 2  -- skip flag + its value
        elseif a:sub(1, 1) == "-" then
            sub_i = sub_i + 1  -- skip other flags
        else
            -- First positional arg is the subcommand
            if always_blocked[a] then
                error("blocked git subcommand: " .. a)
            end
            if branch_locked[a] then
                -- Block when no context (safety default) or when in pool mode (branch is non-empty)
                if worker_context == nil or worker_context.branch ~= "" then
                    error("blocked git subcommand: " .. a .. " (use 'git restore' for file operations)")
                end
            end
            break
        end
    end

    -- Restrict push refspecs to the worker's own branch
    if sub_i <= #args and args[sub_i] == "push" then
        if worker_context ~= nil and worker_context.branch ~= "" then
            local allowed_branch = worker_context.branch
            -- Find refspec: positional args after "push" are remote and refspec
            -- Skip flags (args starting with -)
            local push_positionals = {}
            local pi = sub_i + 1
            while pi <= #args do
                if args[pi]:sub(1, 1) ~= "-" then
                    push_positionals[#push_positionals + 1] = args[pi]
                end
                pi = pi + 1
            end
            -- push_positionals[1] = remote (if any), push_positionals[2] = refspec (if any)
            local refspec = push_positionals[2]
            if refspec ~= nil and refspec ~= "HEAD" then
                -- Split on colon to check destination
                local colon_pos = refspec:find(":")
                if colon_pos then
                    local dst = refspec:sub(colon_pos + 1)
                    if dst ~= allowed_branch then
                        error("blocked push: destination branch '" .. dst .. "' does not match worker branch '" .. allowed_branch .. "'")
                    end
                else
                    -- No colon: the whole refspec is the branch name
                    if refspec ~= allowed_branch then
                        error("blocked push: branch '" .. refspec .. "' does not match worker branch '" .. allowed_branch .. "'")
                    end
                end
            end
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
