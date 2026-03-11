---
name: tk
description: Use when creating, listing, updating, closing, or searching tickets/tasks/bugs/TODOs, managing dependencies between tickets, or when the user mentions tracking work items — uses the tk CLI ticket tracker
---

# Ticket Tracker

Tickets (`tk`) is a file-based ticket tracker that lives in `.tickets/` inside a repository. Tickets are markdown files with YAML frontmatter. Use it for lightweight issue management without leaving the terminal.

## Pre-check

Before using any `tk` command, verify the project has tickets initialized:

```bash
ls .tickets/
```

If not present, `tk create` will initialize it automatically.

## Quick Reference

### Creating Tickets

| Command | Purpose |
|---|---|
| `tk create "title"` | Create a ticket |
| `tk create "title" -t bug -p 1` | Bug with high priority (0=critical, 4=backlog) |
| `tk create "title" -d "description" --tags "tag1,tag2"` | With description and tags |
| `tk create "title" --parent <id>` | As child of a parent ticket |

### Querying Tickets

| Command | Purpose |
|---|---|
| `tk ready` | Work ready to claim (open + no unresolved deps). **Prefer this over `tk list`.** |
| `tk list` | List all open tickets |
| `tk list --status=in_progress` | Filter by status: open, in_progress, closed |
| `tk list -T bug` | Filter by type |
| `tk list -a "name"` | Filter by assignee |
| `tk list --tags "tag"` | Filter by tag |
| `tk show <id>` | Full detail on one ticket (supports partial ID matching) |
| `tk blocked` | All blocked tickets |
| `tk closed` | Recently closed tickets |
| `tk query` | Output all tickets as JSON |
| `tk query '.[] \| select(.status=="open")'` | Query with jq filter |

### Updating Tickets

| Command | Purpose |
|---|---|
| `tk start <id>` | Set status to in_progress |
| `tk close <id>` | Close a ticket |
| `tk reopen <id>` | Reopen a closed ticket |
| `tk add-note <id> "text"` | Append timestamped note |
| `tk edit <id>` | Open ticket in $EDITOR (interactive — avoid in agents) |

### Dependencies

| Command | Purpose |
|---|---|
| `tk dep <id> <depends-on>` | id depends on depends-on |
| `tk dep tree <id>` | Dependency tree |
| `tk dep cycle` | Detect dependency cycles |

## Guidelines

1. **Use `tk ready`** when the user asks "what should I work on next" — it excludes blocked tickets.
2. **Add dependencies proactively** — if a ticket clearly depends on another, use `tk dep`.
3. **Never use `tk edit`** — it opens an interactive editor. Use `tk add-note` for updates instead.
4. **Use partial IDs** — `tk show 5c4` matches any ticket containing "5c4" in its ID.
5. **Tickets are plain markdown** — no database sync needed. Changes are immediately visible and committable.
6. **Use `tk query`** for programmatic access — pipe through `jq` for filtering.

$ARGUMENTS
