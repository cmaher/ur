-- crates/server/src/hostexec/default_scripts/gh.lua
-- Default gh argument transform: rewrites -C for workers, blocks destructive commands

-- Subcommands that workers must never run (keyed by top-level gh command).
-- Merging is deterministic and server-side only.
local blocked_subcommands = {
    ["pr"] = { ["merge"] = true },
}

function transform(command, args, working_dir, worker_context)
    -- Scan for blocked subcommand patterns (e.g. "gh pr merge")
    -- Find the first two positional arguments, skipping global flags.
    local positionals = {}
    local j = 1
    while j <= #args and #positionals < 2 do
        local a = args[j]
        if a == "--" then
            break
        elseif a:sub(1, 1) == "-" then
            -- Global flags that consume the next argument
            if a == "-R" or a == "--repo" then
                j = j + 2
            else
                j = j + 1
            end
        else
            positionals[#positionals + 1] = a
            j = j + 1
        end
    end

    if #positionals >= 2 then
        local top = positionals[1]
        local sub = positionals[2]
        local blocked_subs = blocked_subcommands[top]
        if blocked_subs and blocked_subs[sub] then
            error("blocked gh command: " .. top .. " " .. sub)
        end
    end

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
            -- Extract final path component (strip trailing slashes, take last segment)
            local stripped = path_arg:gsub("/+$", "")
            local final_component = stripped:match("([^/]+)$") or stripped
            if final_component == worker_context.project_key or final_component == "workspace" then
                args[i + 1] = worker_context.slot_path
                i = i + 2
            else
                error("blocked flag: -C (path '" .. path_arg .. "' does not match project key or 'workspace')")
            end
        else
            i = i + 1
        end
    end

    return { command = command, args = args, working_dir = working_dir }
end
