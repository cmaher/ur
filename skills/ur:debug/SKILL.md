---
name: ur:debug
description: Debugging playbook for the ur system — reading logs, inspecting workflow state, diagnosing stalled workers or flows, and using Docker as a last resort. Use when something is broken, stuck, or behaving unexpectedly.
---

# ur Debug Playbook

**Start with logs. Move to `ur` CLI commands. Use Docker only when those aren't enough.**

---

## Log Files

All logs are structured JSON (one object per line). Default location: `~/.ur/logs/` (override with `logs_dir` in `ur.toml`).

| File | Process | Notes |
|------|---------|-------|
| `~/.ur/logs/server.log.YYYY-MM-DD` | ur-server | Main server: gRPC handlers, worker lifecycle, workflow engine |
| `~/.ur/logs/builderd.log.YYYY-MM-DD` | builderd | Git operations (clone, fetch, reset) run on the host |
| `~/.ur/logs/builderd.err` | builderd | Stderr from builderd — check this first for startup failures |
| `~/.ur/logs/ur.log.YYYY-MM-DD` | ur CLI | Host-side CLI traces |
| `~/.ur/logs/workers/<worker_id>/workerd.log.YYYY-MM-DD` | workerd | Per-worker: init, skills, git hooks, Claude Code session |

Logs rotate daily, keeping 7 files per process.

### JSON Log Fields

```json
{
  "timestamp": "2026-04-29T01:10:01.210087Z",
  "level": "WARN",
  "fields": {
    "message": "...",
    "error": "...",
    "worker_id": "..."
  },
  "target": "ur_server::worker",
  "threadId": "ThreadId(42)"
}
```

Key fields: `level` (`TRACE`/`DEBUG`/`INFO`/`WARN`/`ERROR`), `fields.message`, `fields.error`, `target` (Rust module path).

### Reading Logs

Always start with today's file and grep for `ERROR` or `WARN`:

```bash
# All errors in today's server log
grep '"ERROR"' ~/.ur/logs/server.log.$(date +%Y-%m-%d) | jq '.fields'

# All warnings and errors
grep -E '"ERROR"|"WARN"' ~/.ur/logs/server.log.$(date +%Y-%m-%d) | jq '{level: .level, msg: .fields.message, err: .fields.error}'

# Specific worker's workerd log
grep '"ERROR"' ~/.ur/logs/workers/<worker_id>/workerd.log.$(date +%Y-%m-%d) | jq '.fields'

# Find a specific message across all logs
grep -r "pool acquire" ~/.ur/logs/ | grep '"ERROR"' | jq '.fields'

# Tail live (server restarts write new entries continuously)
tail -f ~/.ur/logs/server.log.$(date +%Y-%m-%d) | jq '{level: .level, msg: .fields.message, target: .target}'
```

### Finding a Worker's Logs

Worker ID format: `<project>-<hash>` (e.g., `ur-a1b2-x3y4`). The log directory name is the full internal worker ID:

```bash
ls ~/.ur/logs/workers/          # list all worker log dirs
ls ~/.ur/logs/workers/<id>/     # list log files for one worker
```

---

## Step 1: Check Workflow State

When a ticket's agent is stuck or something went wrong, start here.

```bash
ur flow show <ticket_id>         # full workflow state: status, lifecycle, stall_reason
ur flow list                     # all active workflows
ur flow list --status stalled    # only stalled workflows
```

`ur flow show` output includes:
- **lifecycle**: current phase (`implementing`, `pushing`, `in_review`, etc.)
- **stalled**: whether the workflow is stuck
- **stall_reason**: the error message that caused the stall — **this is the most useful field**

### Fixing a Stalled Flow

```bash
# Advance to the natural next state (most common fix)
ur flow redrive <ticket_id> --continue

# Jump to a specific lifecycle state
ur flow redrive <ticket_id> --to implementing

# Skip pre-push verification (useful when hooks are broken)
ur flow noverify <ticket_id>

# Pre-approve so GithubPoller auto-advances from in_review
ur flow autoapprove <ticket_id>

# Cancel the workflow entirely
ur flow cancel <ticket_id>
```

