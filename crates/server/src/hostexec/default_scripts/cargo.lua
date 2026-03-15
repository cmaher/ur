-- crates/server/src/hostexec/default_scripts/cargo.lua
-- Default cargo argument transform: blocks subcommands that escape the project
-- directory or interact with the global crate registry.

function transform(command, args, working_dir, agent_context)
    -- Subcommands that install/remove global binaries or touch the registry
    local blocked_subcommands = {
        ["install"] = true,
        ["uninstall"] = true,
        ["publish"] = true,
        ["yank"] = true,
        ["owner"] = true,
        ["login"] = true,
        ["logout"] = true,
    }

    local blocked_prefix = {
        "--manifest-path=",
    }

    local i = 1
    while i <= #args do
        local arg = args[i]

        -- Block dangerous subcommands (first positional arg that doesn't start with -)
        if arg:sub(1, 1) ~= "-" then
            if blocked_subcommands[arg] then
                error("blocked cargo subcommand: " .. arg)
            end
        end

        -- Handle -C: rewrite if agent_context allows, block otherwise
        if arg == "-C" then
            if agent_context == nil then
                error("blocked flag: -C")
            end
            if i + 1 > #args then
                error("blocked flag: -C (missing path argument)")
            end
            local path_arg = args[i + 1]
            local stripped = path_arg:gsub("/+$", "")
            local final_component = stripped:match("([^/]+)$") or stripped
            if final_component == agent_context.project_key or final_component == "workspace" then
                args[i + 1] = agent_context.slot_path
                i = i + 2
            else
                error("blocked flag: -C (path '" .. path_arg .. "' does not match project key or 'workspace')")
            end
        elseif arg == "--manifest-path" then
            error("blocked flag: --manifest-path")
        else
            for _, prefix in ipairs(blocked_prefix) do
                if arg:sub(1, #prefix) == prefix then
                    error("blocked flag: " .. arg)
                end
            end
            i = i + 1
        end
    end

    return { command = command, args = args, working_dir = working_dir }
end
