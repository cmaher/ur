---
name: deploy
description: Use when building, redeploying, or testing individual ur components — hostd, server, worker, or host CLI
---

# Build / Redeploy / Test Individual Components

Use this when you need to rebuild and redeploy a single component without rebuilding everything. Each component has its own script and verification step.

## Component Map

| Component | Runs on | Crate(s) | Deploy Script |
|-----------|---------|----------|---------------|
| **ur-hostd** | Host (native) | `crates/hostd/` | `scripts/deploy/hostd.sh` |
| **ur-server** | Container (Alpine) | `crates/server/` | `scripts/deploy/server.sh` |
| **Worker** (ur-tools, ur-workerd, ur-ping) | Container (Debian) | `crates/workercmd/` | `scripts/deploy/worker.sh [PROCESS_ID]` |
| **Host CLI** (ur, ur-hostd) | Host (native) | `crates/ur/`, `crates/hostd/` | `scripts/deploy/host-cli.sh` |
| **Full rebuild** | All | All | `cargo make install` |

## Workflow

### 1. Identify the Component

Determine which component needs rebuilding based on what code changed:

- `crates/ur/`, `crates/hostd/` → **Host CLI** (runs natively, no container rebuild)
- `crates/server/`, `crates/ur_config/` → **Server** (needs cross-compile + image rebuild)
- `crates/workercmd/` → **Worker** (needs cross-compile + image rebuild + relaunch)
- `crates/ur_rpc/` (proto changes) → Rebuild **all affected** consumers
- `crates/container/` → Rebuild **server** (it manages containers)
- `containers/claude-worker/`, `containers/claude-worker-rust/` → **Worker** image only
- `containers/server/` → **Server** image only

### 2. Build & Deploy

Run the appropriate deploy script from the workspace root:

```bash
# Host CLI only (fastest — no containers)
scripts/deploy/host-cli.sh

# ur-hostd (rebuild + restart daemon)
scripts/deploy/hostd.sh

# Server container (cross-compile + image + recreate container)
scripts/deploy/server.sh

# Worker container (cross-compile + image + kill/relaunch worker)
scripts/deploy/worker.sh [PROCESS_ID]
```

### 3. Verify

After deploying, verify the component works:

**Host CLI / ur-hostd:**
```bash
lsof -i :42070          # hostd is listening
target/debug/ur start   # start works
```

**Server:**
```bash
docker logs --tail 10 ur-server   # no errors
docker exec ur-agent-ur ur-ping   # worker can reach server
```

**Worker (git via hostexec):**
```bash
docker exec ur-agent-<ID> git status    # git proxied through hostexec pipeline
docker exec ur-agent-<ID> git log -1    # verify it hits the right host repo
```

**Full stack:**
```bash
cargo make acceptance   # runs full E2E test suite
```

### 4. Common Issues

- **"hostd unavailable: transport error"** → ur-hostd is not running. Run `scripts/deploy/hostd.sh` or `ur start`.
- **"failed to connect to ur server"** → Worker is on a stale network or server was recreated. Kill and relaunch the worker: `ur process kill <ID>` then `ur process launch <ID>`.
- **Worker has stale binaries** → The worker image wasn't rebuilt. Run `scripts/deploy/worker.sh <ID>`.
- **Server has stale code** → Server image wasn't rebuilt. Run `scripts/deploy/server.sh`.

$ARGUMENTS