---

## Step 2: Check Worker State

```bash
ur worker list                   # all running workers with process IDs
ur worker describe               # detailed info for all workers
ur worker describe <worker_id>   # detailed info for one worker
```

`ur worker describe` shows: worker ID, ticket ID, process ID, container name, lifecycle, stall reason, branch.

**Container name** = `{worker_prefix}{process_id}` (default prefix: `ur-worker-`). You'll need this for Docker commands.

---

## Step 3: Read the Logs

### Server Log — Common Error Patterns

| Pattern to grep | Likely cause |
|-----------------|--------------|
| `"pool acquire"` + ERROR | Pool slot unavailable; check pool_limit |
| `"builderd"` + ERROR | Builderd not running or git op failed |
| `"workflow stalled"` | Workflow engine hit an error; check stall_reason via `ur flow show` |
| `"hostexec"` + ERROR | Worker called a disallowed command |
| `"WorkerLaunch"` + ERROR | Worker failed to start; check container.image, credentials |
| `"connection refused"` + `"postgres"` | Database unreachable |
| `"github"` + ERROR | GitHub API call failed (rate limit, auth, network) |

```bash
# Check for workflow stall messages
grep "stalled" ~/.ur/logs/server.log.$(date +%Y-%m-%d) | jq '{msg: .fields.message, reason: .fields.stall_reason, ticket: .fields.ticket_id}'

# Check for pool errors
grep "pool" ~/.ur/logs/server.log.$(date +%Y-%m-%d) | grep '"ERROR"' | jq '.fields'

# Check builderd connectivity
grep "builderd" ~/.ur/logs/server.log.$(date +%Y-%m-%d) | grep -E '"ERROR"|"WARN"' | jq '.fields'
```

### Workerd Log — Common Error Patterns

| Pattern to grep | Likely cause |
|-----------------|--------------|
| `"skill"` + ERROR/WARN | Skill not found in potential-skills |
| `"git hook"` + ERROR | git_hooks_dir path missing or wrong template |
| `"ListHostExecCommands"` + ERROR | Worker couldn't connect to server at startup |
| `"credentials"` | Claude Code credentials not injected; re-seed |

```bash
# Worker failed during init — check earliest lines
head -50 ~/.ur/logs/workers/<id>/workerd.log.$(date +%Y-%m-%d) | jq '{level: .level, msg: .fields.message}'

# Find skill loading issues
grep "skill" ~/.ur/logs/workers/<id>/workerd.log.$(date +%Y-%m-%d) | jq '.fields'
```

### Builderd Log

```bash
# Builderd startup and git op errors
grep -E '"ERROR"|"WARN"' ~/.ur/logs/builderd.log.$(date +%Y-%m-%d) | jq '.fields'

# Check builderd.err for startup crashes
cat ~/.ur/logs/builderd.err
```

---

## Step 4: Docker Inspection

Use Docker only after logs and `ur` CLI haven't explained the problem.

### Container Overview

```bash
docker ps                        # running containers (ur-server, ur-postgres, ur-squid, ur-worker-*)
docker ps -a                     # include stopped containers
```

### Infrastructure Container Logs

The ur-server process writes to the log file (not Docker stdout), so `docker logs ur-server` shows minimal output. Use the log file instead. Exception: if the server failed to start before logging was initialized.

```bash
# Only useful if server crashed before logging initialized
docker logs ur-server --tail 50

# Postgres container — useful for DB connection failures
docker logs ur-postgres --tail 50

# Squid — useful for network/proxy issues
docker logs ur-squid --tail 50
```

### Worker Container

Worker containers are named `{worker_prefix}{process_id}` (e.g., `ur-worker-abc123`). Get the process ID from `ur worker describe`.

