-- crates/server/src/hostexec/default_scripts/go.lua
-- Default go argument transform: blocks host-mutating subcommands and dangerous flags

function transform(command, args, working_dir, worker_context)
    -- Subcommands that mutate the host environment
    local blocked_subcommands = {
        ["install"] = true,   -- writes to $GOBIN / ~/go/bin on host
        ["telemetry"] = true, -- mutates user's Go telemetry config
    }

    -- Flags that are blocked anywhere in args (with or without =value)
    local blocked_flag_prefixes = {
        "-modfile",
        "-overlay",
        "-pkgdir",
        "-toolexec",
    }

    -- Flags that are blocked only when combined with specific subcommands:
    -- go clean -modcache / -cache / -fuzzcache / -testcache
    local blocked_clean_flags = {
        ["-modcache"] = true,
        ["-cache"] = true,
        ["-fuzzcache"] = true,
        ["-testcache"] = true,
    }

    -- Flags that are blocked only for `go env` (write/unset operations)
    local blocked_env_flags = {
        ["-w"] = true,
        ["-u"] = true,
    }

    -- Helper: extract final path component (strip trailing slashes)
    local function final_component(path)
        local stripped = path:gsub("/+$", "")
        return stripped:match("([^/]+)$") or stripped
    end

    -- First pass: scan for -C <dir> rewriting and blocked global flags.
    -- We also need to identify the subcommand for context-sensitive checks.
    local i = 1
    while i <= #args do
        local arg = args[i]

        -- Handle -C: rewrite to slot_path when path matches project_key or "workspace"
        if arg == "-C" then
            if worker_context == nil then
                error("blocked flag: -C")
            end
            if i + 1 > #args then
                error("blocked flag: -C (missing path argument)")
            end
            local path_arg = args[i + 1]
            local comp = final_component(path_arg)
            if comp == worker_context.project_key or comp == "workspace" then
                args[i + 1] = worker_context.slot_path
                i = i + 2
            else
                error("blocked flag: -C (path '" .. path_arg .. "' does not match project key or 'workspace')")
            end

        else
            -- Check blocked flags (exact match or prefix= forms)
            for _, prefix in ipairs(blocked_flag_prefixes) do
                -- Matches "-flag" or "-flag=..." or "--flag" or "--flag=..."
                if arg == prefix or arg == "-" .. prefix:sub(2) or
                   arg:sub(1, #prefix + 1) == prefix .. "=" or
                   arg:sub(1, #prefix + 1) == "-" .. prefix .. "=" then
                    error("blocked flag: " .. arg)
                end
                -- Handle double-dash variants: "--modfile", "--toolexec", etc.
                local double = "-" .. prefix
                if arg == double or arg:sub(1, #double + 1) == double .. "=" then
                    error("blocked flag: " .. arg)
                end
            end

            i = i + 1
        end
    end

    -- Second pass: find the subcommand (first positional arg after global flags)
    -- Global flags with a following value argument that we need to skip:
    -- -C was already handled above (replaced), so just skip other leading flags.
    local global_flags_with_value = {
        ["-C"] = true,
    }

    local sub_i = 1
    while sub_i <= #args do
        local a = args[sub_i]
        if global_flags_with_value[a] then
            sub_i = sub_i + 2
        elseif a:sub(1, 1) == "-" then
            sub_i = sub_i + 1
        else
            break
        end
    end

    local subcommand = (sub_i <= #args) and args[sub_i] or nil

    -- Block subcommands that mutate the host
    if subcommand ~= nil and blocked_subcommands[subcommand] then
        error("blocked go subcommand: " .. subcommand)
    end

    -- Context-sensitive flag checks after subcommand
    if subcommand == "clean" then
        for j = sub_i + 1, #args do
            if blocked_clean_flags[args[j]] then
                error("blocked flag: go clean " .. args[j])
            end
        end
    end

    if subcommand == "env" then
        for j = sub_i + 1, #args do
            if blocked_env_flags[args[j]] then
                error("blocked flag: go env " .. args[j])
            end
        end
    end

    return { command = command, args = args, working_dir = working_dir }
end
