# Process Launch & Credential Injection

How `ur process launch` starts a worker container with agent credentials.

## Credential Flow

```
Claude — macOS: Keychain ("Claude Code-credentials")
         Linux: ~/.claude/.credentials.json
Codex  — any OS: ~/.codex/auth.json (no keychain integration)
    │
    ▼
ur CLI: credential_manager_for(agent) -> Option<Box<dyn AgentCredentialManager>>
    │                                       [crates/ur/src/credential/mod.rs]
    │   None for a no-auth agent (agent.auth() is None) — every caller acknowledges
    │   the no-auth path instead of the trait silently no-op'ing.
    │   ClaudeCredentialManager [credential/claude.rs] and CodexCredentialManager
    │   [credential/codex.rs] each implement ensure_credentials(max_age): re-seeds
    │   if ~/.ur/{agent.name()}/{filename component of auth.credentials_path} is
    │   missing, empty, or older than max_age:
    │       Claude, macOS: runs: security find-generic-password -s "<service>" -w
    │       Claude, Linux: reads ~/.claude/.credentials.json directly
    │       Codex, any OS: reads ~/.codex/auth.json directly
    │       writes result to ~/.ur/{claude,codex}/{credentials file}
    │   A missing host source (never logged into that agent) is a skip with a
    │   warning, not a failure — one agent's absent login must not break
    │   ur start for users of the other agent. A host source that exists but is
    │   unreadable or empty is a hard failure.
    │   max_age callers (via the shared ensure_credentials_for_all_agents(max_age) helper,
    │   which loops AgentType::ALL so it is correct for any number of agents, and
    │   aggregates per-agent failures instead of short-circuiting):
    │     ur start          → Duration::ZERO (force re-seed every restart)
    │     ur worker launch  → 1 day (re-seed if file is stale; otherwise let containers refresh)
    │     ur worker reseed-credentials → Duration::ZERO (manual force re-seed, takes --agent)
    │
    ▼
ur CLI: process_launch()                              [crates/ur/src/main.rs]
    │   sends WorkerLaunchRequest via gRPC (agent_type left empty — no --agent flag
    │   on launch yet; server defaults empty to claude)
    │
    ▼  gRPC (TCP → 127.0.0.1:12321 → ur-server container)
    │
ur-server: WorkerManager.run_and_record()            [crates/server/src/worker.rs]
    │   assembles RunOptsBuilder (volumes, env vars, network, image)
    │   RunOptsBuilder::add_credentials(host_config_dir, agent) — no-ops when
    │   agent.auth() is None; for Claude, bind-mounts:
    │   ~/.ur/claude/.credentials.json
    │   → /home/worker/.claude/.credentials.json
    │   (Codex mounts ~/.ur/codex/auth.json → /home/worker/.codex/auth.json the
    │   same way — never the whole ~/.codex directory, which also holds sqlite
    │   state that must not be shared across concurrent containers.)
    │
    ▼  gRPC LaunchWorker RPC → builderd (host, native)
    │                         [BuilderContainerService::launch_worker]
    │   stats each volume source on host filesystem (host namespace — avoids
    │   container-to-host path mismatch)
    │   docker run (volume mount)
    │
Container: Claude Code reads ~/.claude/.credentials.json
            Claude Code reads ~/.claude.json (baked into image)
           Codex reads ~/.codex/auth.json
            Codex reads ~/.codex/config.toml (baked into image)
```

**Two files are required for Claude Code to skip login:**
- `~/.claude/.credentials.json` — OAuth tokens (bind-mounted from host, shared across all containers)
- `~/.claude.json` — App config with `hasCompletedOnboarding` and project trust (baked into the `agent-claude` image layer). This is a `COPY`, never a mount — a mount here would shadow the baked file and break the onboarding-skip, which is why `add_credentials` mounts only the credentials file.

**Session ownership:** Credentials are seeded from the host Claude Code installation (macOS Keychain or Linux credentials file) on `ur start` and on `ur worker launch` when the shared file is older than a day. Between re-seeds, containers own their token lifecycle — refreshes write back to the shared mount without touching the host credentials. The age check lets host re-logins propagate without clobbering fresh container-driven token refreshes on every launch. To force a re-seed without restarting, run `ur worker reseed-credentials`.

## Process Launch Sequence

```
ur worker launch <ticket-id> [-w <workspace>] [-a] [-f]

1. CLI (host)
   ├── -f flag? → kill_container() (docker stop + rm)
   ├── ensure_credentials_for_all_agents(max_age = 1 day)
   │   └── for each AgentType::ALL with an auth profile, re-seed from the host
   │       if ~/.ur/{agent.name()}/{credentials filename} is missing, empty, or
   │       older than max_age (Claude: macOS Keychain / Linux ~/.claude/.credentials.json;
   │       Codex: ~/.codex/auth.json on any OS)
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
       ├── resolve agent_type from the request (empty → claude) and from_toml
       │   mode resolution — see docs/codeflows/lifecycle-workflow.md
       ├── NetworkManager.ensure() (InspectNetwork RPC → builderd; create if needed)
       ├── Build env vars:
       │   ├── UR_SERVER_ADDR = <server_hostname>:<grpc_port>
       │   ├── UR_AGENT_TYPE = <agent.name()>
       │   ├── HTTP_PROXY / HTTPS_PROXY = http://ur-squid:3128
       │   └── NO_PROXY = ""
       ├── Build volumes (RunOptsBuilder):
       │   ├── workspace_dir → /workspace (if provided)
       │   └── ~/.ur/claude/.credentials.json → /home/worker/.claude/.credentials.json
       │       (add_credentials, no-op if the agent has no auth profile)
       ├── LaunchWorker RPC → builderd (host)
       │   ├── stats each volume source on host filesystem
       │   └── docker run (image: ur-worker:latest, network: worker network)
       └── Record ProcessEntry { container_id, grpc_port, server_handle }

3. Container startup (entrypoint.sh)
   ├── mkdir -p ~/.claude
   ├── Start tmux session "worker"
   └── exec sleep infinity
```

