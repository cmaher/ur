---
name: implement
description: Use when implementing a ticket — implements a single ticket, or an epic (ticket with open descendants) by working through its dispatchable descendants
---

# Implement Tickets

Implement one or more tickets. An **epic** is any ticket with open descendants — detected at runtime, not by ticket type.

**Run `workertools status request-human "<reason>"` and stop when:**
- Ticket acceptance criteria are ambiguous or contradictory
- Work appears to require changes beyond `Files to change` that are not obvious small consequences (import sites, adjacent types are fine)
- Error recovery activities contain contradictory or incomplete guidance
- Ticket description and codebase together do not provide enough context to implement confidently

Do not guess or make assumptions about unclear requirements.

## Style

Follow this style guide for all development.

@/home/worker/.claude/skill-hooks/implement/style-guide.md

## Detect Mode — Single Ticket vs Epic

After reading the ticket, determine which mode to use:

1. `ur ticket list --tree <id> --status open --output json` — check for open descendants
2. If the result contains **any open descendants** → this ticket is an **epic** → use **Epic** mode
3. If **no open descendants** (empty list or only the ticket itself) → use **Single Ticket** mode

## Error Recovery — Check Before Starting

Before doing any implementation work, check for unaddressed workflow error activities on the ticket. These appear when a previous attempt failed verification, CI, or merge.

1. `ur ticket --output json show <id> --activity-author workflow` — read the ticket with only workflow activities
2. Read the **most recent** workflow activities first (they contain the latest failure output)
3. If workflow error activities exist and are not yet addressed:
   - The error output describes what failed (build errors, test failures, merge conflicts, etc.)
   - Activities may reference log files (e.g., `/var/ur/logs/...`) — read them for full error details
   - Fix the errors **before** moving on to any other work
   - The server sends `/clear` before every dispatch, so you start with a clean conversation — the ticket activities are your only source of prior context
4. If no workflow error activities exist, proceed normally

This applies to both single-ticket and epic flows. For epics, fix any errors on the epic ticket itself before implementing children.

## Single Ticket

When the ticket has no open descendants:

1. `ur ticket --output json show <id>` — read the ticket (and check for workflow error activities per above)
2. `ur ticket --output json update <id> --status in_progress` — claim it
@/home/worker/.claude/skill-hooks/implement/after-ticket-claim.md
3. Read every file listed in the ticket's `Files to read first` section (if present) before editing anything. This loads the patterns and conventions needed for the change.
4. Implement the work, scoped to the ticket's `Files to change` list. Treat `Out of scope` as a hard boundary.
5. Before committing, run any verifications listed in the **Verification Hooks** section below
6. Commit and set a summary of the work done as ticket metadata:
   ```
   ur ticket set-meta <id> pr_summary "1-2 sentence summary of the changes made" --output json
   ```

### Verification Hooks

Do NOT run any verification commands unless specified in this section.

@/home/worker/.claude/skill-hooks/implement/subtask-verifications.md

If you cannot complete the work, run `workertools status request-human "<reason>"` and stop.

Do NOT push, create PRs, or advance lifecycle status — that happens automatically after you stop.

**REQUIRED: Signal completion by running `workertools status step-complete` in bash when all work is done.** The system will not advance until this signal is sent.

## Epic (Ticket with Open Descendants)

Work through the epic's descendants until none are left to implement:

1. Check for workflow error activities on the epic ticket (see Error Recovery above) — fix before implementing children
2. `ur ticket --output json dispatchable <epic-id>` — get all currently unblocked tickets
3. Implement each dispatchable ticket using the **Single Ticket** steps above (claim → read → implement → verify → commit):
   - Close it when done: `ur ticket --output json update <id> --status closed`
   - Do NOT add ticket IDs to commit messages
   - Commit on the working branch — never switch branches
4. Re-query `ur ticket --output json dispatchable <epic-id>` — completing a ticket may have unblocked new ones
5. Repeat until no dispatchable tickets remain

Commit after each ticket so progress is durable.

### Verification

Do NOT run any verification commands unless specified in a **Verification Hooks** section.

@/home/worker/.claude/skill-hooks/implement/subtask-verifications.md

### After All Descendants Complete (Epic Only)

@/home/worker/.claude/skill-hooks/implement/final-verifications.md

After all dispatchable tickets are done and verification passes:

1. Set `pr_summary` metadata on the epic with a summary of all work done:
   ```
   ur ticket set-meta <epic-id> pr_summary "Summary of all changes" --output json
   ```

If you cannot complete all work, run `workertools status request-human "<reason>"` and stop.

Do NOT push, create PRs, or advance lifecycle status — that happens automatically after you stop.

**REQUIRED: Signal completion by running `workertools status step-complete` in bash when all work is done.** The system will not advance until this signal is sent.

### Common Mistakes

| Mistake | Fix |
|---------|-----|
| Switching branches mid-work | **Never.** All commits go on the working branch |
| Re-query skipped after completion | Always `ur ticket --output json dispatchable <epic>` again — deps may have unblocked |
| Working a ticket without claiming | Always claim (`--status in_progress`) before implementing, and close when done |
| Advancing lifecycle | **Never.** Push/PR/lifecycle happens automatically after you stop |
| Running cargo build/test/clippy inline | **Never.** Only run verification commands specified by hook files |
| Ignoring workflow error activities | **Always** check for `source=workflow` activities before starting work |

$ARGUMENTS
