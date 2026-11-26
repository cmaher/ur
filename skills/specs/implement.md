# implement

- **Targets:** worker, local
- **Trigger:** Implementing a designed ticket, epic (ticket with open descendants), or well-defined task/plan where design is already settled.
- **Merges:** `implement` + `implement-agents`

## Core
- **Design is already done:** execute the existing design and acceptance criteria; do not restart design or ask for design approval. Resolve routine implementation details as you go.
- **Scope discipline:**
  - Read every file in `Files to read first` before editing anything.
  - Scope edits to `Files to change` (small consequential edits like import sites or adjacent types are fine).
  - Treat `Out of scope` as a hard boundary.
  - Escalate if acceptance criteria are ambiguous/contradictory, genuine scope expansion is needed, or context is insufficient — never guess.
- **Single unit vs multi-unit (epic) mode:**
  - Detect mode from open descendants / subtasks (runtime check, not ticket type).
  - **Single unit:** read spec/ticket → claim → read context files → implement → verify → commit.
  - **Multi-unit (epic) — two execution modes:**
    - **Direct (default):** work dispatchable units sequentially in one context, committing and closing each before re-querying newly unblocked units.
    - **Subagent dispatch (opt-in, or when epic has independent parallelizable subtasks):** parent orchestrates only (never explores code inline); dispatches unblocked units as subagents (parallel by default when files don't overlap; sequential when heavy file overlap). Subagents claim, implement, verify, commit, close, and return only a 1–2 sentence summary. Re-query dispatchable after each wave.
    - Honor caller overrides (e.g., `auto` instructing "do not dispatch subagents").
- **Git discipline:** commit after each unit on the working branch; never switch branches; do not put ticket IDs in commit messages; do not push or open PRs.

## Worker
- **Unattended stop contract:** follow `_profiles/worker.md` strictly (`ur-workertools status step-complete` when done, `ur-workertools status request-human "<reason>"` when blocked). Never stop silently or ask questions in prose.
- **Error recovery first:** before any implementation work, run `ur ticket --output json show <id> --activity-author workflow`; read newest failure output (and any referenced `/var/ur/logs/...`) and fix prior verification/CI/merge failures first.
- **Ticket lifecycle:**
  - Detect epic via `ur ticket list --tree <id> --status open --output json`.
  - Claim (`--status in_progress`), close children (`--status closed`), re-query `ur ticket --output json dispatchable <epic-id>`.
  - Set `ur ticket set-meta <id> pr_summary "<1-2 sentence summary>" --output json` on single ticket or epic when done.
- **Skill hooks (optional):**
  - Style: `@%UR_SKILL_HOOKS%/implement/style-guide.md`
  - After claim: `@%UR_SKILL_HOOKS%/implement/after-ticket-claim.md`
  - Before subagent dispatch (epics): `@%UR_SKILL_HOOKS%/implement/before-dispatch.md`
  - Per-unit verification: `@%UR_SKILL_HOOKS%/implement/subtask-verifications.md`
  - End-of-epic verification: `@%UR_SKILL_HOOKS%/implement/final-verifications.md`
  - Do NOT run verification commands outside what the hook sections specify.
- **Subagent prompt rules:** pass full ticket-reading + scope + verification hook instructions; subagents escalate via `request-human` if blocked (unless overridden by `auto`) and return only a 1–2 sentence summary.

## Local
- Accept either a `ur` ticket/epic ID, a plan/spec file, or a direct task description.
- If using `ur` tickets locally, keep ticket claim/close/`dispatchable` workflow; otherwise track subtasks directly.
- Verify using the project's documented build/lint/test commands (`CLAUDE.md` / `AGENTS.md` / `Makefile`).
- When blocked or scope is unclear, ask the user directly in chat; report summary when done.

## Guardrails
- Never restart design on an already-designed task.
- Never switch branches mid-work or push/advance lifecycle.
- Never skip re-querying dispatchable subtasks after closing a unit (dependencies may have unblocked).
- In subagent mode: never let two parallel agents work an unclaimed ticket; never dump full subagent transcripts into parent context.
- Worker: never run ad-hoc `cargo build`/`test`/`clippy` outside hook instructions; never use `pause-nudge` as a stop; never ignore `workflow` error activities.
