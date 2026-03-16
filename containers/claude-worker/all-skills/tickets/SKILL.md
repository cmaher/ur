---
name: tickets
description: Use when creating, listing, updating, closing, or searching tickets/tasks/bugs/TODOs, managing dependencies between tickets, or when the user mentions tracking work items — uses ur ticket (gRPC)
---

# Ticket Tracker

Tickets are managed via `ur ticket`, which communicates with the ur-server over gRPC. All ticket data lives in the server's SQLite database.

## Quick Reference

### Creating Tickets

| Command | Purpose |
|---|---|
| `ur ticket create "title"` | Create a task (default type) |
| `ur ticket create "title" --type bug --priority 1` | Bug with high priority (0=critical, 4=backlog) |
| `ur ticket create "title" --body "description"` | With description |
| `ur ticket create "title" --parent <id>` | As child of a parent ticket |
| `ur ticket create "title" --type epic` | Create an epic |

### Querying Tickets

| Command | Purpose |
|---|---|
| `ur ticket list` | List all open tickets |
| `ur ticket list --status open` | Filter by status: open, in_progress, closed |
| `ur ticket list --type bug` | Filter by type |
| `ur ticket list --epic <id>` | Filter by parent epic |
| `ur ticket show <id>` | Full detail on one ticket |
| `ur ticket dispatchable <epic-id>` | Open children of an epic with no open blockers |
| `ur ticket status` | Project status report (epic tree with open/closed counts) |
| `ur ticket status -p <project>` | Status filtered by project key (e.g. `-p ur` for ur-* tickets) |

### Updating Tickets

| Command | Purpose |
|---|---|
| `ur ticket update <id> --status in_progress` | Start work on a ticket |
| `ur ticket update <id> --status closed` | Close a ticket |
| `ur ticket update <id> --status open` | Reopen a ticket |
| `ur ticket update <id> --title "new title"` | Change title |
| `ur ticket update <id> --priority 2` | Change priority |
| `ur ticket add-activity <id> "text"` | Append timestamped note |

### Dependencies & Links

| Command | Purpose |
|---|---|
| `ur ticket add-block <id> <blocker-id>` | blocker-id blocks id |
| `ur ticket remove-block <id> <blocker-id>` | Remove blocking dependency |
| `ur ticket add-link <id> <other-id>` | Bidirectional link |
| `ur ticket remove-link <id> <other-id>` | Remove link |

### Metadata

| Command | Purpose |
|---|---|
| `ur ticket set-meta <id> <key> <value>` | Set metadata key-value pair |
| `ur ticket delete-meta <id> <key>` | Delete metadata key |

## Guidelines

1. **Use `ur ticket dispatchable <epic>`** when the user asks "what should I work on next" — it excludes blocked tickets.
2. **Add dependencies proactively** — if a ticket clearly depends on another, use `add-block`.
3. **Use full ticket IDs** — e.g., `ur-abc12`, not `abc12`.
4. **Ticket IDs are prefixed** — all IDs follow the `{project}-{hash}` convention (e.g., `ur-f49c`).

$ARGUMENTS
