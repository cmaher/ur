# Codeflows

Detailed flow diagrams for cross-cutting concerns. Consult these before modifying multi-component flows.

## Table of Contents

- `docs/codeflows/builderd.md` — Builderd architecture (%WORKSPACE% resolution, two client paths, proto definition, connection path)
- `docs/codeflows/config.md` — Unified project configuration (`ur.toml` parsing, template paths, config flow through the launch pipeline)
- `docs/codeflows/database.md` — Database lifecycle (Postgres connection, PgPool, migration, TicketRepo queries, pg_dump/pg_restore backup, BackupTaskManager scheduling, shutdown)
- `docs/codeflows/host-exec-flow.md` — Host execution flow (three-hop gRPC pipeline for git, gh commands from workers)
- `docs/codeflows/lifecycle-workflow.md` — Workflow coordinator (state machine, WorkflowCoordinator, WorkerdNextStepRouter, GithubPollerManager, workflow/intent tables, WorkflowStepComplete RPC)
- `docs/codeflows/pool-git-builder-flow.md` — Pool git operations via builderd (clone, fetch, reset through builder daemon)
- `docs/codeflows/process-launch-credentials.md` — Process launch and credential injection (how containers get Claude Code credentials)
- `docs/codeflows/project-file-mounting.md` — Project file mounting (template path resolution, volume mounts for git hooks/skill hooks/CLAUDE.md/custom mounts, convention fallback)
- `docs/codeflows/server-lifecycle.md` — Server lifecycle (`ur start`/`ur stop`, builderd spawn, compose generation, port allocation, network topology)
- `docs/codeflows/skill-loading.md` — Skill loading (baking skills into images, selective runtime activation)
- `docs/codeflows/ui-events.md` — UI events pipeline (Postgres triggers with pg_notify, PgListener LISTEN/NOTIFY, UiEventPoller, gRPC streaming, TUI consumption)
- `docs/codeflows/urui-v2-tea.md` — urui v2 TEA architecture (TEA loop, Msg/Cmd design, input focus stack, Component trait, navigation model, data fetching, UI event throttling)
