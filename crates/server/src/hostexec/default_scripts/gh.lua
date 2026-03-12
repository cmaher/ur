-- crates/server/src/hostexec/default_scripts/gh.lua
-- Default gh argument transform: rewrites -C for agents

function transform(command, args, working_dir, agent_context)
    local i = 1
    while i <= #args do
        local arg = args[i]

        -- Handle -C: rewrite if agent_context allows, block otherwise
        if arg == "-C" then
            if agent_context == nil then
                error("blocked flag: -C")
            end
            if i + 1 > #args then
                error("blocked flag: -C (missing path argument)")
            end
            local path_arg = args[i + 1]
            -- Extract final path component (strip trailing slashes, take last segment)
            local stripped = path_arg:gsub("/+$", "")
            local final_component = stripped:match("([^/]+)$") or stripped
            if final_component == agent_context.project_key or final_component == "workspace" then
                args[i + 1] = agent_context.slot_path
                i = i + 2
            else
                error("blocked flag: -C (path '" .. path_arg .. "' does not match project key or 'workspace')")
            end
        else
            i = i + 1
        end
    end

    return args
end
