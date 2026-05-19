-- crates/server/src/hostexec/default_scripts/npm.lua
-- Default npm argument transform: blocks dangerous subcommands,
-- rewrites --prefix/--cwd for workers, blocks path-escape flags.

function transform(command, args, working_dir, worker_context)

    -- Subcommands that mutate global state, the registry, or perform unsafe operations
    local blocked_subcommands = {
        ["add"]        = true,
        ["uninstall"]  = true,
        ["remove"]     = true,
        ["rm"]         = true,
        ["un"]         = true,
        ["unlink"]     = true,
        ["publish"]    = true,
        ["login"]      = true,
        ["logout"]     = true,
        ["adduser"]    = true,
        ["config"]     = true,
        ["set"]        = true,
        ["get"]        = true,
        ["version"]    = true,
        ["link"]       = true,
        ["token"]      = true,
        ["owner"]      = true,
        ["profile"]    = true,
        ["team"]       = true,
        ["org"]        = true,
        ["hook"]       = true,
        ["access"]     = true,
        ["deprecate"]  = true,
        ["dist-tag"]   = true,
        ["star"]       = true,
        ["unstar"]     = true,
        ["init"]       = true,
        ["rebuild"]    = true,
    }

    -- Blocked exact flags — short flags that must match exactly
    local blocked_exact_flags = {
        ["-g"]       = true,
        ["--global"] = true,
    }

    -- Blocked flag prefixes — reject any arg starting with these
    local blocked_flag_prefixes = {
        "--cache",
        "--globalconfig",
        "--userconfig",
        "--location",
    }

    -- Global flags that consume the next token (skip when searching for subcommand)
    local global_flags_with_value = {
        ["--prefix"] = true,
        ["--cwd"]    = true,
    }

    -- Step 1: scan all args to handle --prefix/--cwd rewriting and blocked flags.
    local i = 1
    while i <= #args do
        local arg = args[i]

        -- Handle --prefix / --cwd: rewrite if worker_context allows, block otherwise
        if arg == "--prefix" or arg == "--cwd" then
            if worker_context == nil then
                error("blocked flag: " .. arg)
            end
            if i + 1 > #args then
                error("blocked flag: " .. arg .. " (missing path argument)")
            end
            local path_arg = args[i + 1]
            local stripped = path_arg:gsub("/+$", "")
            local final_component = stripped:match("([^/]+)$") or stripped
            if final_component == worker_context.project_key or final_component == "workspace" then
                args[i + 1] = worker_context.slot_path
                i = i + 2
            else
                error("blocked flag: " .. arg .. " (path '" .. path_arg .. "' does not match project key or 'workspace')")
            end

        -- Block --prefix=<path> and --cwd=<path> equals forms outright
        elseif arg:sub(1, 9) == "--prefix=" then
            error("blocked flag: --prefix=<path> (use --prefix <path> instead)")
        elseif arg:sub(1, 6) == "--cwd=" then
            error("blocked flag: --cwd=<path> (use --cwd <path> instead)")

        else
            -- Check blocked exact flags
            if blocked_exact_flags[arg] then
                error("blocked flag: " .. arg)
            end
            -- Check blocked flag prefixes
            for _, prefix in ipairs(blocked_flag_prefixes) do
                if arg:sub(1, #prefix) == prefix then
                    error("blocked flag: " .. arg)
                end
            end
            i = i + 1
        end
    end

    -- Step 2: find the subcommand — first positional arg after global flags.
    local sub_i = 1
    while sub_i <= #args do
        local a = args[sub_i]
        if global_flags_with_value[a] then
            sub_i = sub_i + 2
        elseif a:sub(1, 1) == "-" then
            sub_i = sub_i + 1
        else
            -- Found the subcommand
            if blocked_subcommands[a] then
                error("blocked npm subcommand: " .. a)
            end
            break
        end
    end

    return { command = command, args = args, working_dir = working_dir }
end
