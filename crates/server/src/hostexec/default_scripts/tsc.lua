-- crates/server/src/hostexec/default_scripts/tsc.lua
-- Default tsc argument transform: blocks dangerous flags,
-- rewrites --project/-p for workers, blocks watch and build modes.

function transform(command, args, working_dir, worker_context)

    -- Blocked exact flags (full match required)
    local blocked_exact_flags = {
        ["--watch"]  = true,
        ["-w"]       = true,
        ["--build"]  = true,
        ["-b"]       = true,
    }

    -- Blocked output-configuration flag names (spaced and equals forms)
    local blocked_output_flags = {
        ["--outFile"]         = true,
        ["--outDir"]          = true,
        ["--declarationDir"]  = true,
        ["--tsBuildInfoFile"] = true,
        ["--baseUrl"]         = true,
        ["--rootDir"]         = true,
        ["--rootDirs"]        = true,
    }

    -- Helper: rewrite an absolute path value using worker_context
    local function rewrite_path(flag, value)
        if value:sub(1, 1) == "/" then
            if worker_context == nil then
                error("blocked flag: " .. flag .. " (absolute path requires worker context)")
            end
            local stripped = value:gsub("/+$", "")
            local final_component = stripped:match("([^/]+)$") or stripped
            if final_component == worker_context.project_key or final_component == "workspace" then
                return worker_context.slot_path
            else
                error("blocked flag: " .. flag .. " (path '" .. value .. "' does not match project key or 'workspace')")
            end
        end
        -- Relative path: pass through unchanged
        return value
    end

    local i = 1
    while i <= #args do
        local arg = args[i]

        -- Handle --project <path> and -p <path> (space-separated form)
        if arg == "--project" or arg == "-p" then
            if i + 1 > #args then
                error("blocked flag: " .. arg .. " (missing path argument)")
            end
            args[i + 1] = rewrite_path(arg, args[i + 1])
            i = i + 2

        -- Reject equals forms: --project=<path> and -p=<path>
        elseif arg:sub(1, 10) == "--project=" then
            error("blocked flag: --project=<path> (use --project <path> instead)")
        elseif arg:sub(1, 3) == "-p=" then
            error("blocked flag: -p=<path> (use -p <path> instead)")

        -- Blocked exact flags
        elseif blocked_exact_flags[arg] then
            error("blocked flag: " .. arg)

        else
            -- Check blocked output flags (exact name match, catches both spaced and equals forms)
            -- For equals form e.g. --outDir=/dist, extract the flag name before '='
            local flag_name = arg:match("^([^=]+)")
            if blocked_output_flags[flag_name] then
                error("blocked flag: " .. arg)
            end

            i = i + 1
        end
    end

    return { command = command, args = args, working_dir = working_dir }
end
