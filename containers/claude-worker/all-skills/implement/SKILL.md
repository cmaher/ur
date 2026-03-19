---
name: implement
description: Use when implementing a ticket, epic, or set of tickets — dispatches regularly for a single ticket, uses subagents for epics or multiple tickets
---

# Implement Tickets

Implement one or more tickets. For a **single ticket**, work on it directly. For an **epic or multiple tickets**, dispatch subagents — one per ticket, sequential by default.

**If the ticket description and codebase do not provide enough context to implement confidently, run `workertools agent request-human "<what context is missing>"` and stop.** Do not guess or make assumptions about unclear requirements. This applies to any agent — parent or subagent.

## Single Ticket

When given a single non-epic ticket:

1. `ur ticket --output json show <id>` — read the ticket
2. `ur ticket --output json update <id> --status in_progress` — claim it
@/home/worker/.claude/skill-hooks/implement/after-ticket-claim.md
3. Implement the work directly in this context
@/home/worker/.claude/skill-hooks/implement/before-commit.md
@/home/worker/.claude/skill-hooks/implement/before-ticket-close.md
4. Commit, close: `ur ticket --output json update <id> --status closed`
5. Set a summary of the work done as ticket metadata:
   ```
   ur ticket set-meta <id> pr_summary "1-2 sentence summary of the changes made" --output json
   ```

If you cannot complete the work, run `workertools agent request-human "<reason>"` and stop.

Do NOT push, create PRs, or advance lifecycle status — that happens automatically after you stop.

No subagents needed. Just do the work.

## Epic or Multiple Tickets — Subagent Dispatch

**Core principle:** The parent orchestrates via `ur ticket`; subagents do the work. Only essential outcomes flow back.

### Parallel (Default)

Dispatch all dispatchable tickets as subagents in parallel. Each subagent commits independently on the working branch.

1. `ur ticket --output json dispatchable <epic-id>` — get all unblocked tickets
2. Dispatch all subagents in parallel (each subagent claims its own ticket via the prompt template)
4. Each subagent closes its ticket when done
5. After all complete, re-query dispatchable — newly unblocked tickets may have surfaced
6. Repeat until no dispatchable tickets remain

Parent never reads files or explores code inline — if it takes more than a glance, delegate.

### Sequential Mode

Use when explicitly requested or when tickets have heavy file overlap (check the "Files" section in ticket bodies):

- Dispatch one subagent at a time on the working branch
- Each agent commits, and the next agent inherits all previous work
- Re-query `ur ticket --output json dispatchable <epic-id>` each iteration — newly unblocked tickets surface naturally
- Pass only 1-2 sentence summaries between tasks

### Testing Strategy

- **Subagents**: Run only the minimum tests needed to validate their change (check CLAUDE.md for project-specific commands)
- **Parent (after all issues done)**: Run full CI and fix any integration issues

@/home/worker/.claude/skill-hooks/implement/before-dispatch.md

### Subagent Prompt Template

```
Implement ticket <id>.

`ur ticket --output json show <id>` to read the full ticket. Tickets have four sections:
- **Description**: What to build and why
- **Context**: How this component interacts with neighbors — use this for architectural awareness
- **Files**: Likely file paths to create or modify — use this to focus your work
- **Acceptance Criteria**: Conditions for done — verify all are met before closing

Claim: `ur ticket --output json update <id> --status in_progress`

[If relevant: "Previous ticket accomplished: <1-2 sentences>"]

Constraints:
- [Scope boundaries]
- [What NOT to change]

Parallel work:
- Other agents may be working on sibling tickets at the same time
- They may modify some of the same files — check the ticket's Context and Files sections to understand potential overlap
- Keep your changes focused to the files relevant to your ticket to minimize conflicts

VCS:
- Use `git add <files> && git commit -m "message"` when done — do NOT switch branches
- The parent agent manages branching and pushing

Testing:
- Run only the minimum tests needed to validate YOUR change — not the full CI suite
- The parent agent will run full CI after all issues are done

@/home/worker/.claude/skill-hooks/implement/before-commit.md
@/home/worker/.claude/skill-hooks/implement/before-ticket-close.md

When done:
1. Close the ticket: `ur ticket --output json update <id> --status closed`
2. Do NOT add ticket IDs to commit messages
3. Return ONLY a 1-2 sentence summary of what you did and any key values/paths the next task might need
```

### After All Subagents Complete (Epic Only)

After all dispatchable tickets are done and CI passes:

1. Set `pr_summary` metadata on the epic with a summary of all work done:
   ```
   ur ticket set-meta <epic-id> pr_summary "Summary of all changes across subagents" --output json
   ```

If you cannot complete all work, run `workertools agent request-human "<reason>"` and stop.

Do NOT push, create PRs, or advance lifecycle status — that happens automatically after you stop.

### Common Mistakes

| Mistake | Fix |
|---------|-----|
| Switching branches mid-work | **Never.** All agents commit on the working branch |
| Parent reads full subagent output | Ask for "1-2 sentence summary" in every prompt |
| Parent explores code inline | Delegate to subagent |
| Re-query skipped after completion | Always `ur ticket --output json dispatchable <epic>` again — deps may have unblocked |
| Parallel without claiming | Two agents grab same ticket — always claim first |
| Using sequential when tickets are independent | Default to parallel — only use sequential when tickets have heavy file overlap |
| Calling /push or advancing lifecycle | **Never.** Push/PR/lifecycle happens automatically after you stop |

$ARGUMENTS
