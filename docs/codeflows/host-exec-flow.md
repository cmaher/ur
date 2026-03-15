# Host Exec Flow (ur-7jle)

## Overview

Workers execute host commands (git, gh, tk, etc.) through a three-hop gRPC pipeline
with Lua-based validation and `%WORKSPACE%` CWD templating.

## Flow

1. Worker calls `git status` (or any configured command)
2. Bash shim at `/home/worker/.local/bin/git` runs `workertools host-exec git status`
3. `workertools` captures CWD, sends `HostExecRequest` to ur-server (per-agent gRPC)
4. ur-server `HostExecServiceHandler`:
   a. Checks command against merged allowlist (defaults + `[hostexec.commands]` from ur.toml)
   b. Maps CWD: `/workspace/...` -> `%WORKSPACE%/...` (template prefix, not a resolved host path)
   c. Runs Lua transform if configured (validates/modifies args)
   d. Forwards `BuilderDaemonExecRequest` to builderd
5. builderd resolves `%WORKSPACE%` to its local workspace path (from env var or CLI flag), spawns the actual process
6. Output streams back: builderd -> ur-server -> workertools -> stdout/stderr

## Shim Generation

At container startup, `workerd init` calls `ListHostExecCommands` on ur-server and
creates bash shims in `/home/worker/.local/bin/` (on PATH, writable by worker user).

## Configuration

- Built-in defaults: git (with git.lua), gh (with gh.lua), cargo (with cargo.lua)
- Global hostexec commands: `[hostexec.commands]` section in `ur.toml` (commands with optional Lua transforms)
- Per-project passthrough commands: `hostexec = ["tk", "make"]` in `ur.toml` `[projects.<key>]`
- Custom Lua scripts: ~/.ur/hostexec/<name>.lua (referenced from `[hostexec.commands]`)
- Passthrough commands: `command = {}` in `[hostexec.commands]` (no Lua transform)
- Merge order: built-in defaults -> global `[hostexec.commands]` -> per-project hostexec list (passthrough only, does not override existing commands)

## Key Files

- Proto: proto/hostexec.proto, proto/builder.proto
- Server handler: crates/server/src/grpc_hostexec.rs
- Config: crates/server/src/hostexec/
- Builder daemon: crates/builderd/
- Worker tools: crates/workertools/, crates/workerd/
