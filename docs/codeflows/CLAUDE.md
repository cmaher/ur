# Codeflows

Detailed flow diagrams for cross-cutting concerns. Consult these before modifying multi-component flows.

## Table of Contents

- `docs/codeflows/builderd.md` — Builderd architecture (%WORKSPACE% resolution, two client paths, proto definition, connection path)
- `docs/codeflows/config.md` — Unified project configuration (`ur.toml` parsing, template paths, config flow through the launch pipeline)
- `docs/codeflows/database.md` — Database lifecycle (SQLite initialization, migration, TicketRepo queries, SnapshotManager backup/restore, BackupTaskManager scheduling, shutdown)
- `docs/codeflows/host-exec-flow.md` — Host execution flow (three-hop gRPC pipeline for git, gh commands from workers)
- `docs/codeflows/lifecycle-workflow.md` — Workflow coordinator (state machine, WorkflowCoordinator, WorkerdNextStepRouter, GithubPollerManager, workflow/intent tables, WorkflowStepComplete RPC)
- `docs/codeflows/pool-git-builder-flow.md` — Pool git operations via builderd (clone, fetch, reset through builder daemon)
- `docs/codeflows/process-launch-credentials.md` — Process launch and credential injection (how containers get Claude Code credentials)
- `docs/codeflows/server-lifecycle.md` — Server lifecycle (`ur start`/`ur stop`, builderd spawn, compose generation, port allocation, network topology)
- `docs/codeflows/skill-loading.md` — Skill loading (baking skills into images, selective runtime activation)
