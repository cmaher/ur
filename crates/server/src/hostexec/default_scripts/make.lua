-- crates/server/src/hostexec/default_scripts/make.lua
-- Default make argument transform: blocks sandbox-escape path flags, rewrites -C for workers

function transform(command, args, working_dir, worker_context)

    -- Check if a path argument is safe (relative and not containing ..)
    local function is_safe_path(path)
        if path:sub(1, 1) == "/" then
            return false
        end
        -- Check for .. components
        if path == ".." or path:find("^%.%./") or path:find("/%.%./") or path:find("/%.%.$") then
            return false
        end
        return true
    end

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

        -- Handle -f <path> (next arg is the path)
        elseif arg == "-f" then
            if i + 1 > #args then
                error("blocked flag: -f (missing path argument)")
            end
            local path_arg = args[i + 1]
            if not is_safe_path(path_arg) then
                error("blocked flag: -f (path '" .. path_arg .. "' is not allowed)")
            end
            i = i + 2

        -- Handle -I <dir> (next arg is the path)
        elseif arg == "-I" then
            if i + 1 > #args then
                error("blocked flag: -I (missing dir argument)")
            end
            local path_arg = args[i + 1]
            if not is_safe_path(path_arg) then
                error("blocked flag: -I (path '" .. path_arg .. "' is not allowed)")
            end
            i = i + 2

        -- Handle -o <file> (next arg is the path)
        elseif arg == "-o" then
            if i + 1 > #args then
                error("blocked flag: -o (missing file argument)")
            end
            local path_arg = args[i + 1]
            if not is_safe_path(path_arg) then
                error("blocked flag: -o (path '" .. path_arg .. "' is not allowed)")
            end
            i = i + 2

        -- Handle -W <file> (next arg is the path)
        elseif arg == "-W" then
            if i + 1 > #args then
                error("blocked flag: -W (missing file argument)")
            end
            local path_arg = args[i + 1]
            if not is_safe_path(path_arg) then
                error("blocked flag: -W (path '" .. path_arg .. "' is not allowed)")
            end
            i = i + 2

        -- Handle long-form path flags with = separator
        else
            local long_path_prefixes = {
                "--file=",
                "--makefile=",
                "--include-dir=",
                "--old-file=",
                "--what-if=",
                "--new-file=",
                "--assume-new=",
            }
            local matched = false
            for _, prefix in ipairs(long_path_prefixes) do
                if arg:sub(1, #prefix) == prefix then
                    local path_arg = arg:sub(#prefix + 1)
                    if not is_safe_path(path_arg) then
                        error("blocked flag: " .. prefix:sub(1, -2) .. " (path '" .. path_arg .. "' is not allowed)")
                    end
                    matched = true
                    break
                end
            end
            if not matched then
                -- Everything else passes through (targets, VAR=value, -j, -k, -n, -B, -s, --debug, --trace, etc.)
            end
            i = i + 1
        end
    end

    return { command = command, args = args, working_dir = working_dir }
end
