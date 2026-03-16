---
name: tk
description: Use when creating, listing, updating, closing, or searching tickets/tasks/bugs/TODOs, managing dependencies between tickets, or when the user mentions tracking work items — uses workertools ticket (gRPC)
---

# Ticket Tracker

Tickets are managed via `workertools ticket`, which communicates with the ur-server over gRPC. All ticket data lives in the server's SQLite database.

## Quick Reference

### Creating Tickets

| Command | Purpose |
|---|---|
| `workertools ticket create "title"` | Create a task (default type) |
| `workertools ticket create "title" --type bug --priority 1` | Bug with high priority (0=critical, 4=backlog) |
| `workertools ticket create "title" --body "description"` | With description |
| `workertools ticket create "title" --parent <id>` | As child of a parent ticket |
| `workertools ticket create "title" --type epic` | Create an epic |

### Querying Tickets

| Command | Purpose |
|---|---|
| `workertools ticket list` | List all open tickets |
| `workertools ticket list --status open` | Filter by status: open, in_progress, closed |
| `workertools ticket list --type bug` | Filter by type |
| `workertools ticket list --epic <id>` | Filter by parent epic |
| `workertools ticket show <id>` | Full detail on one ticket |
| `workertools ticket dispatchable <epic-id>` | Open children of an epic with no open blockers |
| `workertools ticket status` | Project status report (epic tree with open/closed counts) |
| `workertools ticket status -p <project>` | Status filtered by project key (e.g. `-p ur` for ur-* tickets) |

### Updating Tickets

| Command | Purpose |
|---|---|
| `workertools ticket update <id> --status in_progress` | Start work on a ticket |
| `workertools ticket update <id> --status closed` | Close a ticket |
| `workertools ticket update <id> --status open` | Reopen a ticket |
| `workertools ticket update <id> --title "new title"` | Change title |
| `workertools ticket update <id> --priority 2` | Change priority |
| `workertools ticket add-activity <id> "text"` | Append timestamped note |

### Dependencies & Links

| Command | Purpose |
|---|---|
| `workertools ticket add-block <id> <blocker-id>` | blocker-id blocks id |
| `workertools ticket remove-block <id> <blocker-id>` | Remove blocking dependency |
| `workertools ticket add-link <id> <other-id>` | Bidirectional link |
| `workertools ticket remove-link <id> <other-id>` | Remove link |

### Metadata

| Command | Purpose |
|---|---|
| `workertools ticket set-meta <id> <key> <value>` | Set metadata key-value pair |
| `workertools ticket delete-meta <id> <key>` | Delete metadata key |

## Guidelines

1. **Use `workertools ticket dispatchable <epic>`** when the user asks "what should I work on next" — it excludes blocked tickets.
2. **Add dependencies proactively** — if a ticket clearly depends on another, use `add-block`.
3. **Use full ticket IDs** — e.g., `ur-abc12`, not `abc12`.
4. **Ticket IDs are prefixed** — all IDs follow the `{project}-{hash}` convention (e.g., `ur-f49c`).

$ARGUMENTS
