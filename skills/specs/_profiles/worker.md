# Profile: ur Worker (`worker`)

Shared rules applied to every compiled skill in `containers/worker-base/potential-skills/`.

## Environment & Runtime
- Runs inside a `ur` worker (container or host-local `runtime = "local"`), driven by `ur-server` / `workerd`.
- Skills are installed by `workerd init` (or as a `ur:` plugin for local workers). Cross-skill references to other installed skills are rewritten automatically by `workerd`.
- Project instructions (`CLAUDE.md` / `AGENTS.md`) and shared instructions (`tickets.md`) are already composed into the agent's instruction file.

## Unattended Execution & Stopping Contract
- Automated worker skills (`implement`, `auto`, `address-feedback`) run unattended; response text is NOT delivered to a human.
- Exactly two legal ways to end a turn:
  - Done: `ur-workertools status step-complete`
  - Blocked: `ur-workertools status request-human "<concrete reason or question>"`
- Never pause, wait, ask questions in response text, or stop without one of the two signals.
- `ur-workertools status pause-nudge` only mutes nudges for 5 minutes while continuing work; it is never a way to stop.
- Never push, open PRs, or advance ticket lifecycle status unless the specific skill explicitly requires it — the workflow handles that after `step-complete`.

## Tickets
- Manage tickets via `ur ticket ... --output json` (always pass `--output json`).
- Check unaddressed workflow error activities (`ur ticket --output json show <id> --activity-author workflow`) before starting implementation work.
- Do not put ticket IDs in commit messages.

## Project Skill Hooks
- Worker skills that support project hooks reference them via `@%UR_SKILL_HOOKS%/<skill>/<hook>.md`.
- Always include the optional-hook rule: read and follow each hook if the file exists; if absent, skip silently and continue (never treat absence as a blocker, never call `request-human`, never create missing hooks or invent replacement checks).

## Project Brain
- `<brain>` is `$UR_BRAIN` (mounted at `/brain` in workers with a brain).
- Run `ur dream` from the normal workspace directory, never from inside `<brain>` (project is pinned in workers).
- Run every `git` command for the brain from inside `<brain>` (`cd <brain>` first), never `git -C` (only cwd is mapped to host).
