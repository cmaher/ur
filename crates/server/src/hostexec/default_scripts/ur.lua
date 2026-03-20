-- crates/server/src/hostexec/default_scripts/ur.lua
-- Default ur argument transform: whitelist-only.
-- Workers may use all ticket subcommands and readonly query commands.
-- Mutating commands (start, stop, worker launch/attach/stop/kill, etc.) are blocked.

function transform(command, args, working_dir, worker_context)
    if #args == 0 then
        error("ur: subcommand required")
    end

    local subcmd = args[1]

    -- Admin subcommands are never allowed from workers
    if subcmd == "admin" then
        error("ur admin: not allowed (privileged operation)")
    end

    -- All ticket subcommands are allowed (create, list, show, update, etc.)
    -- but --force is blocked on update/close (could force-close epics with open children)
    if subcmd == "ticket" then
        local ticket_sub = args[2]
        if ticket_sub == "update" or ticket_sub == "close" then
            for i = 3, #args do
                if args[i] == "--force" then
                    error("ur ticket " .. ticket_sub .. " --force: not allowed from workers")
                end
            end
        end

        -- Pass UR_PROJECT env var so the ur CLI resolves the correct project scope.
        -- The CLI's resolve_project() reads UR_PROJECT as a fallback after --project/-p.
        local env = {}
        if worker_context ~= nil and worker_context.project_key ~= nil then
            env["UR_PROJECT"] = worker_context.project_key
        end

        return { command = command, args = args, working_dir = working_dir, env = env }
    end

    -- Readonly subcommands for management commands
    local readonly_subcommands = {
        ["worker"] = { ["list"] = true, ["describe"] = true, ["dir"] = true },
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
