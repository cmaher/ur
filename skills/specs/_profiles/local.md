# Profile: Local Host (`local`)

Shared rules applied to every compiled skill in `skills/local/` (symlinked into home Claude Code, AGY, and Codex skill directories).

## Environment & Runtime
- Runs interactively on the user's host machine across **Claude Code, AGY, and Codex**.
- Use agent-agnostic phrasing: avoid Claude-only syntax (`@file` includes, Claude-specific tool names when generic phrasing works).
- No `ur-workertools` binary or worker daemon is present — never reference `ur-workertools`.
- No `@%UR_SKILL_HOOKS%` directives — rely on the repo's `CLAUDE.md` / `AGENTS.md` / `Makefile` or explicit user instructions for verification commands and project rules.

## Interaction & Stopping
- A human is present in the session: report outcomes, ask clarifying questions, and escalate blockers directly in chat.
- Respect explicit autonomy when invoked (e.g., `auto` still avoids interrupting except on Hard stops, then reports in chat when finished).

## Tickets & Scope
- Do not require `ur ticket` unless the user passed a ticket ID or explicitly asked to use `ur` tickets; work equally well from a prompt, design doc, plan, or git branch.
- Do not push or open PRs unless asked.

## Project Brain
- `<brain>` is `$UR_BRAIN` if set; otherwise resolve via `ur project list --output json` or `ur dream path -p <key>` (using the skill argument or matching the repo's `origin` remote; ask if none or several match).
- Pass `-p <key>` to `ur dream` commands on the host, run from the normal workspace directory (not `<brain>`).
- Run brain git commands from inside `<brain>` (`cd <brain>` first).
