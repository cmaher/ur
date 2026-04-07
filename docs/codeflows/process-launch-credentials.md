# Process Launch & Credential Injection

How `ur process launch` starts a worker container with Claude Code credentials.

## Credential Flow

```
macOS: Keychain ("Claude Code-credentials")
Linux: ~/.claude/.credentials.json
    │
    ▼
ur CLI: CredentialManager.ensure_credentials(max_age) [crates/ur/src/credential.rs]
    │   re-seeds if ~/.ur/claude/.credentials.json is missing, empty, or older than max_age:
    │     macOS: runs: security find-generic-password -s "Claude Code-credentials" -w
    │     Linux: reads ~/.claude/.credentials.json directly
    │     writes result to ~/.ur/claude/.credentials.json
    │   max_age callers:
    │     ur start          → Duration::ZERO (force re-seed every restart)
    │     ur worker launch  → 1 day (re-seed if file is stale; otherwise let containers refresh)
    │     ur worker reseed-credentials → Duration::ZERO (manual force re-seed)
    │
    ▼
ur CLI: process_launch()                              [crates/ur/src/main.rs]
    │   sends WorkerLaunchRequest via gRPC
    │
    ▼  gRPC (TCP → 127.0.0.1:42069 → ur-server container)
    │
ur-server: WorkerManager.run_and_record()            [crates/server/src/worker.rs]
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

**Session ownership:** Credentials are seeded from the host Claude Code installation (macOS Keychain or Linux credentials file) on `ur start` and on `ur worker launch` when the shared file is older than a day. Between re-seeds, containers own their token lifecycle — refreshes write back to the shared mount without touching the host credentials. The age check lets host re-logins propagate without clobbering fresh container-driven token refreshes on every launch. To force a re-seed without restarting, run `ur worker reseed-credentials`.

## Process Launch Sequence

```
ur worker launch <ticket-id> [-w <workspace>] [-a] [-f]

1. CLI (host)
   ├── -f flag? → kill_container() (docker stop + rm)
   ├── CredentialManager.ensure_credentials(max_age = 1 day)
   │   └── re-seed from host Claude Code if ~/.ur/claude/.credentials.json
   │       is missing, empty, or older than max_age
   │       (macOS: Keychain, Linux: ~/.claude/.credentials.json)
   ├── connect() → gRPC channel to server at 127.0.0.1:<port>
   └── client.worker_launch(WorkerLaunchRequest { ... })

2. Server (ur-server container)
   ├── Phase 1: WorkerManager.prepare()
   │   ├── Check for duplicate worker_id
   │   ├── workspace_dir provided? → register_absolute (no git init)
   │   └── no workspace? → create dir, git init, register
   │
   ├── Spawn per-worker gRPC server
   │   └── TCP on 0.0.0.0:<random_port> (reachable via Docker network)
   │
   └── Phase 2: WorkerManager.run_and_record()
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
   ├── Start tmux session "worker"
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
| `UR_SERVER_ADDR` | WorkerManager (server) | worker binaries (git, gh, ur-ping) | gRPC endpoint for proxied commands |
| `HTTP_PROXY` / `HTTPS_PROXY` | WorkerManager (server) | Claude Code, curl, etc. | Squid forward proxy |
| `GH_TOKEN` / `GITHUB_TOKEN` | docker-compose.yml | gh CLI (on server) | GitHub auth for server-side git/gh |

## Key Files

| File | Purpose |
|---|---|
| `crates/ur/src/credential.rs` | CredentialManager: Keychain seeding, save-from-container |
| `crates/ur/src/main.rs` | CLI entry, `process_launch()` calls `ensure_credentials()` |
| `crates/server/src/worker.rs` | WorkerManager: bind-mounts credentials, launches containers |
| `crates/server/src/grpc.rs` | Server RPC handler, maps request to WorkerConfig |
| `containers/claude-worker/claude.json` | Baked-in `.claude.json` (onboarding + project trust) |
| `containers/claude-worker/entrypoint.sh` | Starts tmux, keeps container alive |
| `containers/claude-worker/claude-settings.json` | Baked-in settings (bypassPermissions mode) |
| `containers/claude-worker/Dockerfile.base` | Base image with Claude Code installed as worker user |
| `containers/claude-worker/Dockerfile` | Final image with worker binaries and config |

## Manual Credential Management

- `ur worker reseed-credentials` — force re-seed `~/.ur/claude/.credentials.json` from the host (Keychain on macOS, `~/.claude/.credentials.json` on Linux). Use after re-logging into Claude Code on the host when you don't want to wait for the next `ur start` or 1-day age trigger.
- `ur worker save-credentials <id>` — copy `.credentials.json` and `.claude.json` from a running container to `~/.ur/claude/`. Useful for bootstrapping from a container login.
- Delete `~/.ur/claude/.credentials.json` to force re-seeding from host credentials on next launch.
