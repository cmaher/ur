---
name: implement
description: Use when implementing tickets — single ticket or epic. For a single ticket, work directly. For an epic or multiple tickets, dispatch one subagent per ticket with minimal context return.
---

# Ticket Agent Dispatch

Execute tickets with subagents. Sequential by default — one ticket per agent, minimal reporting back. Keeps the parent context window small.

**Core principle:** The parent orchestrates via `ur ticket`; subagents do the work. Only essential outcomes flow back.

## Ticket Commands Quick Reference

```bash
# Query
ur ticket show <id>                              # full detail on one ticket
ur ticket dispatchable <epic-id>                 # open children with no open blockers
ur ticket list --status open                     # list by status (open, in_progress, closed)
ur ticket status -p <project>                    # epic tree with open/closed counts

# Update
ur ticket update <id> --status in_progress       # claim a ticket
ur ticket update <id> --status closed             # close a ticket
ur ticket add-activity <id> "text"                # append timestamped note

# Create
ur ticket create "title" --parent <epic-id>       # task under an epic
ur ticket create "title" --type epic --body "..."  # new epic

# Dependencies
ur ticket add-block <id> <blocker-id>             # blocker-id blocks id
ur ticket add-link <id> <other-id>                # bidirectional link
```

## Before Starting — Branch Setup

Ensure you have the latest remote code and a working branch for the epic:

1. `git fetch origin` — pull latest from remote
2. Verify with `git log --oneline origin/master -5` — confirm master is up to date
3. **Create the working branch:**
   ```bash
   git checkout -B <branch-name> origin/master
   ```
4. **Record the branch on the epic ticket:**
   ```bash
   ur ticket add-activity <epic-id> "branch: <branch-name>"
   ```

## VCS: Sequential Stacking (Critical)

Agents stack commits sequentially on the working branch. Each agent commits, and the next agent inherits all previous work.

```
origin/master -> agent1 commits -> agent2 commits -> ...
```

- **Sequential**: Agents stack automatically via `git commit`. No extra VCS commands needed between agents.
- **Parallel**: Agents work in the same worktree. Coordinate to avoid editing the same files.

## The Loop

```dot
digraph tickets_agents {
    "Fresh from master" [shape=doublecircle];
    "Query dispatchable" [shape=box];
    "Pick next ticket" [shape=box];
    "Show ticket for context" [shape=box];
    "Dispatch subagent" [shape=box];
    "Record 1-2 line summary" [shape=box];
    "More dispatchable tickets?" [shape=diamond];
    "Report final summary" [shape=doublecircle];

    "Fresh from master" -> "Query dispatchable";
    "Query dispatchable" -> "Pick next ticket";
    "Pick next ticket" -> "Show ticket for context";
    "Show ticket for context" -> "Dispatch subagent";
    "Dispatch subagent" -> "Record 1-2 line summary";
    "Record 1-2 line summary" -> "More dispatchable tickets?";
    "More dispatchable tickets?" -> "Query dispatchable" [label="yes — re-query\nfor newly unblocked"];
    "More dispatchable tickets?" -> "Run full CI" [label="no"];
    "Run full CI" -> "Report final summary";
}
```

## Testing Strategy

- **Subagents**: Run only the minimum tests needed to validate their change (check CLAUDE.md for project-specific commands)
- **Parent (after all issues done)**: Run full CI and fix any integration issues

## Sequential (Default)

- Re-query `ur ticket dispatchable <epic-id>` each iteration — newly unblocked tickets surface naturally
- Pass only 1-2 sentence summaries between tasks
- Parent never reads files or explores code inline — if it takes more than a glance, delegate

## Parallel Mode

Use only when explicitly requested or when tickets are clearly independent:

1. Each agent claims with `ur ticket update <id> --status in_progress`
2. Dispatch via `superpowers:dispatching-parallel-agents` pattern
3. Each agent closes when done: `ur ticket update <id> --status closed`

## Subagent Prompt Template

```
Work on ticket <id>: "<title>"

1. Claim: `ur ticket update <id> --status in_progress`
2. Read the ticket: `ur ticket show <id>`

[If relevant: "Previous ticket accomplished: <1-2 sentences>"]

Constraints:
- [Scope boundaries]
- [What NOT to change]

VCS:
- Use `git add <files> && git commit -m "message"` when done — do NOT switch branches
- The parent agent manages branching and pushing

Testing:
- Run only the minimum tests needed to validate YOUR change — not the full CI suite
- The parent agent will run full CI after all issues are done

When done:
1. Close the ticket: `ur ticket update <id> --status closed`
2. Do NOT add ticket IDs to commit messages
3. Return ONLY a 1-2 sentence summary of what you did and any key values/paths the next task might need
```

## Common Mistakes

| Mistake | Fix |
|---------|-----|
| Switching branches mid-work | **Never.** Stack via `git commit` on the working branch — next agent inherits automatically |
| Parent reads full subagent output | Ask for "1-2 sentence summary" in every prompt |
| Parent explores code inline | Delegate to subagent |
| Subagent skips teardown | Must close ticket with `ur ticket update <id> --status closed` |
| Re-query skipped after completion | Always `ur ticket dispatchable <epic>` again — deps may have unblocked |
| Parallel without claiming | Two agents grab same ticket — always claim with `--status in_progress` first |
