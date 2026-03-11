# Host Exec Flow (ur-7jle)

## Overview

Workers execute host commands (git, gh, tk, etc.) through a three-hop gRPC pipeline
with Lua-based validation and CWD mapping.

## Flow

1. Worker calls `git status` (or any configured command)
2. Bash shim at `/home/worker/.local/bin/git` runs `ur-tools host-exec git status`
3. `ur-tools` captures CWD, sends `HostExecRequest` to ur-server (per-agent gRPC)
4. ur-server `HostExecServiceHandler`:
   a. Checks command against merged allowlist (defaults + ~/.ur/hostexec/allowlist.toml)
   b. Maps CWD: /workspace/... -> host workspace path via RepoRegistry
   c. Runs Lua transform if configured (validates/modifies args)
   d. Forwards `HostDaemonExecRequest` to ur-hostd
5. ur-hostd spawns the actual process on the host, streams CommandOutput
6. Output streams back: ur-hostd -> ur-server -> ur-tools -> stdout/stderr

## Shim Generation

At container startup, ur-workerd calls `ListHostExecCommands` on ur-server and
creates bash shims in `/home/worker/.local/bin/` (on PATH, writable by worker user).

## Configuration

- Built-in defaults: git (with git.lua), gh (with gh.lua)
- Global user extensions: ~/.ur/hostexec/allowlist.toml (commands with optional Lua transforms)
- Per-project passthrough commands: `hostexec = ["tk", "make"]` in `ur.toml` `[projects.<key>]`
- Custom Lua scripts: ~/.ur/hostexec/<name>.lua (referenced from allowlist.toml)
- Passthrough commands: `command = {}` in allowlist (no Lua transform)
- Merge order: built-in defaults → global allowlist.toml → per-project hostexec list (passthrough only, does not override existing commands)

## Key Files

- Proto: proto/hostexec.proto, proto/hostd.proto
- Server handler: crates/server/src/grpc_hostexec.rs
- Config: crates/server/src/hostexec/
- Host daemon: crates/hostd/
- Worker tools: crates/workercmd/tools/, crates/workercmd/workerd/
