-- crates/server/src/hostexec/default_scripts/ur.lua
-- Default ur argument transform: whitelist-only.
-- Workers may use all ticket subcommands and readonly query commands.
-- Mutating commands (start, stop, worker launch/attach/stop/kill, etc.) are blocked.

function transform(command, args, working_dir, agent_context)
    if #args == 0 then
        error("ur: subcommand required")
    end

    local subcmd = args[1]

    -- All ticket subcommands are allowed (create, list, show, update, etc.)
    if subcmd == "ticket" then
        return { command = command, args = args, working_dir = working_dir }
    end

    -- Readonly subcommands for management commands
    local readonly_subcommands = {
        ["worker"] = { ["list"] = true, ["status"] = true, ["dir"] = true },
        ["project"] = { ["list"] = true },
        ["proxy"] = { ["list"] = true },
        ["db"] = { ["list"] = true },
    }

    local allowed_subs = readonly_subcommands[subcmd]
    if allowed_subs then
        local sub = args[2]
        if sub == nil then
            -- No subcommand — ur prints help
            return { command = command, args = args, working_dir = working_dir }
        end
        if allowed_subs[sub] then
            return { command = command, args = args, working_dir = working_dir }
        end
        error("ur " .. subcmd .. " " .. sub .. ": not allowed (not readonly)")
    end

    error("ur " .. subcmd .. ": not allowed")
end
