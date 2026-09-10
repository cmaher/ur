# Codeflows

Detailed flow diagrams for cross-cutting concerns. Consult these before modifying multi-component flows.

## Table of Contents

- `docs/codeflows/builderd.md` — Builderd architecture (%WORKSPACE% resolution, three gRPC services, three client paths, proto definitions, connection path)
- `docs/codeflows/config.md` — Unified project configuration (`ur.toml` parsing, template paths, config flow through the launch pipeline, agent default precedence, per-agent `[worker_models]`, launch-time image alias resolution)
- `docs/codeflows/database.md` — Database lifecycle (two-pool architecture: ticket_db + workflow_db, per-crate migrations, cross-DB soft references, two independent BackupTaskManagers, shutdown order)
- `docs/codeflows/host-exec-flow.md` — Host execution flow (three-hop gRPC pipeline for git, gh commands from workers)
- `docs/codeflows/lifecycle-workflow.md` — Workflow coordinator (state machine, WorkflowCoordinator, WorkerdNextStepRouter, GithubPollerManager, workflow/intent tables, WorkflowStepComplete RPC, agent-phrased dispatch commands)
- `docs/codeflows/pool-git-builder-flow.md` — Pool slot operations via builderd (DB orchestration in server, coarse-grained BuilderPoolService RPCs, slot acquire/release/shared/checkout flows)
- `docs/codeflows/process-launch-credentials.md` — Process launch and credential injection (Claude Keychain/file, Codex host `auth.json`, AGY in-container OAuth cache, and why each credential is mounted as one file rather than a whole agent state directory)
- `docs/codeflows/project-file-mounting.md` — Project file mounting (template path resolution, volume mounts for git hooks/skill hooks/CLAUDE.md/custom mounts, convention fallback)
- `docs/codeflows/server-lifecycle.md` — Server lifecycle (`ur start`/`ur stop`, builderd spawn, compose generation, port allocation, network topology)
- `docs/codeflows/skill-loading.md` — Skill loading (baking skills into images, selective runtime activation, skill hook integration)
- `docs/codeflows/ui-events.md` — UI events pipeline (Postgres triggers with pg_notify, PgListener LISTEN/NOTIFY, UiEventPoller, gRPC streaming, TUI consumption)
- `docs/codeflows/urui-v2-tea.md` — urui v2 TEA architecture (TEA loop, Msg/Cmd design, input focus stack, Component trait, navigation model, data fetching, UI event throttling)
