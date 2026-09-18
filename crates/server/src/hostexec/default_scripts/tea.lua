-- crates/server/src/hostexec/default_scripts/tea.lua
-- Default tea argument transform: allow non-interactive Gitea pull-request
-- inspection, creation, editing, comments, reviews, thread replies/resolution,
-- approval or change requests, CI inspection, and explicit-strategy merges.
-- Block login/logout/config mutation, insecure TLS, repository or organization
-- administration, checkout-mutating helpers, credentials in arguments,
-- interactive-only forms, and unknown commands.

long_lived = false
bidi = false

local HELP = [[

--- tea via ur host-exec ---
tea runs on the HOST, not inside your container.
Gitea authentication and configuration are host-managed.

Allowed operations:
  tea pr ls|list                        list pull requests (JSON output)
  tea pr view|detail <index>            view pull request details (JSON output)
  tea pr diff <index>                   view pull request diff
  tea pr checks|status <index>          view CI/status checks (JSON output)
  tea pr create --title "..."           create a pull request
  tea pr edit <index> ...               edit a pull request
  tea pr close|reopen <index>           close or reopen a pull request
  tea pr review <index> [--approve|--reject|--comment "..."]
                                        review a pull request
  tea pr approve|reject <index> "..."   approve or request changes
  tea pr comment <index> "..."          comment on a pull request
  tea pr review-comments <index>        list review comments (JSON output)
  tea pr resolve|unresolve <index> <id> resolve or unresolve review threads
  tea pr merge <index> --style <s>      merge PR (explicit style required:
                                        merge, rebase, squash, rebase-merge)
  tea comment <index> "..."             comment on an issue/PR
  tea comments <index>                  list comments on issue/PR (JSON output)
  tea runs ls|view ...                  view CI workflow runs (JSON output)
  tea actions runs|workflows ls         view CI actions and workflows (JSON output)

Blocked:
  tea login|logout|auth                 host-managed authentication
  tea repo|org|admin                    administrative operations
  tea pr checkout|clean, tea clone      git operations must use git
  --insecure, -k                        insecure TLS is not permitted
  Passing credentials/tokens in flags   host-managed credentials
  Interactive commands without flags    must run non-interactively]]

local function fail(msg)
    error(msg .. "\n" .. HELP, 0)
end

-- Flags that take a following argument (not starting with '-')
local flags_with_value = {
    ["-R"] = true, ["--repo"] = true, ["-r"] = true,
    ["--remote"] = true,
    ["--login"] = true, ["-l"] = true,
    ["--output"] = true, ["-o"] = true,
    ["--title"] = true, ["-t"] = true,
    ["--description"] = true, ["-d"] = true,
    ["--body"] = true, ["-b"] = true,
    ["--message"] = true, ["-m"] = true,
    ["--style"] = true, ["-s"] = true,
    ["--head"] = true, ["-H"] = true,
    ["--base"] = true, ["-B"] = true,
    ["--assignees"] = true, ["-a"] = true,
    ["--labels"] = true, ["-L"] = true,
    ["--milestone"] = true, ["-M"] = true,
    ["--comment"] = true, ["-c"] = true,
    ["-C"] = true,
}

