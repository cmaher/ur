-- crates/server/src/hostexec/default_scripts/make.lua
-- Default make argument transform: blocks path-escape flags, rewrites -C for workers

function transform(command, args, working_dir, worker_context)
    -- Helper: returns true if the path is absolute or contains ".."
    local function is_unsafe_path(path)
        if path:sub(1, 1) == "/" then
            return true
        end
        -- Check for ".." as a path component
        if path == ".." then
            return true
        end
        if path:find("^%.%./") then
            return true
        end
        if path:find("/%.%./") then
            return true
        end
        if path:find("/%.%.$") then
            return true
        end
        return false
    end

    -- Flags whose argument is a file/directory path that must be safe
    -- Short forms handled as separate arg; long= forms checked by prefix
    local path_flag_short = {
        ["-f"] = true,
        ["-I"] = true,
        ["-o"] = true,
        ["-W"] = true,
    }
    local path_flag_long_equals = {
        "--file=",
        "--makefile=",
        "--include-dir=",
        "--old-file=",
        "--what-if=",
        "--new-file=",
        "--assume-new=",
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
            local stripped = path_arg:gsub("/+$", "")
            local final_component = stripped:match("([^/]+)$") or stripped
            if final_component == worker_context.project_key or final_component == "workspace" then
                args[i + 1] = worker_context.slot_path
                i = i + 2
            else
                error("blocked flag: -C (path '" .. path_arg .. "' does not match project key or 'workspace')")
            end

        -- Handle short path flags (-f path, -I dir, -o file, -W file)
        elseif path_flag_short[arg] then
            if i + 1 > #args then
                -- No argument: let make handle the error
                i = i + 1
            else
                local path_arg = args[i + 1]
                if is_unsafe_path(path_arg) then
                    error("blocked flag: " .. arg .. " " .. path_arg)
                end
                i = i + 2
            end

        else
            -- Check long= forms
            local blocked = false
            for _, prefix in ipairs(path_flag_long_equals) do
                if arg:sub(1, #prefix) == prefix then
                    local path_arg = arg:sub(#prefix + 1)
                    if is_unsafe_path(path_arg) then
                        error("blocked flag: " .. arg)
                    end
                    blocked = true
                    break
                end
            end

            -- Check long forms without = (--file path, --makefile path, etc.)
            if not blocked then
                local long_path_flags = {
                    "--file",
                    "--makefile",
                    "--include-dir",
                    "--old-file",
                    "--what-if",
                    "--new-file",
                    "--assume-new",
                }
                for _, flag in ipairs(long_path_flags) do
                    if arg == flag then
                        if i + 1 <= #args then
                            local path_arg = args[i + 1]
                            if is_unsafe_path(path_arg) then
                                error("blocked flag: " .. arg .. " " .. path_arg)
                            end
                            i = i + 2
                        else
                            i = i + 1
                        end
                        blocked = true
                        break
                    end
                end
            end

            if not blocked then
                i = i + 1
            end
        end
    end

    return { command = command, args = args, working_dir = working_dir }
end
