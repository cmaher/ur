-- crates/server/src/hostexec/default_scripts/bazel.lua
-- Default bazel argument transform: blocks host-state-mutating startup options,
-- commands, and command options.

function transform(command, args, working_dir, worker_context)
    -- Startup options (appear before the command) that mutate host-level state.
    -- Both --flag=val and --flag val forms are blocked.
    local blocked_startup_prefixes = {
        "--output_base",
        "--output_user_root",
        "--install_base",
        "--bazelrc",
        "--server_jvm_out",
    }

    -- Commands that are always blocked.
    local blocked_commands = {
        ["shutdown"] = true,
    }

    -- Command options (appear after the command) that mutate host-level state.
    -- Both --flag=val and --flag val forms are blocked.
    local blocked_command_prefixes = {
        "--override_repository",
        "--disk_cache",
        "--repository_cache",
        "--experimental_repository_cache",
    }

    -- Helper: returns true if arg matches a blocked prefix (--flag= or exact --flag)
    -- and additionally blocks the next arg for space-separated --flag val form.
    -- Returns: "equals" if the = form matched, "space" if exact match (next arg is val),
    -- nil if no match.
    local function matches_blocked_prefix(arg, prefixes)
        for _, prefix in ipairs(prefixes) do
            -- --flag=val form
            local eq_prefix = prefix .. "="
            if arg:sub(1, #eq_prefix) == eq_prefix then
                return "equals"
            end
            -- exact match: --flag (space-separated value follows)
            if arg == prefix then
                return "space"
            end
        end
        return nil
    end

    -- Pass 1: scan startup options (everything before the first non-flag positional).
    -- Startup options start with "--". The first arg that doesn't start with "--" is
    -- the command positional (bazel has no single-dash startup flags).
    local command_pos = nil
    local i = 1
    while i <= #args do
        local arg = args[i]
        if arg:sub(1, 2) ~= "--" then
            -- First non-flag positional: this is the bazel command.
            command_pos = i
            break
        end
        -- Check blocked startup options.
        local form = matches_blocked_prefix(arg, blocked_startup_prefixes)
        if form == "equals" then
            error("blocked startup option: " .. arg)
        elseif form == "space" then
            -- --flag val: block the flag (val is next arg, just error on the flag)
            error("blocked startup option: " .. arg)
        end
        i = i + 1
    end

    -- If no positional was found, bazel was called with only flags or no args — allow.
    if command_pos == nil then
        return { command = command, args = args, working_dir = working_dir }
    end

    local bazel_command = args[command_pos]

    -- Check blocked commands.
    if blocked_commands[bazel_command] then
        error("blocked bazel command: " .. bazel_command)
    end

    -- Special case: `clean --expunge` and `clean --expunge_async` are blocked;
    -- plain `clean` is allowed.
    if bazel_command == "clean" then
        for j = command_pos + 1, #args do
            local a = args[j]
            if a == "--expunge" or a == "--expunge_async" or
               a:sub(1, 10) == "--expunge=" or a:sub(1, 16) == "--expunge_async=" then
                error("blocked bazel command: clean " .. a)
            end
        end
    end

    -- Pass 2: scan command options (everything after the command positional).
    local j = command_pos + 1
    while j <= #args do
        local arg = args[j]
        local form = matches_blocked_prefix(arg, blocked_command_prefixes)
        if form == "equals" then
            error("blocked command option: " .. arg)
        elseif form == "space" then
            error("blocked command option: " .. arg)
        end
        j = j + 1
    end

    return { command = command, args = args, working_dir = working_dir }
end
