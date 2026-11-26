---
name: implement
description: Use when implementing a designed ur ticket or epic (ticket with open descendants) where design and acceptance criteria are already settled
---

# Implement Tickets

Implement a single ticket or an epic (any ticket with open descendants — detected at runtime, not by ticket type).

Design is already complete. Execute the ticket's existing design and acceptance criteria; do not start another design phase or ask for design approval. Resolve routine implementation details as you work.

## Optional Hook Files

The `@%UR_SKILL_HOOKS%/...` references below are optional project hooks. Read and follow each hook if the file exists. If a hook file is absent, skip it silently and continue — absence is never a blocker, never a reason to call `request-human`, and never a reason to create missing hooks or invent replacement verification commands.

## Unattended Execution — Exactly Two Ways To Stop

You are running unattended; response text is not delivered to anyone. Ending your turn without a signal silently stalls the workflow.

| Situation | Required action |
| --- | --- |
| All work is done | `ur-workertools status step-complete` |
| Blocked and cannot finish | `ur-workertools status request-human "<concrete reason or question>"` |

Never pause, wait, ask a question in prose, report partial progress, or stop without one of those two commands. `ur-workertools status pause-nudge` only suppresses nudges for 5 minutes while you keep working — it is never a way to stop.

Call `ur-workertools status request-human "<reason>"` and stop when:
- Acceptance criteria are ambiguous or contradictory
- Work requires genuine scope expansion beyond `Files to change` (small consequential edits like import sites or adjacent types are fine)
- Workflow error recovery activities are contradictory or incomplete
- Ticket description and codebase together lack enough context to implement confidently

Never guess on unclear requirements. Never push, create PRs, or advance lifecycle status — the workflow does that after `step-complete`.

*(Caller override: when driven by the `auto` skill, follow its prompt override instead of calling `ur-workertools status` or dispatching subagents.)*

## Style

Follow this style guide for all development (parent and subagents alike):

@%UR_SKILL_HOOKS%/implement/style-guide.md

## Error Recovery (Always First)

Before any implementation work or child dispatch, check for unaddressed workflow failures on `<id>`:

1. Run `ur ticket --output json show <id> --activity-author workflow`.
2. Read the **most recent** workflow activities first (build/test/CI/merge failures) and any referenced log files (e.g., `/var/ur/logs/...`).
3. Fix unaddressed failures before doing other work. The server runs `/clear` before dispatch, so ticket activities are your only prior context.

## Detect Mode

Run `ur ticket list --tree <id> --status open --output json`:
- **No open descendants** (empty or only `<id>`) → **Single Ticket** mode.
- **Any open descendants** → **Epic** mode.

## Single Ticket Mode

1. Read the ticket: `ur ticket --output json show <id>`.
2. Claim it: `ur ticket --output json update <id> --status in_progress`.
@%UR_SKILL_HOOKS%/implement/after-ticket-claim.md
3. Read every file listed in `Files to read first` (if present) before editing anything.
4. Implement within `Files to change`; treat `Out of scope` as a hard boundary.
5. Run only the verification commands specified in **Verification Hooks** below.
6. Commit on the working branch (never switch branches; never include ticket IDs in commit messages).
7. Record a summary: `ur ticket set-meta <id> pr_summary "1-2 sentence summary" --output json`.
8. Signal `ur-workertools status step-complete`.

## Epic Mode

Choose execution mode:
- **Direct (default)** — work dispatchable children sequentially in this context (always use Direct when invoked by the `auto` skill or when subtasks are small/tightly coupled).
- **Subagent dispatch (opt-in / parallelizable)** — use when explicitly requested or when unblocked children touch independent files. Parent orchestrates via `ur ticket` only and never explores code inline.

### Direct Execution
1. Fix any workflow error activities on `<epic-id>` first.
2. Query `ur ticket --output json dispatchable <epic-id>`.
3. For each dispatchable child, follow **Single Ticket Mode** steps 1–6 (`ur ticket --output json show <child-id>`, claim `--status in_progress`, run `@%UR_SKILL_HOOKS%/implement/after-ticket-claim.md`, read `Files to read first`, implement within `Files to change` / `Out of scope`, run **Verification Hooks**, commit on the working branch without ticket IDs), then close it (`ur ticket --output json update <child-id> --status closed`).
4. Re-query `ur ticket --output json dispatchable <epic-id>` after completing tickets — dependencies may have unblocked. Repeat until none remain.

### Subagent Dispatch Execution
1. Fix any workflow error activities on `<epic-id>` first.
2. Run pre-dispatch hook:
@%UR_SKILL_HOOKS%/implement/before-dispatch.md
3. Query `ur ticket --output json dispatchable <epic-id>`.
4. Dispatch subagents **in parallel** when `Files to change` do not heavily overlap; dispatch **sequentially** (one at a time, passing only 1–2 sentence summaries) when files heavily overlap or sequential mode is requested.
5. Give each subagent this prompt:

```
Implement ticket <id>.
1. Read: `ur ticket --output json show <id>` (read all `Files to read first` before editing).
2. Claim: `ur ticket --output json update <id> --status in_progress`.
3. Scope: stay within `Files to change` (small import/type consequences fine); treat `Out of scope` as a hard boundary. If genuine scope expansion or ambiguity blocks you, run `ur-workertools status request-human "<reason>"` and stop.
4. Sibling agents may edit other files on the same branch — keep edits focused. Never switch branches.
5. Verify ONLY with these hooks:
@%UR_SKILL_HOOKS%/implement/subtask-verifications.md
6. Stage your files and commit (no ticket IDs in commit message; do not push).
7. Close: `ur ticket --output json update <id> --status closed`.
8. Return ONLY a 1-2 sentence summary of what changed and any key paths/values needed next.
```

6. Re-query `ur ticket --output json dispatchable <epic-id>` after each wave until empty.

### Epic Wrap-Up
1. Run final verification hook:
@%UR_SKILL_HOOKS%/implement/final-verifications.md
2. Set epic summary: `ur ticket set-meta <epic-id> pr_summary "Summary of all changes" --output json`.
3. Signal `ur-workertools status step-complete`.

## Verification Hooks

Do NOT run verification commands (such as ad-hoc `cargo build`/`test`/`clippy`) unless specified in a hook section.

@%UR_SKILL_HOOKS%/implement/subtask-verifications.md

## Common Mistakes

| Mistake | Fix |
| --- | --- |
| Stopping or asking a question in prose | Always end via `step-complete` or `request-human "<reason>"` |
| Using `pause-nudge` to pause | Only mutes nudges for 5 min; never ends a turn |
| Skipping workflow error activities | Always check `--activity-author workflow` before starting |
| Skipping `dispatchable` re-query | Always re-query after closing tickets — deps unblock |
| Parallel subagents on unclaimed tickets | Every subagent claims `--status in_progress` first |
| Switching branches or pushing | Commit on working branch only; never push or advance lifecycle |
| Running unhooked verification commands | Run only commands specified in Verification Hooks |

$ARGUMENTS
<!-- compiled-from: skills/specs/implement.md@ba78c5ca5043 profile:worker@6afed0c52191 -->
