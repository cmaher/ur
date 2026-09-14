# Host Exec Flow (ur-7jle)

## Overview

Workers execute host commands (git, gh, etc.) through a three-hop gRPC pipeline
with Lua-based validation and `%WORKSPACE%` CWD templating.  A second, parallel
path handles path-based script invocations: project-declared shell scripts that
are bind-mounted into the container and forwarded to builderd without Lua or
allowlist checks.

## Command Flow (PATH-based)

1. Worker calls `git status` (or any configured command)
2. Bash shim at `/home/worker/.local/bin/git` runs `workertools host-exec git status`
3. `workertools` captures CWD, sends `HostExecRequest` to ur-server (per-worker gRPC)
4. ur-server `HostExecServiceHandler`:
   a. Checks command against the built-in defaults plus commands granted by the project
   b. Maps CWD: `/workspace/...` -> `%WORKSPACE%/...` (template prefix, not a resolved host path)
   c. Runs Lua transform if configured (validates/modifies args)
   d. Forwards `BuilderDaemonExecRequest` to builderd
5. builderd resolves `%WORKSPACE%` to its local workspace path (from env var or CLI flag), spawns the actual process
6. Output streams back: builderd -> ur-server -> workertools -> stdout/stderr

## Script Flow (path-based, no Lua, no allowlist)

Workers can invoke project-local scripts (`./needs-to-run-on-host.sh`) that
execute on the host without appearing on PATH.

```
Worker container                   ur-server                    builderd / host
────────────────                   ─────────                    ───────────────
$ ./needs-to-run-on-host.sh foo
  ▼
/workspace/needs-to-run-on-host.sh  (shim, bind-mounted; original
                                     project script untouched on host)
  ▼
exec workertools host-exec
  --script /workspace/needs-to-run-on-host.sh foo
  ▼ HostExecMessage::Start{ script_path, args, working_dir }
                                   ▼
                                   require non-empty project_key
                                   strip /workspace/ prefix
                                   script_registry.allows(project_key, rel_path)
                                     → PermissionDenied/SCRIPT_NOT_ALLOWED on miss
                                   map → %WORKSPACE%/<rel_path>  (slot-relative in pool mode)
                                   no Lua, no allowlist check
                                   ▼ BuilderExecRequest{ command: %WORKSPACE% path, ... }
                                                                ▼
                                                                spawn original host script,
                                                                stream output back
```

### Start Frame Dispatch

`HostExecRequest` (proto field 4 `script_path`) controls which branch runs:

- `script_path` non-empty, `command` empty → script flow
- `command` non-empty, `script_path` empty → command flow (existing)
- Both empty or both set → `InvalidArgument`

### Server-Side Script Branch

1. Require worker context with non-empty `project_key` (anonymous workers are rejected).
2. Strip `/workspace/` prefix from `script_path`; reject if missing or path contains `..`.
3. `ScriptRegistry::allows(project_key, rel_path)` — built from `ProjectRegistry` at server start, refreshed on `ur.toml` reload.  On miss → `PermissionDenied`, error code `SCRIPT_NOT_ALLOWED`, metadata `{script, project}`.
4. Map `rel_path` to `%WORKSPACE%/<rel_path>` via the same `map_working_dir` logic (prepends the slot-relative prefix in pool mode).
5. Build `BuilderExecRequest { command: <template>, args: passthrough, working_dir: <CWD mapping>, env: empty, long_lived: false }`.
6. Forward via `forward_to_builderd`.

No Lua transform runs and no command allowlist is consulted — the `hostexec_scripts` registry is the sole gate.

### Shim Materialization (server start)

At server startup, `HostExecServiceHandler` materializes a static shim file at
`$URCONFIG/hostexec/script-shim.sh` (mode 0755) via atomic temp-file + rename.
The shim content:

```sh
#!/bin/sh
exec workertools host-exec --script "$(readlink -f "$0")" "$@"
```

The file is overwritten idempotently if content differs.  `readlink -f` requires
coreutils, which is present in the `ur-worker` base image.

