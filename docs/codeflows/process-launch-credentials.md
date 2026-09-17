# Process Launch & Credential Injection

How `ur process launch` starts a worker container with agent credentials.

## Credential Flow

```
Claude — macOS: Keychain ("Claude Code-credentials")
         Linux: ~/.claude/.credentials.json
Codex  — any OS: ~/.codex/auth.json (no keychain integration)
AGY    — macOS: Keychain (service "gemini", account "antigravity")
         Linux: Secret Service, falling back to the native token cache file
    │
    ▼
ur CLI: credential_manager_for(agent) -> Option<Box<dyn AgentCredentialManager>>
    │                                       [crates/ur/src/credential/mod.rs]
    │   None for a no-auth agent (agent.auth() is None) — every caller acknowledges
    │   the no-auth path instead of the trait silently no-op'ing.
    │   Every manager implements ensure_credentials(max_age): re-seeds
    │   if ~/.ur/{agent.name()}/{filename component of auth.credentials_path} is
    │   missing, empty, or older than max_age:
    │       Claude, macOS: runs: security find-generic-password -s "<service>" -w
    │       Claude, Linux: reads ~/.claude/.credentials.json directly
    │       Codex, any OS: reads ~/.codex/auth.json directly
    │       writes result to ~/.ur/{agent}/{credentials file}
    │   AGY: reads the host OS keyring (or Linux file fallback) and writes the
    │       payload to ~/.ur/agy/antigravity-oauth-token. If the host has no AGY
    │       login, ur init's empty mode-600 file remains a valid interactive
    │       bootstrap cache that AGY can populate in-container.
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
    │   same way — never the whole ~/.codex directory. Codex keeps a sqlite
    │   database under ~/.codex for session/memory state, and sqlite's WAL
    │   mode assumes a single writer process holds the file's WAL/SHM
    │   sidecar files; bind-mounting the whole directory into N concurrent
    │   containers would let N processes open the same WAL simultaneously,
    │   which is exactly the corruption scenario WAL mode does not tolerate.
    │   Mounting only auth.json avoids sharing that file at all.)
    │   AGY similarly mounts exactly one read-write file:
    │   ~/.ur/agy/antigravity-oauth-token
    │   → /home/worker/.gemini/antigravity-cli/antigravity-oauth-token.
    │   Never mount ~/.gemini: it also contains SQLite conversations, logs,
    │   updater state, and installation identity that must remain per-worker.
    │   A missing cache is created through the server-visible config mount
    │   (`local_config_dir`) at mode 600, while the volume request retains the
    │   corresponding host path (`host_config_dir`) for builderd/Docker.
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
           AGY reads/writes ~/.gemini/antigravity-cli/antigravity-oauth-token
```

**Two files are required for Claude Code to skip login:**
- `~/.claude/.credentials.json` — OAuth tokens (bind-mounted from host, shared across all containers)
- `~/.claude.json` — App config with `hasCompletedOnboarding` and project trust (baked into the `worker-claude` image layer). This is a `COPY`, never a mount — a mount here would shadow the baked file and break the onboarding-skip, which is why `add_credentials` mounts only the credentials file.

**Session ownership:** Credentials are seeded from the host Claude Code installation (macOS Keychain or Linux credentials file) on `ur start` and on `ur worker launch` when the shared file is older than a day. Between re-seeds, containers own their token lifecycle — refreshes write back to the shared mount without touching the host credentials. The age check lets host re-logins propagate without clobbering fresh container-driven token refreshes on every launch. To force a re-seed without restarting, run `ur worker reseed-credentials`.

**AGY bootstrap is intentionally permissive:** ur first tries to seed the host AGY login from
macOS Keychain or Linux Secret Service/file storage. The pre-launch credential gate still
permits an empty token file, so users without a host login can complete OAuth in the pane and
write the renewable bundle in place. `reseed-credentials --agent agy` forces another host read;
`save-credentials` remains unnecessary because the token file is already a read-write mount.

## Process Launch Sequence

