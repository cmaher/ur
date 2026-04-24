-- crates/server/src/hostexec/default_scripts/go.lua
-- Default go argument transform: blocks host-mutating subcommands and dangerous flags,
-- rewrites -C for workers.

function transform(command, args, working_dir, worker_context)

    -- Subcommands that are unconditionally blocked
    local blocked_subcommands = {
        ["install"]   = true,  -- writes to $GOBIN / ~/go/bin on host
        ["telemetry"] = true,  -- mutates user's Go telemetry config
    }

    -- For these subcommands, certain flags are also blocked
    local clean_blocked_flags = {
        ["-modcache"]   = true,
        ["-cache"]      = true,
        ["-fuzzcache"]  = true,
        ["-testcache"]  = true,
    }

    local env_blocked_flags = {
        ["-w"] = true,
        ["-u"] = true,
    }

    -- Flags that are blocked anywhere in the args list (both -flag <val> and -flag=<val> forms)
    local globally_blocked_flag_prefixes = {
        "-modfile",
        "-overlay",
        "-pkgdir",
        "-toolexec",
    }

    -- Step 1: scan all args to handle -C rewriting and globally-blocked flags.
    -- -C is a global flag that appears before the subcommand in go.
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
        else
            -- Check globally-blocked flags (both -flag <val> and -flag=<val> forms)
            for _, prefix in ipairs(globally_blocked_flag_prefixes) do
                -- Exact flag match (value is the next token)
                if arg == prefix then
                    error("blocked flag: " .. arg)
                end
                -- Equals form: -flag=<value>
                if arg:sub(1, #prefix + 1) == prefix .. "=" then
                    error("blocked flag: " .. prefix)
                end
            end
            i = i + 1
        end
    end

    -- Step 2: find the subcommand — first positional arg after global flags.
    -- Global flags that consume the next token (skip them when searching):
    local global_flags_with_value = {
        ["-C"] = true,
    }
    local sub_i = 1
    local subcommand = nil
    while sub_i <= #args do
        local a = args[sub_i]
        if global_flags_with_value[a] then
            sub_i = sub_i + 2
        elseif a:sub(1, 1) == "-" then
            sub_i = sub_i + 1
        else
            subcommand = a
            break
        end
    end

    -- Step 3: block unconditionally-blocked subcommands
    if subcommand ~= nil and blocked_subcommands[subcommand] then
        error("blocked go subcommand: " .. subcommand)
    end

    -- Step 4: block dangerous flag+subcommand combinations
    if subcommand == "clean" then
        -- Scan args after the subcommand for blocked clean flags
        local j = sub_i + 1
        while j <= #args do
            local flag = args[j]
            if clean_blocked_flags[flag] then
                error("blocked flag: " .. flag .. " (not allowed with 'go clean')")
            end
            j = j + 1
        end
    elseif subcommand == "env" then
        -- Scan args after the subcommand for -w / -u
        local j = sub_i + 1
        while j <= #args do
            local flag = args[j]
            if env_blocked_flags[flag] then
                error("blocked flag: " .. flag .. " (not allowed with 'go env'; use read-only 'go env')")
            end
            j = j + 1
        end
    end

    return { command = command, args = args, working_dir = working_dir }
end