local function get_flag_value(args, long_name, short_name)
    for i = 1, #args do
        local a = args[i]
        if long_name and a == long_name then
            return args[i + 1]
        elseif short_name and a == short_name then
            return args[i + 1]
        elseif long_name and a:sub(1, #long_name + 1) == long_name .. "=" then
            return a:sub(#long_name + 2)
        elseif short_name and a:sub(1, #short_name + 1) == short_name .. "=" then
            return a:sub(#short_name + 2)
        end
    end
    return nil
end

local function has_flag(args, ...)
    local names = { ... }
    for _, a in ipairs(args) do
        for _, name in ipairs(names) do
            if a == name or a:sub(1, #name + 1) == name .. "=" then
                return true
            end
        end
    end
    return false
end

function transform(command, args, working_dir, worker_context)
    -- 1. Check for blocked flags anywhere in args
    for _, a in ipairs(args) do
        if a == "--insecure" or a == "-k" or a == "--insecure-skip-tls-verify" then
            fail("blocked flag: " .. a .. " (insecure TLS is not allowed)")
        end
        if a == "--interactive" or a == "-i" then
            fail("blocked flag: " .. a .. " (interactive mode is not allowed)")
        end
        if a == "--token" or a:sub(1, 8) == "--token=" or
           a == "--password" or a:sub(1, 11) == "--password=" or
           a == "--secret" or a:sub(1, 9) == "--secret=" or
           a == "--key" or a:sub(1, 6) == "--key=" then
            fail("blocked: passing credentials or secrets in command arguments is not allowed")
        end
    end

    -- 2. Extract positional arguments, skipping known flags and their values
    local positionals = {}
    local j = 1
    while j <= #args do
        local a = args[j]
        if a == "--" then
            break
        elseif a:sub(1, 1) == "-" then
            if not a:find("=") and flags_with_value[a] then
                j = j + 2
            else
                j = j + 1
            end
        else
            positionals[#positionals + 1] = a
            j = j + 1
        end
    end

    if #positionals == 0 then
        fail("blocked: tea requires a command")
    end

    local raw_top = positionals[1]
    local top = raw_top
    if top == "pull" or top == "pr" then
        top = "pulls"
    elseif top == "issue" or top == "i" then
        top = "issues"
    elseif top == "run" then
        top = "runs"
    end

    -- Blocked commands
    if top == "login" or top == "logins" or top == "logout" or top == "auth" then
        fail("blocked: tea authentication is host-managed; login/logout commands are not allowed")
    end
    if top == "admin" or top == "org" or top == "orgs" or top == "organization" or top == "organizations"
       or top == "user" or top == "users" or top == "repo" or top == "repos" or top == "migrate" then
        fail("blocked: tea " .. raw_top .. " is an administrative or repository management operation and is not allowed")
    end
    if top == "clone" then
        fail("blocked: git clone operations must be performed using git, not tea")
    end

    local is_json_read = false

    if top == "pulls" then
        local raw_sub = positionals[2] or "list"
        local sub = raw_sub
        if sub == "ls" then sub = "list" end
        if sub == "detail" or sub == "show" then sub = "view" end

        if sub == "checkout" then
            fail("blocked: tea pulls checkout is not allowed (git operations must be performed using git)")
        end
        if sub == "clean" then
            fail("blocked: tea pulls clean is not allowed (branch deletion must be performed using git)")
        end

        if sub == "list" then
            is_json_read = true
        elseif sub == "view" then
            is_json_read = true
        elseif sub:match("^%d+$") then
            -- tea pr 42 (view)
            is_json_read = true
        elseif sub == "diff" then
            -- Allowed, raw diff output (not JSON)
        elseif sub == "checks" or sub == "status" then
            is_json_read = true
        elseif sub == "review-comments" then
            is_json_read = true
        elseif sub == "create" then
            local title = get_flag_value(args, "--title", "-t")
            if not title then
                fail("blocked: tea pr create requires --title to run non-interactively")
            end
        elseif sub == "edit" then
            local idx = positionals[3]
            if not idx or not idx:match("^%d+$") then
                fail("blocked: tea pr edit requires a pull request index")
            end
        elseif sub == "close" or sub == "reopen" then
            local idx = positionals[3]
            if not idx or not idx:match("^%d+$") then
                fail("blocked: tea pr " .. sub .. " requires a pull request index")
            end
        elseif sub == "merge" then
            local idx = positionals[3]
            if not idx or not idx:match("^%d+$") then
                fail("blocked: tea pulls merge requires a pull request index")
            end
            local style = get_flag_value(args, "--style", "-s")
            if not style then
                fail("blocked: tea pulls merge requires an explicit --style (merge, rebase, squash, rebase-merge)")
            end
            if style ~= "merge" and style ~= "rebase" and style ~= "squash" and style ~= "rebase-merge" then
                fail("blocked: unsupported merge style '" .. style .. "' (supported: merge, rebase, squash, rebase-merge)")
            end
        elseif sub == "review" then
            local idx = positionals[3]
            if not idx or not idx:match("^%d+$") then
                fail("blocked: tea pr review requires a pull request index")
            end
            local has_action = has_flag(args, "--approve", "--reject", "--request-changes", "--comment", "-c")
            if not has_action then
                fail("blocked: tea pr review requires an action flag (--approve, --reject, or --comment)")
            end
        elseif sub == "approve" or sub == "reject" then
            local idx = positionals[3]
            if not idx or not idx:match("^%d+$") then
                fail("blocked: tea pr " .. sub .. " requires a pull request index")
            end
        elseif sub == "comment" then
            local idx = positionals[3]
            if not idx or not idx:match("^%d+$") then
                fail("blocked: tea pr comment requires a pull request index")
            end
        elseif sub == "comments" then
            local idx = positionals[3]
            if not idx or not idx:match("^%d+$") then
                fail("blocked: tea pr comments requires a pull request index")
            end
            is_json_read = true
        elseif sub == "resolve" or sub == "unresolve" then
            local idx = positionals[3]
            local comment_id = positionals[4]
            if not idx or not idx:match("^%d+$") or not comment_id then
                fail("blocked: tea pr " .. sub .. " requires a pull request index and comment id")
            end
        else
            fail("blocked: tea pulls " .. raw_sub .. " is not allowed")
        end

    elseif top == "issues" then
        local raw_sub = positionals[2] or "list"
        local sub = raw_sub
        if sub == "ls" then sub = "list" end
        if sub == "detail" or sub == "show" then sub = "view" end

        if sub == "list" or sub == "view" or sub:match("^%d+$") or sub == "comments" then
            is_json_read = true
        elseif sub == "comment" then
            local idx = positionals[3]
            if not idx or not idx:match("^%d+$") then
                fail("blocked: tea issues comment requires an issue index")
            end
        elseif sub == "close" or sub == "reopen" then
            local idx = positionals[3]
            if not idx or not idx:match("^%d+$") then
                fail("blocked: tea issues " .. sub .. " requires an issue index")
            end
        else
            fail("blocked: tea issues " .. raw_sub .. " is not allowed")
        end

    elseif top == "comment" then
        local idx = positionals[2]
        if not idx or not idx:match("^%d+$") then
            fail("blocked: tea comment requires an issue or pull request index")
        end

    elseif top == "comments" then
        local idx = positionals[2]
        if not idx or not idx:match("^%d+$") then
            fail("blocked: tea comments requires an issue or pull request index")
        end
        is_json_read = true

    elseif top == "runs" then
        local raw_sub = positionals[2] or "list"
        local sub = raw_sub
        if sub == "ls" then sub = "list" end
        if sub == "show" then sub = "view" end

        if sub == "list" or sub == "view" or sub:match("^%d+$") then
            is_json_read = true
        else
            fail("blocked: tea runs " .. raw_sub .. " is not allowed")
        end

    elseif top == "actions" then
        local sub = positionals[2]
        if sub == "runs" or sub == "workflows" then
            is_json_read = true
        else
            fail("blocked: tea actions " .. tostring(sub) .. " is not allowed")
        end

    else
        fail("blocked: tea " .. raw_top .. " is not allowed")
    end

    -- 3. Enforce / inject JSON output for read operations
    if is_json_read then
        local output_val = get_flag_value(args, "--output", "-o")
        if output_val then
            if output_val:lower() ~= "json" then
                fail("blocked: read operations must use JSON output (--output json)")
            end
        else
            table.insert(args, "--output")
            table.insert(args, "json")
        end
    end

    -- 4. Handle -C flag: rewrite if worker_context allows, block otherwise
    local i = 1
    while i <= #args do
        local arg = args[i]
        if arg == "-C" then
            if worker_context == nil then
                fail("blocked flag: -C")
            end
            if i + 1 > #args then
                fail("blocked flag: -C (missing path argument)")
            end
            local path_arg = args[i + 1]
            local stripped = path_arg:gsub("/+$", "")
            local final_component = stripped:match("([^/]+)$") or stripped
            if final_component == worker_context.project_key or final_component == "workspace" then
                args[i + 1] = worker_context.slot_path
                i = i + 2
            else
                fail("blocked flag: -C (path '" .. path_arg .. "' does not match project key or 'workspace')")
            end
        else
            i = i + 1
        end
    end

    return { command = command, args = args, working_dir = working_dir }
end