```
ur worker launch <ticket-id> [-w <workspace>] [-a] [-f]

1. CLI (host)
   ├── -f flag? → kill_container() (docker stop + rm)
   ├── ensure_credentials_for_all_agents(max_age = 1 day)
   │   └── for each AgentType::ALL with an auth profile, re-seed from the host
   │       if ~/.ur/{agent.name()}/{credentials filename} is missing, empty, or
   │       older than max_age (Claude: macOS Keychain / Linux file; Codex: host file;
   │       AGY: macOS Keychain / Linux Secret Service or file fallback)
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
       ├── resolve agent_type from the request (empty → mode's agent, then the
       │   top-level default) and from_toml mode resolution — see
       │   docs/codeflows/lifecycle-workflow.md and #agent-default-precedence
       │   in docs/codeflows/config.md
       ├── NetworkManager.ensure() (InspectNetwork RPC → builderd; create if needed)
       ├── Build env vars:
       │   ├── UR_SERVER_ADDR = <server_hostname>:<grpc_port>
       │   ├── UR_AGENT_TYPE = <agent.name()>
       │   ├── HTTP_PROXY / HTTPS_PROXY = http://ur-squid:3128
       │   └── NO_PROXY = ""
       ├── Build volumes (RunOptsBuilder):
       │   ├── workspace_dir → /workspace (if provided)
       │   └── one agent credential file → its container credential path
       │       (AGY's empty in-container cache is valid and mounted read-write)
       ├── LaunchWorker RPC → builderd (host)
       │   ├── stats each volume source on host filesystem
       │   └── docker run (image: ur-worker-claude:latest, network: worker network)
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

containers/worker-claude/Dockerfile (ur-worker-claude:latest)
├── FROM ur-worker-base:latest
├── USER worker → install-claude.sh (binary at /home/worker/.local/bin/claude)
├── USER root → cleanup
├── COPY entrypoint.sh, worker binaries (ur-ping, git, gh, tk)
├── COPY claude.json → /home/worker/.claude.json (skip onboarding/login prompts)
├── COPY claude-settings.json → /home/worker/.claude/potential-settings.json
│   └── permissions.defaultMode: "bypassPermissions" (skip permissions for non-interactive use)
├── USER worker
└── ENTRYPOINT ["/entrypoint.sh"]

containers/worker-agy/Dockerfile (ur-worker-agy:latest)
├── FROM ur-worker-base:latest
├── USER worker → vendored AGY installer (run with bash)
├── COPY onboarding/settings runtime files under ~/.gemini/antigravity-cli/
├── COPY trusted Stop hook → ~/.gemini/config/hooks.json
├── pre-create both trees with worker ownership
└── ENTRYPOINT ["/entrypoint.sh"]
```

The Claude CLI install moved from the base image into the `worker-claude` layer (inverting the pre-split caching story — see `docs/codeflows/skill-loading.md` and `scripts/build/image.sh` for the `UR_FORCE_REBUILD_BASE`/`UR_UPDATE_AGENT` cache-busting behavior this implies). Image tags include `ur-worker-base:latest` and `ur-worker-claude:latest`; each directory name matches its tag.

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
| `crates/ur/src/credential/agy.rs` | `AgyCredentialManager` impl (host OS-keyring seeding plus writable in-container OAuth fallback) |
| `crates/ur/src/main.rs` | CLI entry; `process_launch()` and `start_server()` call `ensure_credentials_for_all_agents()` |
| `crates/server/src/worker.rs` | WorkerManager: injects `UR_AGENT_TYPE`, launches containers |
| `crates/server/src/run_opts_builder.rs` | `add_credentials` — mounts the credentials file, no-op when `agent.auth()` is `None` |
| `crates/server/src/grpc.rs` | Server RPC handler, resolves `agent_type` from the request (empty → mode's agent, then the top-level default), maps to `WorkerConfig` |
| `containers/worker-claude/claude.json` | Baked-in `.claude.json` (onboarding + project trust) |
| `containers/worker-claude/entrypoint.sh` | Starts tmux, keeps container alive |
| `containers/worker-claude/claude-settings.json` | Baked-in settings (bypassPermissions mode), copied to `potential-settings.json` in the image |
| `containers/worker-base/Dockerfile` | Agent-agnostic base image (no Claude CLI) |
| `containers/worker-claude/Dockerfile` | Claude-specific layer: CLI install, worker binaries, config |

## Manual Credential Management

- `ur worker reseed-credentials [--agent claude|codex|agy]` — force re-seed credentials from the host (Claude: Keychain on macOS, `~/.claude/.credentials.json` on Linux; Codex: `~/.codex/auth.json`; AGY: the `gemini`/`antigravity` OS-keyring entry or Linux token-file fallback). `--agent` defaults to the top-level `agent` key in `ur.toml` (claude when omitted).
- `ur worker save-credentials <id> [--agent claude|codex|agy]` — copy a host-sourced agent's credential files from a running container. AGY needs no save operation because its token is already a read-write host bind mount.

  **Host-sourced agents are not normally from-nothing bootstraps.** `check_credentials_seeded` rejects a Claude or Codex launch whose host credential is absent. AGY is the exception: even though it can seed from the host, the gate permits its empty cache so the pane can perform first sign-in.
- Delete an agent's shared file under `~/.ur/<agent>/` to force host re-seeding on next launch. For AGY, leave it in place if there is no host login: it may contain the only worker-created renewable token bundle.
