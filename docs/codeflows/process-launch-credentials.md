# Process Launch & Credential Injection

How `ur process launch` starts a worker container with Claude Code credentials.

## Credential Flow

```
macOS Keychain ("Claude Code-credentials")
    │
    ▼
ur CLI: CredentialManager.ensure_credentials()        [crates/ur/src/credential.rs]
    │   if ~/.ur/claude/.credentials.json missing:
    │     runs: security find-generic-password -s "Claude Code-credentials" -w
    │     writes result to ~/.ur/claude/.credentials.json
    │   (no-op if file already exists — containers own their session after seeding)
    │
    ▼
ur CLI: process_launch()                              [crates/ur/src/main.rs]
    │   sends ProcessLaunchRequest via gRPC
    │
    ▼  gRPC (TCP → 127.0.0.1:42069 → ur-server container)
    │
ur-server: ProcessManager.run_and_record()            [crates/server/src/process.rs]
    │   bind-mounts ~/.ur/claude/.credentials.json
    │   → /home/worker/.claude/.credentials.json
    │
    ▼  docker run (volume mount)
    │
Container: Claude Code reads ~/.claude/.credentials.json
            Claude Code reads ~/.claude.json (baked into image)
```

**Two files are required for Claude Code to skip login:**
- `~/.claude/.credentials.json` — OAuth tokens (bind-mounted from host, shared across all containers)
- `~/.claude.json` — App config with `hasCompletedOnboarding` and project trust (baked into image)

**Session ownership:** Credentials are seeded once from the macOS Keychain. After that, containers own their token lifecycle — refreshes write back to the shared mount without touching the host Keychain. This avoids token rotation conflicts between host and container Claude Code sessions.

## Process Launch Sequence

```
ur process launch <ticket-id> [-w <workspace>] [-a] [-f]

1. CLI (host)
   ├── -f flag? → kill_container() (docker stop + rm)
   ├── CredentialManager.ensure_credentials()
   │   └── seed from Keychain if ~/.ur/claude/.credentials.json missing
   ├── connect() → gRPC channel to server at 127.0.0.1:<port>
   └── client.process_launch(ProcessLaunchRequest { ... })

2. Server (ur-server container)
   ├── Phase 1: ProcessManager.prepare()
   │   ├── Check for duplicate process_id
   │   ├── workspace_dir provided? → register_absolute (no git init)
   │   └── no workspace? → create dir, git init, register
   │
   ├── Spawn per-agent gRPC server
   │   └── TCP on 0.0.0.0:<random_port> (reachable via Docker network)
   │
   └── Phase 2: ProcessManager.run_and_record()
       ├── NetworkManager.ensure() (create Docker network if needed)
       ├── Build env vars:
       │   ├── UR_SERVER_ADDR = <server_hostname>:<grpc_port>
       │   ├── HTTP_PROXY / HTTPS_PROXY = http://ur-squid:3128
       │   └── NO_PROXY = ""
       ├── Build volumes:
       │   ├── workspace_dir → /workspace (if provided)
       │   └── ~/.ur/claude/.credentials.json → /home/worker/.claude/.credentials.json
       ├── docker run (image: ur-worker:latest, network: worker network)
       └── Record ProcessEntry { container_id, grpc_port, server_handle }

3. Container startup (entrypoint.sh)
   ├── mkdir -p ~/.claude
   ├── Start tmux session "agent"
   └── exec sleep infinity
```

## Container Image Layers

```
Dockerfile.base (ur-worker-base:latest)
├── debian:bookworm-slim + system packages
├── useradd worker
├── USER worker → install-claude.sh (binary at /home/worker/.local/bin/claude)
└── USER root → cleanup

Dockerfile (ur-worker:latest)
├── FROM ur-worker-base:latest
├── COPY entrypoint.sh, worker binaries (ur-ping, git, gh, tk)
├── COPY claude.json → /home/worker/.claude.json (skip onboarding/login prompts)
├── COPY claude-settings.json → /home/worker/.claude/settings.json
│   └── permissions.defaultMode: "bypassPermissions" (skip permissions for non-interactive use)
├── USER worker
└── ENTRYPOINT ["/entrypoint.sh"]
```

Permissions are bypassed via `settings.json` (`permissions.defaultMode: "bypassPermissions"`) rather than a CLI flag, so no wrapper script is needed.

## Environment Variables

| Variable | Set by | Consumed by | Purpose |
|---|---|---|---|
| `UR_SERVER_ADDR` | ProcessManager (server) | worker binaries (git, gh, ur-ping) | gRPC endpoint for proxied commands |
| `HTTP_PROXY` / `HTTPS_PROXY` | ProcessManager (server) | Claude Code, curl, etc. | Squid forward proxy |
| `GH_TOKEN` / `GITHUB_TOKEN` | docker-compose.yml | gh CLI (on server) | GitHub auth for server-side git/gh |

## Key Files

| File | Purpose |
|---|---|
| `crates/ur/src/credential.rs` | CredentialManager: Keychain seeding, save-from-container |
| `crates/ur/src/main.rs` | CLI entry, `process_launch()` calls `ensure_credentials()` |
| `crates/server/src/process.rs` | ProcessManager: bind-mounts credentials, launches containers |
| `crates/server/src/grpc.rs` | Server RPC handler, maps request to ProcessConfig |
| `containers/claude-worker/claude.json` | Baked-in `.claude.json` (onboarding + project trust) |
| `containers/claude-worker/entrypoint.sh` | Starts tmux, keeps container alive |
| `containers/claude-worker/claude-settings.json` | Baked-in settings (bypassPermissions mode) |
| `containers/claude-worker/Dockerfile.base` | Base image with Claude Code installed as worker user |
| `containers/claude-worker/Dockerfile` | Final image with worker binaries and config |

## Manual Credential Management

- `ur process save-credentials <id>` — copy `.credentials.json` and `.claude.json` from a running container to `~/.ur/claude/`. Useful for refreshing stale credentials or bootstrapping from a container login.
- Delete `~/.ur/claude/.credentials.json` to force re-seeding from Keychain on next launch.
