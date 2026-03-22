---
name: implement
description: Use when implementing a ticket — works directly for a single ticket, dispatches subagents for epics (tickets with open descendants)
---

# Implement Tickets

Implement one or more tickets. An **epic** is any ticket with open descendants — detected at runtime, not by ticket type.

**If the ticket description and codebase do not provide enough context to implement confidently, run `workertools agent request-human "<what context is missing>"` and stop.** Do not guess or make assumptions about unclear requirements. This applies to any agent — parent or subagent.

## Detect Mode — Single Ticket vs Epic

After reading the ticket, determine which mode to use:

1. `ur ticket list --tree <id> --status open --output json` — check for open descendants
2. If the result contains **any open descendants** → this ticket is an **epic** → use **Subagent Dispatch** mode
3. If **no open descendants** (empty list or only the ticket itself) → use **Single Ticket** mode

## Error Recovery — Check Before Starting

Before doing any implementation work, check for unaddressed workflow error activities on the ticket. These appear when a previous attempt failed verification, CI, or merge.

1. `ur ticket --output json show <id> --activity-author workflow` — read the ticket with only workflow activities
2. Read the **most recent** workflow activities first (they contain the latest failure output)
3. If workflow error activities exist and are not yet addressed:
   - The error output describes what failed (build errors, test failures, merge conflicts, etc.)
   - Fix the errors **before** moving on to any other work or dispatching child tickets
   - The server sends `/clear` before every dispatch, so you start with a clean conversation — the ticket activities are your only source of prior context
4. If no workflow error activities exist, proceed normally

This applies to both single-ticket and epic flows. For epics (tickets with open descendants), fix any errors on the epic ticket itself before dispatching children.

## Single Ticket

When the ticket has no open descendants:

1. `ur ticket --output json show <id>` — read the ticket (and check for workflow error activities per above)
2. `ur ticket --output json update <id> --status in_progress` — claim it
@/home/worker/.claude/skill-hooks/implement/after-ticket-claim.md
3. Implement the work directly in this context
4. Before committing, run any verifications listed in the **Verification Hooks** section below
5. Commit, close: `ur ticket --output json update <id> --status closed`
6. Set a summary of the work done as ticket metadata:
   ```
   ur ticket set-meta <id> pr_summary "1-2 sentence summary of the changes made" --output json
   ```

### Verification Hooks

Do NOT run any verification commands unless specified in this section.

@/home/worker/.claude/skill-hooks/implement/subtask-verifications.md

If you cannot complete the work, run `workertools agent request-human "<reason>"` and stop.

Do NOT push, create PRs, or advance lifecycle status — that happens automatically after you stop.

**REQUIRED: Signal completion by running `workertools step-complete` in bash when all work is done.** The system will not advance until this signal is sent.

No subagents needed. Just do the work.

## Epic (Ticket with Open Descendants) — Subagent Dispatch

**Core principle:** The parent orchestrates via `ur ticket`; subagents do the work. Only essential outcomes flow back.

### Parallel (Default)

Dispatch all dispatchable tickets as subagents in parallel. Each subagent commits independently on the working branch.

1. Check for workflow error activities on the epic ticket (see Error Recovery above) — fix before dispatching
2. `ur ticket --output json dispatchable <epic-id>` — get all unblocked tickets
3. Dispatch all subagents in parallel (each subagent claims its own ticket via the prompt template)
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

### Verification

Do NOT run any verification commands unless specified in a **Verification Hooks** section.

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

--- VERIFICATION HOOKS ---
Do NOT run any verification commands unless specified in this section.

@/home/worker/.claude/skill-hooks/implement/subtask-verifications.md
--- END VERIFICATION HOOKS ---

When done:
1. Close the ticket: `ur ticket --output json update <id> --status closed`
2. Do NOT add ticket IDs to commit messages
3. Return ONLY a 1-2 sentence summary of what you did and any key values/paths the next task might need
```

### After All Subagents Complete (Epic Only)

@/home/worker/.claude/skill-hooks/implement/final-verifications.md

After all dispatchable tickets are done and verification passes:

1. Set `pr_summary` metadata on the epic with a summary of all work done:
   ```
   ur ticket set-meta <epic-id> pr_summary "Summary of all changes across subagents" --output json
   ```

If you cannot complete all work, run `workertools agent request-human "<reason>"` and stop.

Do NOT push, create PRs, or advance lifecycle status — that happens automatically after you stop.

**REQUIRED: Signal completion by running `workertools step-complete` in bash when all work is done.** The system will not advance until this signal is sent.

### Common Mistakes

| Mistake | Fix |
|---------|-----|
| Switching branches mid-work | **Never.** All agents commit on the working branch |
| Parent reads full subagent output | Ask for "1-2 sentence summary" in every prompt |
| Parent explores code inline | Delegate to subagent |
| Re-query skipped after completion | Always `ur ticket --output json dispatchable <epic>` again — deps may have unblocked |
| Parallel without claiming | Two agents grab same ticket — always claim first |
| Using sequential when tickets are independent | Default to parallel — only use sequential when tickets have heavy file overlap |
| Advancing lifecycle | **Never.** Push/PR/lifecycle happens automatically after you stop |
| Running cargo build/test/clippy inline | **Never.** Only run verification commands specified by hook files |
| Ignoring workflow error activities | **Always** check for `source=workflow` activities before starting work |

$ARGUMENTS
