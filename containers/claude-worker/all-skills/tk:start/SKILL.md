---
name: tk:start
description: Use when starting work on a ticket — claims the ticket and sets up context for work
---

# Start Working on a Ticket

Claim a ticket and begin working on it. This is the entry point for any agent picking up a task.

## Inputs

`$ARGUMENTS` should be a ticket ID (full or partial). If empty, pick the highest-priority ticket from `workertools ticket dispatchable <epic-id>`.

## Workflow

### 1. Select a Ticket

If a ticket ID was provided:

```bash
workertools ticket show <id>
```

If no ID was provided, ask the user which epic to check, then:

```bash
workertools ticket dispatchable <epic-id>
```

Pick the highest-priority unblocked ticket and confirm with the user before proceeding.

### 2. Claim the Ticket

```bash
workertools ticket update <full-id> --status in_progress
```

Use the **full prefixed ID** (e.g., `ur-038cd`, not `038cd`).

### 3. Check the Epic for Worktree

Look at the `Parent:` field from the `workertools ticket show` output. If the ticket has a parent (epic):

```bash
workertools ticket show <parent-id>
```

Scan the epic's activities for a line matching `worktree: <path>, branch: <branch>`. If found, `cd` to that worktree path before doing any work.

If the ticket has no parent, skip this step.

### 4. Report Ready

Tell the user:
- Which ticket you claimed (ID + title)
- Which worktree you're working in (if applicable)

Then begin working on the ticket.

## After Work is Done

1. Commit and push
2. Close the ticket: `workertools ticket update <full-id> --status closed`

$ARGUMENTS
