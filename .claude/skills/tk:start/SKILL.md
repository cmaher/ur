---
name: tk:start
description: Use when starting work on a ticket — claims the ticket and sets up context for work
---

# Start Working on a Ticket

Claim a ticket and begin working on it. This is the entry point for any agent picking up a task.

## Inputs

`$ARGUMENTS` should be a ticket ID (full or partial). If empty, pick the highest-priority ticket from `tk ready`.

## Workflow

### 1. Select a Ticket

If a ticket ID was provided:

```bash
tk show <id>
```

If no ID was provided:

```bash
tk ready
```

Pick the highest-priority unblocked ticket and confirm with the user before proceeding.

### 2. Claim the Ticket

```bash
tk start <full-id>
```

Use the **full prefixed ID** (e.g., `ic-038cd`, not `038cd`).

### 3. Check the Epic for Worktree

Look at the `parent:` field from the `tk show` output. If the ticket has a parent (epic):

```bash
tk show <parent-id>
```

Scan the epic's notes for a line matching `worktree: <path>, branch: <branch>`. If found, `cd` to that worktree path before doing any work.

If the ticket has no parent, skip this step.

### 4. Sync to Jira (if external ref exists)

Check the ticket's `external-ref` field from the `tk show` output. If it contains a Jira issue key (e.g., `ASC-123`), transition it to "In Progress" and assign it:

```bash
jira issue move <JIRA-KEY> "In Progress"
jira issue assign <JIRA-KEY> $(jira me)
```

Skip this step if there is no external ref or the ref is not a Jira key.

### 5. Report Ready

Tell the user:
- Which ticket you claimed (ID + title)
- Which worktree you're working in (if applicable)

Then begin working on the ticket.

## After Work is Done

1. Commit and push
2. Close the ticket: `tk close <full-id>`
3. If the ticket has a Jira external ref: `jira issue move <JIRA-KEY> "Done"`

$ARGUMENTS
