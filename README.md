# Ur Agentic Development Environment (Ur ADE)

Run [Claude Code](https://docs.anthropic.com/en/docs/claude-code) agents in secure, isolated containers with full permissions — no more permission prompts, no security trade-offs.

Ur ADE coordinates containerized Claude Code workers via gRPC, managing the full lifecycle from design through implementation and PR creation. All agent activity is sandboxed: workers run on isolated Docker networks with no direct host or internet access, while a Lua-scripted command gateway mediates every host interaction.

> **Warning:** Ur ADE is under active development and highly unstable. APIs, configuration formats, and behavior may change without notice.

## Prerequisites

### Container Runtime

Ur requires Docker with Compose support. On macOS, [OrbStack](https://orbstack.dev/) is recommended as a lightweight, fast alternative to Docker Desktop.

### Build Dependencies


```sh
# Instructions are for macos but should work on Linux and WSL with alternative installation commands

# Install mise (https://mise.jdx.dev/):
brew install mise

# Install terminal-notifier for notifications (Mac OS only)
brew install terminal-notifier

# Install github cli (https://github.com/cli/cli?tab=readme-ov-file)
brew install gh
```

## Getting Started

```sh
# clone
git clone https://github.com/cmaher/ur.git
cd ur
git submodule update --init --recursive

# Install all project tools (Rust, protoc, zig, cargo-make, etc.)
mise trust
mise install

# Build and install the ur CLI + container images
# installs binaries to ~/.local/bin
cargo make install

# Initialize the config directory (~/.ur/) with default ur.toml, squid allowlist, etc.
ur init

# Add a project (auto-detects repo URL from the git remote)
cd /path/to/your/project
ur project add . --image ur-worker

# Start the server (launches containers and host process)
ur server start

# Open the TUI
urui
```

Configure projects in `~/.ur/ur.toml` — each `[projects.<key>]` entry specifies a git repository and container configuration. Key options:

- **`container.image`** — Container image for workers (e.g. `"ur-worker"`, `"ur-worker-rust"`)
- **`container.mounts`** — Additional volume mounts for the container
- **`git_hooks_dir`** — Template path to git hook scripts run during verification (e.g. `"%PROJECT%/.ur/git-hooks"`)
- **`skill_hooks_dir`** — Template path to skill hook snippets copied to `~/.claude/skill-hooks/` at container startup (e.g. `"%PROJECT%/.ur/skill-hooks"`)
- **`workflow_hooks_dir`** — Template path to workflow hook scripts for lifecycle automation

Template paths support `%PROJECT%/...` (resolved relative to the project repo) and `%URCONFIG%/...` (resolved relative to `~/.ur/`).

## How It Works

```
┌─────────────────────────────────────────────────────────────────┐
│  Host (macOS)                                                   │
│                                                                 │
│  ┌─────────┐    ┌──────────┐    ┌──────────────────────────┐    │
│  │ ur CLI  │───▶│  Server  │───▶│  Worker Containers       │    │
│  │ ur TUI  │    │ (Docker) │    │  ┌────────┐ ┌────────┐   │    │
│  └─────────┘    │          │    │  │Claude  │ │Claude  │   │    │
│                 │ Tickets  │    │  │Code    │ │Code    │   │    │
│  ┌──────────┐   │ Workflow │    │  └───┬────┘ └───┬────┘   │    │
│  │ builderd │◀──│ Workers  │    │      │          │        │    │
│  │ (host)   │   └──────────┘    │  workerd      workerd    │    │
│  └──────────┘       │           └──────────────────────────┘    │
│       │         ┌───┴───┐                                       │
│       │         │ Squid │  (proxy: Anthropic domains only)      │
│       ▼         └───────┘                                       │
│  Git repos, gh, cargo, docker (sandboxed)                       │
└─────────────────────────────────────────────────────────────────┘
```

**Key components:**

- **Worker containers** — Run Claude Code with all permissions on an isolated Docker network
- **Server** — Orchestrates workers, manages tickets, automates the development lifecycle
- **builderd** — Host daemon that executes sandboxed commands (git, gh, cargo) on behalf of workers
- **Squid proxy** — Restricts network access to Anthropic API domains only
- **Lua command gateway** — Validates and filters every host command, blocking directory escapes and dangerous flags

See [docs/design.md](docs/design.md) for the full architecture and security model.

## Typical Workflow

Ur ADE is built around the TUI for day-to-day use. The typical development cycle:

### 1. Design

Create a design ticket in the TUI and let a worker architect the solution:

1. Open the TUI and navigate to the **Tickets** page
2. Press **C** to open the ticket editor — fill in the title and description, then save and exit
3. Select **Create as Design** from the ticket type menu
4. Attach to the design worker with `ur worker attach`
5. In the worker session, run `/design <ticket-id>` to generate a design and implementation plan
6. The design worker breaks the ticket into sub-tasks with dependencies

### 2. Implement

Dispatch the ticket and let workers implement it autonomously:

1. In the TUI **Tickets** page, select the design ticket
2. Press **D** to dispatch — the server creates a workflow that assigns workers to sub-tasks
3. Monitor dispatch progress from the TUI **Flows** page
4. Workers implement tickets in parallel (or sequentially for dependent tasks), each committing to a feature branch
5. The workflow coordinator manages the full lifecycle: claiming tickets, running CI, creating PRs

### 3. Review

Review and merge the results:

1. When the workflow completes, a PR is created automatically — tickets show as `in_review` on the **Flows** page during this step
2. Review the PR on GitHub
3. Use `ur approve` to approve or `ur respond` to request changes — workers will address feedback and update the PR

## Development

```sh
cargo make ci        # Run all CI checks (fmt, clippy, build, test)
cargo make fmt-fix   # Auto-format code
cargo make clippy    # Run clippy lints
cargo make audit     # Check dependency vulnerabilities
```

Alternatively, run `cargo make install-hooks` and rely on pre-push hooks.

## Project Structure

```
crates/
  ur/        — Host CLI (TUI, process management, ticket management)
  server/    — Server (orchestration, gRPC server, container management)
  ur_rpc/    — Shared RPC contract (protobuf/tonic service definitions)
  workercmd/ — Worker binaries for containers (ur-ping, git proxy)
docs/
  design.md  — Architecture and security model
  codeflows/ — Detailed flow diagrams for cross-cutting concerns
```

## License

MIT
