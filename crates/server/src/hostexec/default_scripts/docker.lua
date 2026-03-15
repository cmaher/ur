-- crates/server/src/hostexec/default_scripts/docker.lua
-- Default docker argument transform: allowlist-only readonly commands.
-- Workers may inspect container/image/network/volume state but cannot
-- create, modify, or destroy resources on the host Docker daemon.

function transform(command, args, working_dir, agent_context)
    -- Top-level commands that are inherently readonly
    local readonly_commands = {
        ["ps"] = true,
        ["images"] = true,
        ["inspect"] = true,
        ["logs"] = true,
        ["stats"] = true,
        ["top"] = true,
        ["port"] = true,
        ["diff"] = true,
        ["history"] = true,
        ["version"] = true,
        ["info"] = true,
        ["search"] = true,
    }

    -- Management commands and their allowed (readonly) subcommands
    local management_readonly = {
        ["container"] = {
            ["ls"] = true,
            ["list"] = true,
            ["inspect"] = true,
            ["logs"] = true,
            ["top"] = true,
            ["port"] = true,
            ["diff"] = true,
            ["stats"] = true,
        },
        ["image"] = {
            ["ls"] = true,
            ["list"] = true,
            ["inspect"] = true,
            ["history"] = true,
        },
        ["network"] = {
            ["ls"] = true,
            ["list"] = true,
            ["inspect"] = true,
        },
        ["volume"] = {
            ["ls"] = true,
            ["list"] = true,
            ["inspect"] = true,
        },
        ["compose"] = {
            ["ps"] = true,
            ["logs"] = true,
            ["config"] = true,
            ["ls"] = true,
            ["top"] = true,
            ["images"] = true,
            ["version"] = true,
        },
        ["system"] = {
            ["df"] = true,
            ["info"] = true,
        },
        ["context"] = {
            ["ls"] = true,
            ["list"] = true,
            ["inspect"] = true,
            ["show"] = true,
        },
        ["buildx"] = {
            ["ls"] = true,
            ["inspect"] = true,
        },
        ["manifest"] = {
            ["inspect"] = true,
        },
    }

    -- Find the first positional argument (skip flags like --host, -H, etc.)
    local first_pos = nil
    local first_pos_idx = nil
    local i = 1
    while i <= #args do
        local arg = args[i]
        if arg == "--" then
            break
        elseif arg:sub(1, 1) == "-" then
            -- Flags that consume the next argument
            if arg == "-H" or arg == "--host" or arg == "-l" or arg == "--log-level"
                or arg == "--config" or arg == "--context" or arg == "--tlscacert"
                or arg == "--tlscert" or arg == "--tlskey" then
                i = i + 2
            else
                i = i + 1
            end
        else
            first_pos = arg
            first_pos_idx = i
            break
        end
    end

    if first_pos == nil then
        -- No subcommand found (e.g., bare `docker` or only flags) — allow through,
        -- docker will just print help/version info
        return { command = command, args = args, working_dir = working_dir }
    end

    -- Check if it's a simple readonly command
    if readonly_commands[first_pos] then
        return { command = command, args = args, working_dir = working_dir }
    end

    -- Check if it's a management command with a readonly subcommand
    local mgmt_subs = management_readonly[first_pos]
    if mgmt_subs then
        -- Find the subcommand (next positional after the management command)
        local sub_pos = nil
        local j = first_pos_idx + 1
        while j <= #args do
            local arg = args[j]
            if arg == "--" then
                break
            elseif arg:sub(1, 1) == "-" then
                j = j + 1
            else
                sub_pos = arg
                break
            end
        end

        if sub_pos == nil then
            -- No subcommand after management command — docker prints help
            return { command = command, args = args, working_dir = working_dir }
        end

        if mgmt_subs[sub_pos] then
            return { command = command, args = args, working_dir = working_dir }
        end

        error("blocked docker command: " .. first_pos .. " " .. sub_pos .. " (not readonly)")
    end

    error("blocked docker command: " .. first_pos .. " (not readonly)")
end