```bash
# Check if a worker container is running
docker ps | grep ur-worker-<process_id>

# Inspect container state (exit code, restart count, OOM status)
docker inspect ur-worker-<process_id> | jq '.[0].State'

# Get container environment variables (useful to verify skill/model/logs injection)
docker inspect ur-worker-<process_id> | jq '.[0].Config.Env'

# Get volume mounts (verify workspace, logs, credentials, git hooks)
docker inspect ur-worker-<process_id> | jq '.[0].Mounts'
```

### Exec Into a Container

```bash
# Prefer ur worker attach for interactive sessions
ur worker attach <worker_id>

# Docker exec for non-interactive inspection
docker exec ur-worker-<process_id> ls /home/worker/.claude/skills/
docker exec ur-worker-<process_id> ls /home/worker/.claude/potential-skills/
docker exec ur-worker-<process_id> cat /home/worker/.claude/settings.json
docker exec ur-worker-<process_id> env | grep UR_
docker exec ur-worker-<process_id> ls /var/ur/

# Check tmux session inside worker
docker exec ur-worker-<process_id> tmux list-sessions
docker exec ur-worker-<process_id> tmux capture-pane -t agent -p
```

### Network Debugging

```bash
# Verify worker can reach server (run from inside worker container)
docker exec ur-worker-<process_id> curl -s http://ur-server:9120

# Verify squid proxy is reachable
docker exec ur-worker-<process_id> curl -s --proxy http://ur-squid:3128 https://api.anthropic.com

# Inspect network membership
docker inspect ur-worker-<process_id> | jq '.[0].NetworkSettings.Networks | keys'
```

---

## Common Scenarios

### Agent appears to have stopped / no activity

1. `ur flow show <ticket_id>` — check if stalled and read `stall_reason`
2. Check workerd log: `tail -f ~/.ur/logs/workers/<id>/workerd.log.$(date +%Y-%m-%d) | jq '.fields.message'`
3. `ur worker attach <worker_id>` to see the live tmux session
4. If stalled: `ur flow redrive <ticket_id> --continue`

### Worker failed to start

1. `ur flow show <ticket_id>` or `ur worker describe <worker_id>` for stall_reason
2. Check server log for `WorkerLaunch` errors
3. `docker inspect ur-worker-<process_id> | jq '.[0].State'` — look at exit code
4. Check credentials: `ur worker reseed-credentials`

### Flow stuck in `in_review`

1. `ur flow show <ticket_id>` — confirm lifecycle is `in_review`
2. Check GitHub CI status on the PR
3. If checks are passing but flow hasn't advanced: `ur flow redrive <ticket_id> --continue`
4. To auto-advance on next poller cycle: `ur flow autoapprove <ticket_id>`

### Pool exhausted / "no available slot"

1. `ur flow list` — see how many active flows are running
2. `ur worker list` — count running workers
3. Check `pool_limit` in `ur.toml` under `[projects.<key>]`
4. Stop idle workers: `ur worker stop <worker_id>`

### Skill not available in worker

1. Check workerd log for skill loading warnings: `grep "skill" ~/.ur/logs/workers/<id>/workerd.log.$(date +%Y-%m-%d) | jq '.fields'`
2. `docker exec ur-worker-<process_id> ls /home/worker/.claude/potential-skills/` — is the skill present?
3. Verify `[skills]` section in `ur.toml` — path must be visible to the **server process**, use `%URCONFIG%/...`
4. `docker exec ur-worker-<process_id> env | grep UR_WORKER_SKILLS` — confirm the skill was included in the launch

### Server won't start

1. `cat ~/.ur/logs/builderd.err` — builderd startup errors
2. `docker logs ur-postgres --tail 30` — database not ready?
3. `docker ps` — are infrastructure containers running?
4. `ur server restart` — attempt a clean restart
5. Check server log from the start of today's file: `head -50 ~/.ur/logs/server.log.$(date +%Y-%m-%d) | jq '{level: .level, msg: .fields.message}'`

### Push failing / pre-push hook errors

1. `ur flow show <ticket_id>` — check stall_reason for hook output
2. Check server log for workflow verify step errors
3. To skip verification: `ur flow noverify <ticket_id>` then redrive

$ARGUMENTS
