-- crates/server/src/hostexec/default_scripts/bazel.lua
-- Default bazel argument transform: blocks host-state-mutating startup options,
-- dangerous commands, and dangerous command options.
--
-- Bazel args are structured as:
--   bazel [startup-options] <command> [command-options] [targets]
--
-- Startup options start with '--' and appear before the command positional.
-- The command is the first non-flag positional argument.
-- Command options appear after the command.

function transform(command, args, working_dir, worker_context)

    -- Startup options that are blocked (appear before the command)
    local blocked_startup_flags = {
        ["--output_base"]        = true,
        ["--output_user_root"]   = true,
        ["--install_base"]       = true,
        ["--bazelrc"]            = true,
        ["--server_jvm_out"]     = true,
    }

    -- Command options that are blocked (appear after the command)
    local blocked_command_flags = {
        ["--override_repository"]             = true,
        ["--disk_cache"]                      = true,
        ["--repository_cache"]                = true,
        ["--experimental_repository_cache"]   = true,
    }

    -- Helper: check if a flag name (without =value) is in the given blocked set.
    -- Handles both "--flag=val" (extract "--flag") and bare "--flag" forms.
    local function is_blocked_flag(arg, blocked_set)
        -- Check exact match (space-separated value form)
        if blocked_set[arg] then
            return true
        end
        -- Check "--flag=val" form
        local eq_pos = arg:find("=", 1, true)
        if eq_pos then
            local flag_name = arg:sub(1, eq_pos - 1)
            if blocked_set[flag_name] then
                return true
            end
        end
        return false
    end

    -- Phase 1: scan startup options (all args starting with '--' before the command).
    -- Advance past value tokens for space-separated blocked flags.
    local i = 1
    while i <= #args do
        local arg = args[i]

        -- Stop at the first non-flag positional (the command)
        if arg:sub(1, 2) ~= "--" then
            break
        end

        -- Check if this startup flag is blocked
        if is_blocked_flag(arg, blocked_startup_flags) then
            -- Determine the flag name for the error message
            local eq_pos = arg:find("=", 1, true)
            local flag_name = eq_pos and arg:sub(1, eq_pos - 1) or arg
            error("blocked bazel startup option: " .. flag_name)
        end

        -- For blocked flags in space-separated form the check above already errors.
        -- For any startup flag with a value token (--flag val), we need to skip
        -- the value so it does not get mistaken for the command positional.
        -- We only need to do this for blocked flags (already handled above);
        -- other startup flags may or may not take values, but since we are only
        -- blocking specific flags, safe to advance by 1 for all bare '--flag' forms.
        -- (Bazel startup flags that take values: --output_base, etc. — all blocked.)
        -- Unrecognised startup flags pass through; advance by 1 token.
        i = i + 1
    end

    -- Phase 2: the current position is the command (first non-flag positional).
    local cmd_index = i
    local cmd = args[cmd_index]

    if cmd == nil then
        -- No command provided — pass through
        return { command = command, args = args, working_dir = working_dir }
    end

    -- Block the 'shutdown' command outright
    if cmd == "shutdown" then
        error("blocked bazel command: shutdown")
    end

    -- Phase 3: scan command options (args after the command).
    -- For 'clean', check for --expunge / --expunge_async.
    -- For all commands, check for blocked command options.
    local j = cmd_index + 1
    while j <= #args do
        local arg = args[j]

        -- Check blocked command flags
        if is_blocked_flag(arg, blocked_command_flags) then
            local eq_pos = arg:find("=", 1, true)
            local flag_name = eq_pos and arg:sub(1, eq_pos - 1) or arg
            error("blocked bazel command option: " .. flag_name)
        end

        -- For 'clean': block --expunge and --expunge_async
        if cmd == "clean" then
            if arg == "--expunge" or arg == "--expunge_async" then
                error("blocked bazel clean option: " .. arg)
            end
            -- Also handle --expunge=... form (unlikely but consistent)
            if arg:sub(1, 10) == "--expunge=" then
                error("blocked bazel clean option: --expunge")
            end
        end

        -- Advance: if this is a space-separated blocked command flag, skip next token too
        if blocked_command_flags[arg] then
            j = j + 2
        else
            j = j + 1
        end
    end

    return { command = command, args = args, working_dir = working_dir }
end
