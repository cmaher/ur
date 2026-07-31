---
name: ur:ticket
description: Use when creating, listing, updating, closing, or searching tickets in the ur project — uses `ur ticket` over gRPC. Covers all ticket operations including lifecycle transitions, dependencies, priorities, and epic workflows.
---

# ur Ticket System

Tickets are managed via `ur ticket`, which communicates with ur-server over gRPC. All ticket data lives in the server's Postgres database. IDs are project-prefixed (e.g., `ur-f49c`). Always use the full prefixed ID.

## Ticket Types

| Type | Alias | Purpose |
|------|-------|---------|
| `code` | `c` | Default — a unit of implementation work |
| `task` | | Non-code task |
| `design` | `d` | Design/spike work |
| `epic` | | Parent container for a set of related tickets |

## Priority

`0` = critical, `1` = high, `2` = medium, `3` = low, `4` = backlog. Default is `0`.

## Status

`open` → `in_progress` → `closed`

## Lifecycle

Distinct from status. Tracks workflow phase: `design`, `open`, `implementing`, `pushing`, `in_review`, `addressing_feedback`. Set with `--lifecycle` on update. The `approve` command transitions `in_review` → `addressing_feedback`.

---

## Command Reference

### Creating Tickets

```bash
ur ticket create "title"                                 # code ticket, priority 0
ur ticket create "title" --type epic                     # epic
ur ticket create "title" --type design                   # design ticket
ur ticket create "title" --parent ur-abc1                # child of an epic
ur ticket create "title" --body "description"            # with body text
ur ticket create "title" --priority 2                    # medium priority
ur ticket create "title" --branch my-branch              # associate a branch
ur ticket create "title" -d                              # WIP (lifecycle=design)
ur ticket create "title" -p other-project                # in a different project
```

### Querying Tickets

```bash
ur ticket list                                           # open tickets in current project
ur ticket list --status in_progress                      # filter by status
ur ticket list --type epic                               # filter by type
ur ticket list -p ur                                     # filter by project key
ur ticket list --all                                     # all projects
ur ticket list -t ur-abc1                                # tree view of ticket + descendants
ur ticket list --meta ref=PROJ-321                       # by metadata value
ur ticket list --meta pr_number                          # any ticket carrying the key
ur ticket show ur-abc1                                   # full detail on one ticket
ur ticket show ur-abc1 --activity-author workflow        # filter activities by author
ur ticket dispatchable ur-abc1                           # open children with no open blockers
```

### Finding a ticket by external ref

When a ticket is synced to an external issue tracker, the tracker's key is stored in the `ref`
metadata key. `ur ticket show` accepts that value wherever it accepts an ID:

```bash
ur ticket show PROJ-321                                  # ticket carrying ref=PROJ-321
```

An exact ticket ID always wins — only a miss falls through to a ref lookup, so this can never
change the meaning of a valid ID.

**Refs are not unique.** Several ur tickets can carry the same tracker key. When more than one
matches, `show` refuses and lists the candidates instead of guessing:

```
$ ur ticket show PROJ-322
error: ref 'PROJ-322' matches 3 tickets:
  myproj-a1b2  open         Add retry backoff
  myproj-c3d4  closed       Add retry backoff (dupe)
  myproj-e5f6  in_progress  Backoff config schema
re-run with a ticket ID, or: ur ticket list --meta ref=PROJ-322
```

Re-run with one of the listed IDs. Do not work around this by taking the first candidate —
which one is correct is a judgement call.

`--meta` searches across all projects by default (a metadata lookup is a search, not a browse),
so it needs no `--all`. Combine with `-p`, `--status`, or `--type` to narrow it. Beyond `ref`,
common keys include `pr_number`, `pr_url`, and `gh_repo`.

### Updating Tickets

```bash
ur ticket update ur-abc1 --status in_progress            # start work
ur ticket update ur-abc1 --status closed                 # close
ur ticket close ur-abc1                                  # sugar for --status closed
ur ticket open ur-abc1                                   # reopen
ur ticket update ur-abc1 --title "new title"
ur ticket update ur-abc1 --body "new body"
ur ticket update ur-abc1 --priority 1
ur ticket update ur-abc1 --type task
ur ticket update ur-abc1 --parent ur-xyz9                # re-parent
ur ticket update ur-abc1 --unparent                      # remove from epic
ur ticket update ur-abc1 --branch my-branch
ur ticket update ur-abc1 --no-branch                     # clear branch
ur ticket update ur-abc1 --lifecycle implementing        # advance lifecycle
ur ticket update ur-abc1 -p other-project                # change project
ur ticket update ur-abc1 --force                         # force (e.g. close epic with open children)
ur ticket approve ur-abc1                                # in_review → addressing_feedback
```

### Dependencies & Links

```bash
ur ticket add-block ur-abc1 ur-xyz9                      # ur-xyz9 BLOCKS ur-abc1
ur ticket remove-block ur-abc1 ur-xyz9
ur ticket add-link ur-abc1 ur-xyz9                       # bidirectional soft link
ur ticket remove-link ur-abc1 ur-xyz9
```

### Activities & Metadata

```bash
ur ticket add-activity ur-abc1 "deployed to staging"     # timestamped note
ur ticket list-activities ur-abc1
ur ticket set-meta ur-abc1 pr_url "https://..."          # key-value metadata
ur ticket delete-meta ur-abc1 pr_url
```

---

## Common Workflows

### Create an Epic with Children

```bash
ur ticket create "Epic title" --type epic                # note returned ID, e.g. ur-abc1
ur ticket create "Subtask one" --parent ur-abc1
ur ticket create "Subtask two" --parent ur-abc1
ur ticket add-block ur-def2 ur-abc3                      # if subtask two blocks subtask three
```

### Find What to Work on Next (from an Epic)

```bash
ur ticket dispatchable ur-abc1                           # open children with no open blockers
```

### Start and Close a Ticket

```bash
ur ticket update ur-abc1 --status in_progress
# ... do the work ...
ur ticket close ur-abc1
```

### Track a PR

```bash
ur ticket update ur-abc1 --branch my-branch-name
ur ticket set-meta ur-abc1 pr_url "https://github.com/..."
ur ticket update ur-abc1 --lifecycle in_review
ur ticket approve ur-abc1                                # once PR is reviewed/approved
```

---

## Guidelines

- **Always use full prefixed IDs** — `ur-abc1`, not `abc1`.
- **`dispatchable` before dispatching subagents** — it filters out blocked tickets; don't skip it.
- **Add blocks proactively** — if a ticket clearly depends on another, wire the dependency with `add-block`.
- **Lifecycle vs status** — status is the coarse work state (open/in_progress/closed); lifecycle tracks the finer-grained workflow phase. Update lifecycle when the work enters a new phase (e.g. code is pushed, PR is open).
- **`--force` sparingly** — closing an epic with open children should be intentional.

$ARGUMENTS
