-- crates/server/src/hostexec/default_scripts/pnpm.lua
-- Default pnpm argument transform: blocks dangerous subcommands,
-- rewrites -C/--dir for workers, blocks path-escape flags.

function transform(command, args, working_dir, worker_context)

    -- Subcommands that mutate global state, the registry, or the store
    local blocked_subcommands = {
        ["add"]          = true,
        ["remove"]       = true,
        ["dlx"]          = true,
        ["create"]       = true,
        ["publish"]      = true,
        ["login"]        = true,
        ["logout"]       = true,
        ["config"]       = true,
        ["setup"]        = true,
        ["env"]          = true,
        ["server"]       = true,
        ["store"]        = true,
        ["patch"]        = true,
        ["patch-commit"] = true,
        ["rebuild"]      = true,
        ["deploy"]       = true,
    }

    -- Blocked flag prefixes — reject any arg starting with these
    -- (catches both --global-dir=X and bare --global-dir X forms)
    local blocked_flag_prefixes = {
        "--global-dir",
        "--store-dir",
    }

    -- Global flags that consume the next token (skip when searching for subcommand)
    local global_flags_with_value = {
        ["-C"]    = true,
        ["--dir"] = true,
    }

    -- Step 1: scan all args to handle -C/--dir rewriting and blocked flag prefixes.
    local i = 1
    while i <= #args do
        local arg = args[i]

        -- Handle -C / --dir: rewrite if worker_context allows, block otherwise
        if arg == "-C" or arg == "--dir" then
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

        -- Block --dir=<path> equals form outright
        elseif arg:sub(1, 6) == "--dir=" then
            error("blocked flag: --dir=<path> (use --dir <path> instead)")

        else
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
                error("blocked pnpm subcommand: " .. a)
            end
            break
        end
    end

    return { command = command, args = args, working_dir = working_dir }
end
