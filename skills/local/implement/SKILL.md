---
name: implement
description: Use when implementing a well-defined task, design doc, plan, or ticket where design and acceptance criteria are already settled
---

# Implement

Implement a single well-defined unit of work or a multi-task epic/plan where design is already settled. Do not restart design or ask for design approval; resolve routine implementation details as you go.

## Inputs

Accept whichever form the user provides:
- A `ur` ticket or epic ID (use `ur ticket ... --output json` for claim/close/`dispatchable` tracking)
- A design doc, plan file, or task checklist
- A direct task description in chat

## Scope Discipline

- Read every file listed in `Files to read first` (or referenced context) before editing anything.
- Scope edits to the planned `Files to change` (small consequential edits like import sites or adjacent types are fine).
- Treat `Out of scope` items as a hard boundary.
- If acceptance criteria are ambiguous or contradictory, genuine scope expansion is needed, or context is insufficient, **stop and ask the user directly in chat** rather than guessing.

## Single Unit vs Multi-Unit Mode

Determine at runtime whether there are open subtasks/descendants (for `ur` tickets: `ur ticket list --tree <id> --status open --output json`).

### Single Unit
1. Read the full spec/ticket and any `Files to read first` files.
2. If working a `ur` ticket, claim it (`ur ticket --output json update <id> --status in_progress`).
3. Implement within scope.
4. Verify using the project's documented build, lint, and test commands (`CLAUDE.md`, `AGENTS.md`, or `Makefile`).
5. Commit on the working branch (never switch branches; never put ticket IDs in commit messages; do not push or open PRs unless asked).
6. If working a `ur` ticket, close it (`ur ticket --output json update <id> --status closed`).
7. Report a concise summary of what changed and what was verified.

### Multi-Unit (Epic / Plan)
Choose between two modes:
- **Direct (default)** — work unblocked subtasks sequentially in this context by running **Single Unit** steps 1–6 for each subtask (for `ur` tickets: `ur ticket --output json show <child-id>`, claim `--status in_progress`, implement within scope, verify, commit, close `--status closed`, then re-query `ur ticket --output json dispatchable <epic-id>` to pick up newly unblocked work). Always use Direct when instructed by a caller like `auto` or when tasks are tightly coupled.
- **Subagent dispatch (opt-in / independent subtasks)** — parent orchestrates only (never explores code inline). Dispatch unblocked subtasks in parallel when files don't overlap, or sequentially when files heavily overlap. Each subagent claims its task/ticket, stays within scope, runs project verification, commits on the working branch without switching branches, closes its ticket if applicable, and returns only a 1–2 sentence summary. Re-query unblocked tasks after each wave until none remain.

After all subtasks finish, run the project's full test/lint suite and report the overall summary in chat.

## Guardrails

- Never restart design on an already-designed task.
- Never switch branches mid-work, add ticket IDs to commit messages, or push/open PRs unless asked.
- Never skip re-querying newly unblocked subtasks after completing a dependency.
- In subagent mode, never let parallel agents grab the same task unclaimed, and never dump full subagent transcripts into parent context.

$ARGUMENTS
<!-- compiled-from: skills/specs/implement.md@ba78c5ca5043 profile:local@daa4419e0589 -->