## Container Image Layers

```
containers/worker-base/Dockerfile (ur-worker-base:latest)
├── debian:bookworm-slim + system packages
├── useradd worker
├── COPY .agent-shared assets (potential-skills/, instructions/, shared-instructions/)
└── (no agent CLI installed here — agent-agnostic layer)

containers/agent-claude/Dockerfile (ur-worker:latest)
├── FROM ur-worker-base:latest
├── USER worker → install-claude.sh (binary at /home/worker/.local/bin/claude)
├── USER root → cleanup
├── COPY entrypoint.sh, worker binaries (ur-ping, git, gh, tk)
├── COPY claude.json → /home/worker/.claude.json (skip onboarding/login prompts)
├── COPY claude-settings.json → /home/worker/.claude/potential-settings.json
│   └── permissions.defaultMode: "bypassPermissions" (skip permissions for non-interactive use)
├── USER worker
└── ENTRYPOINT ["/entrypoint.sh"]
```

The Claude CLI install moved from the base image into the `agent-claude` layer (inverting the pre-split caching story — see `docs/codeflows/skill-loading.md` and `scripts/build/image.sh` for the `UR_FORCE_REBUILD_BASE`/`UR_UPDATE_CLAUDE` cache-busting behavior this implies). **Image tags are unchanged**: `ur-worker-base:latest`, `ur-worker:latest`, `ur-worker-rust:latest`.

`potential-settings.json` is baked here (agent-specific config, not shared content) and copied verbatim to `~/.claude/settings.json` by `InitSettingsManager` at container startup — permissions are bypassed via `settings.json` (`permissions.defaultMode: "bypassPermissions"`) rather than a CLI flag, so no wrapper script is needed.

## Environment Variables

| Variable | Set by | Consumed by | Purpose |
|---|---|---|---|
| `UR_SERVER_ADDR` | WorkerManager (server) | worker binaries (git, gh, ur-ping) | gRPC endpoint for proxied commands |
| `UR_AGENT_TYPE` | WorkerManager (server) | workerd (`AgentType::from_env()`) | Which agent this worker runs — resolves all agent-specific paths and the spawn command |
| `HTTP_PROXY` / `HTTPS_PROXY` | WorkerManager (server) | Claude Code, curl, etc. | Squid forward proxy |
| `GH_TOKEN` / `GITHUB_TOKEN` | docker-compose.yml | gh CLI (on server) | GitHub auth for server-side git/gh |

## Key Files

| File | Purpose |
|---|---|
| `crates/ur_config/src/agent.rs` | `AgentType`/`AgentAuth`/`AuthSource` — single source of truth for per-agent names, paths, and how each agent's credentials are sourced |
| `crates/ur/src/credential/mod.rs` | `AgentCredentialManager` trait, `credential_manager_for(agent)` factory, `ensure_credentials_for_all_agents`, shared file/container-read helpers |
| `crates/ur/src/credential/claude.rs` | `ClaudeCredentialManager` impl (Keychain on macOS, home-relative file fallback on Linux) |
| `crates/ur/src/credential/codex.rs` | `CodexCredentialManager` impl (always a home-relative host file, no keychain) |
| `crates/ur/src/main.rs` | CLI entry; `process_launch()` and `start_server()` call `ensure_credentials_for_all_agents()` |
| `crates/server/src/worker.rs` | WorkerManager: injects `UR_AGENT_TYPE`, launches containers |
| `crates/server/src/run_opts_builder.rs` | `add_credentials` — mounts the credentials file, no-op when `agent.auth()` is `None` |
| `crates/server/src/grpc.rs` | Server RPC handler, resolves `agent_type` from the request (empty → claude), maps to `WorkerConfig` |
| `containers/agent-claude/claude.json` | Baked-in `.claude.json` (onboarding + project trust) |
| `containers/agent-claude/entrypoint.sh` | Starts tmux, keeps container alive |
| `containers/agent-claude/claude-settings.json` | Baked-in settings (bypassPermissions mode), copied to `potential-settings.json` in the image |
| `containers/worker-base/Dockerfile` | Agent-agnostic base image (no Claude CLI) |
| `containers/agent-claude/Dockerfile` | Claude-specific layer: CLI install, worker binaries, config |

## Manual Credential Management

- `ur worker reseed-credentials [--agent claude|codex]` — force re-seed that agent's credentials file from the host (Claude: Keychain on macOS, `~/.claude/.credentials.json` on Linux; Codex: `~/.codex/auth.json` on any OS). Use after re-logging in on the host when you don't want to wait for the next `ur start` or 1-day age trigger. `--agent` defaults to `claude`; an unrecognized value is a descriptive error, not a panic. If the agent has never been logged into on this host, this fails loudly (unlike the `ensure_credentials_for_all_agents` skip-with-warning path, since a manual reseed request implies the caller expects a source to exist).
- `ur worker save-credentials <id> [--agent claude|codex]` — copy that agent's credential files from a running container to `~/.ur/{agent}/` (`.credentials.json` + `.claude.json` for Claude, `auth.json` only for Codex — its app config is baked into the image, not extracted). Useful for bootstrapping from a container login.
- Delete `~/.ur/claude/.credentials.json` (or `~/.ur/codex/auth.json`) to force re-seeding from host credentials on next launch.