### Per-Script Bind Mounts (worker launch)

`RunOptsBuilder::add_project_hostexec_scripts` is called during worker-launch
setup alongside `add_mounts`, `add_git_hooks`, etc.  For each entry in
`project.hostexec_scripts` it appends:

```
<host_shim_path>:/workspace/<rel_path>:ro
```

where `<host_shim_path>` is the materialized `script-shim.sh` on the host.
The bind mount shadows the container path with the shim, while the original
script on the host filesystem remains untouched.  No-op when the list is empty.

### Configuration (`hostexec_scripts`)

Declared per project in `ur.toml`:

```toml
[projects.myproject]
hostexec_scripts = [
    "./needs-to-run-on-host.sh",
    "./scripts/deploy.sh",
]
```

Normalization rules (enforced at `Config::load()`):

- Leading `./` is stripped; canonical form is a clean relative path (e.g. `scripts/deploy.sh`).
- Rejected: absolute paths, paths with `..` segments, paths starting with `%PROJECT%`, empty strings.
- Defaults to `[]` when omitted.

### Known Caveat: `bash ./script.sh` Bypasses the Shim

When the worker invokes a script via an explicit interpreter
(`bash ./script.sh` or `sh -c './script.sh'`), the kernel never reads the
shim's shebang and the intercept is silently skipped.  The script runs
*inside the container* as a plain shell file.  This is the same documented
limitation as existing PATH shims invoked via `sh -c`.  Workers must invoke
scripts directly (`./script.sh`) for the host-exec path to activate.

## Shim Generation (PATH commands)

At container startup, `workerd init` calls `ListHostExecCommands` on ur-server and
creates bash shims in `/home/worker/.local/bin/` (on PATH, writable by worker user).
`ListHostExecCommands` is unchanged for the script flow — scripts are not surfaced
as PATH commands.

## Configuration (command flow)

`HostExecConfigManager` starts with the 11 baked-in commands (`git`, `gh`,
`cargo`, `docker`, `ur`, `make`, `go`, `bazel`, `npm`, `pnpm`, and `tsc`). It
then scans `$UR_CONFIG/hostexec/*.lua`; each filename stem is the command name,
so `daemon.lua` registers `daemon`. A discovered file with a built-in name, such
as `git.lua`, shadows that built-in entirely. Non-Lua files and subdirectories
are ignored.

Discovery populates the registry but does not grant access. Every project gets
the built-ins automatically. A project grants an additional discovered command
with its existing array:

```toml
[projects.myproject]
hostexec = ["daemon", "jq"]
```

A granted name with no discovered Lua file remains a plain passthrough command.
A discovered name absent from the project's grant array is unreachable by that
project. The effective merge order is therefore baked-in defaults → discovered
Lua overlay → per-project grant filter.

Lua scripts declare process metadata as top-level globals. Both default to
`false`; `bidi = true` requires `long_lived = true`.

```lua
-- $UR_CONFIG/hostexec/daemon.lua
long_lived = true
bidi = true

function transform(command, args, working_dir, worker_context)
  return { command = command, args = args, working_dir = working_dir }
end
```

The server executes each script once in the restricted transform sandbox during
startup and config reload. A syntax error, non-boolean metadata value, invalid
`bidi`/`long_lived` combination, or missing `transform` function aborts the load
and names the offending file.

The `[hostexec]` TOML section has been removed. Upgrading with that section still
present is a hard config error that lists the old command keys and directs the
operator to delete the section after placing transforms at
`$UR_CONFIG/hostexec/<command>.lua`; it is never silently ignored.

## Key Files

- Proto: proto/hostexec.proto, proto/builder.proto
- Server handler: crates/server/src/grpc_hostexec.rs
- Script registry: crates/server/src/hostexec/script_registry.rs
- Config: crates/server/src/hostexec/
- Run opts builder: crates/server/src/run_opts_builder.rs
- Builder daemon: crates/builderd/
- Worker tools: crates/workertools/, crates/workerd/
