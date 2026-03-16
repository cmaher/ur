---
name: tickets
description: Use when creating, listing, updating, closing, or searching tickets/tasks/bugs/TODOs, managing dependencies between tickets, or when the user mentions tracking work items — uses workertools ticket (gRPC)
---

# Ticket Tracker

Tickets are managed via `workertools ticket --output json`, which communicates with the ur-server over gRPC. All ticket data lives in the server's SQLite database.

**Always use `--output json`** for machine-readable structured output. JSON responses are wrapped in `{"ok":true,"data":...}` on success or `{"ok":false,"error":...}` on failure.

## Quick Reference

### Creating Tickets

| Command | Purpose |
|---|---|
| `workertools ticket create "title" --output json` | Create a task (default type) |
| `workertools ticket create "title" --type bug --priority 1 --output json` | Bug with high priority (0=critical, 4=backlog) |
| `workertools ticket create "title" --body "description" --output json` | With description |
| `workertools ticket create "title" --parent <id> --output json` | As child of a parent ticket |
| `workertools ticket create "title" --type epic --output json` | Create an epic |

### Querying Tickets

| Command | Purpose |
|---|---|
| `workertools ticket list --output json` | List all open tickets |
| `workertools ticket list --status open --output json` | Filter by status: open, in_progress, closed |
| `workertools ticket list --type bug --output json` | Filter by type |
| `workertools ticket list --epic <id> --output json` | Filter by parent epic |
| `workertools ticket show <id> --output json` | Full detail on one ticket |
| `workertools ticket dispatchable <epic-id> --output json` | Open children of an epic with no open blockers |
| `workertools ticket status --output json` | Project status report (epic tree with open/closed counts) |
| `workertools ticket status -p <project> --output json` | Status filtered by project key (e.g. `-p ur` for ur-* tickets) |

### Updating Tickets

| Command | Purpose |
|---|---|
| `workertools ticket update <id> --status in_progress --output json` | Start work on a ticket |
| `workertools ticket update <id> --status closed --output json` | Close a ticket |
| `workertools ticket update <id> --status open --output json` | Reopen a ticket |
| `workertools ticket update <id> --title "new title" --output json` | Change title |
| `workertools ticket update <id> --priority 2 --output json` | Change priority |
| `workertools ticket add-activity <id> "text" --output json` | Append timestamped note |

### Dependencies & Links

| Command | Purpose |
|---|---|
| `workertools ticket add-block <id> <blocker-id> --output json` | blocker-id blocks id |
| `workertools ticket remove-block <id> <blocker-id> --output json` | Remove blocking dependency |
| `workertools ticket add-link <id> <other-id> --output json` | Bidirectional link |
| `workertools ticket remove-link <id> <other-id> --output json` | Remove link |

### Metadata

| Command | Purpose |
|---|---|
| `workertools ticket set-meta <id> <key> <value> --output json` | Set metadata key-value pair |
| `workertools ticket delete-meta <id> <key> --output json` | Delete metadata key |

## Guidelines

1. **Use `workertools ticket dispatchable <epic> --output json`** when the user asks "what should I work on next" — it excludes blocked tickets.
2. **Add dependencies proactively** — if a ticket clearly depends on another, use `add-block`.
3. **Use full ticket IDs** — e.g., `ur-abc12`, not `abc12`.
4. **Ticket IDs are prefixed** — all IDs follow the `{project}-{hash}` convention (e.g., `ur-f49c`).

$ARGUMENTS
