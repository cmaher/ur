# Ticket Tracker

Tickets are managed via `ur ticket`, which communicates with the ur-server over gRPC. All ticket data lives in the server's SQLite database.

**Always use `--output json`** on all `ur ticket` subcommands for machine-readable output.

## Quick Reference

### Creating Tickets

| Command | Purpose |
|---|---|
| `ur ticket create "title" --output json` | Create a task (default type) |
| `ur ticket create "title" --type design --output json` | Create a design ticket |
| `ur ticket create "title" --body "description" --output json` | With description |
| `ur ticket create "title" --parent <id> --output json` | As child of a parent ticket |

### Querying Tickets

| Command | Purpose |
|---|---|
| `ur ticket list --output json` | List all open tickets |
| `ur ticket list --status open --output json` | Filter by status: open, in_progress, closed |
| `ur ticket list --type bug --output json` | Filter by type |
| `ur ticket list --parent <id> --output json` | Filter by parent ticket |
| `ur ticket show <id> --output json` | Full detail on one ticket |
| `ur ticket dispatchable <parent-id> --output json` | Open children of a parent with no open blockers |
| `ur ticket status --output json` | Project status report (parent tree with open/closed counts) |
| `ur ticket status -p <project> --output json` | Status filtered by project key (e.g. `-p ur` for ur-* tickets) |

### Updating Tickets

| Command | Purpose |
|---|---|
| `ur ticket update <id> --status in_progress --output json` | Start work on a ticket |
| `ur ticket update <id> --status closed --output json` | Close a ticket |
| `ur ticket update <id> --status open --output json` | Reopen a ticket |
| `ur ticket update <id> --title "new title" --output json` | Change title |
| `ur ticket update <id> --priority 2 --output json` | Change priority |
| `ur ticket update <id> --parent <parent-id> --output json` | Re-parent under a different parent |
| `ur ticket update <id> --unparent --output json` | Remove from parent (clear parent) |
| `ur ticket add-activity <id> "text" --output json` | Append timestamped note |

### Dependencies & Links

| Command | Purpose |
|---|---|
| `ur ticket add-block <id> <blocker-id> --output json` | blocker-id blocks id |
| `ur ticket remove-block <id> <blocker-id> --output json` | Remove blocking dependency |
| `ur ticket add-link <id> <other-id> --output json` | Bidirectional link |
| `ur ticket remove-link <id> <other-id> --output json` | Remove link |

### Metadata

| Command | Purpose |
|---|---|
| `ur ticket set-meta <id> <key> <value> --output json` | Set metadata key-value pair |
| `ur ticket delete-meta <id> <key> --output json` | Delete metadata key |

## Guidelines

1. **Use `ur ticket dispatchable <parent> --output json`** when the user asks "what should I work on next" — it excludes blocked tickets.
2. **Add dependencies proactively** — if a ticket clearly depends on another, use `add-block`.
3. **Use full ticket IDs** — e.g., `ur-abc12`, not `abc12`.
4. **Ticket IDs are prefixed** — all IDs follow the `{project}-{hash}` convention (e.g., `ur-f49c`).
