-- crates/server/src/hostexec/default_scripts/git.lua
-- Default git argument transform: blocks sandbox-escape flags

function transform(command, args, working_dir)
    local blocked_exact = {
        ["-C"] = true,
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

        if blocked_exact[arg] then
            error("blocked flag: " .. arg)
        end

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

    return args
end
