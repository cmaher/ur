-- crates/server/src/hostexec/default_scripts/ur.lua
-- Default ur argument transform: allowlist-only subcommand filter.
-- Workers get full access to `ticket` subcommands. All other ur
-- subcommands are blocked.

function transform(command, args, working_dir, agent_context)
    -- Subcommands with full access (all sub-subcommands allowed)
    local full_access = {
        ["ticket"] = true,
    }

    -- Find the first positional argument (the subcommand), skipping global flags
    local first_pos = nil
    local i = 1
    while i <= #args do
        local arg = args[i]
        if arg == "--" then
            break
        elseif arg == "--port" or arg == "-p" then
            i = i + 2
        elseif arg:sub(1, 1) == "-" then
            i = i + 1
        else
            first_pos = arg
            break
        end
    end

    if first_pos == nil then
        -- No subcommand — ur prints help
        return { command = command, args = args, working_dir = working_dir }
    end

    if full_access[first_pos] then
        local env = {}
        if agent_context ~= nil and agent_context.project_key ~= nil then
            env["UR_PROJECT"] = agent_context.project_key
        end
        return { command = command, args = args, working_dir = working_dir, env = env }
    end

    error("blocked ur command: " .. first_pos .. " (not allowed)")
end
