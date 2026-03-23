# Ur Agentic Development Environment (Ur ADE)

Run [Claude Code](https://docs.anthropic.com/en/docs/claude-code) agents in secure, isolated containers with full permissions — no more permission prompts, no security trade-offs.

Ur ADE coordinates containerized Claude Code workers via gRPC, managing the full lifecycle from design through implementation and PR creation. All agent activity is sandboxed: workers run on isolated Docker networks with no direct host or internet access, while a Lua-scripted command gateway mediates every host interaction.

## How It Works

```
┌─────────────────────────────────────────────────────────────────┐
│  Host (macOS)                                                   │
│                                                                 │
│  ┌─────────┐    ┌──────────┐    ┌──────────────────────────┐   │
│  │ ur CLI  │───▶│  Server  │───▶│  Worker Containers       │   │
│  │ ur TUI  │    │ (Docker) │    │  ┌────────┐ ┌────────┐   │   │
│  └─────────┘    │          │    │  │Claude  │ │Claude  │   │   │
│                 │ Tickets  │    │  │Code    │ │Code    │   │   │
│  ┌──────────┐  │ Workflow  │    │  └───┬────┘ └───┬────┘   │   │
│  │ builderd │◀─│ Workers   │    │      │          │        │   │
│  │ (host)   │  └──────────┘    │  workerd      workerd     │   │
│  └──────────┘       │          └──────────────────────────┘   │
│       │         ┌───┴───┐                                      │
│       │         │ Squid │  (proxy: Anthropic domains only)     │
│       ▼         └───────┘                                      │
│  Git repos, gh, cargo, docker (sandboxed)                      │
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
2. Press **C** to create a new ticket, selecting **design** as the type
3. Attach to the design worker with `ur worker attach`
4. In the worker session, run `/design <ticket-id>` to generate a design and implementation plan
5. The design worker breaks the ticket into sub-tasks with dependencies

### 2. Implement

Dispatch the ticket and let workers implement it autonomously:

1. In the TUI **Tickets** page, select the design ticket
2. Press **D** to dispatch — the server creates a workflow that assigns workers to sub-tasks
3. Workers implement tickets in parallel (or sequentially for dependent tasks), each committing to a feature branch
4. The workflow coordinator manages the full lifecycle: claiming tickets, running CI, creating PRs

### 3. Review

Review and merge the results:

1. When the workflow completes, a PR is created automatically
2. Review the PR on GitHub
3. Use `ur approve` to approve or `ur respond` to request changes — workers will address feedback and update the PR

## Prerequisites

### Container Runtime

Ur requires Docker with Compose support. On macOS, [OrbStack](https://orbstack.dev/) is recommended as a lightweight, fast alternative to Docker Desktop.

### Build Dependencies

All project dependencies are managed via [mise](https://mise.jdx.dev/):

```sh
# Install mise (macOS)
brew install mise

# Install all project tools (Rust, protoc, zig, cargo-make, etc.)
mise install
```

Required tooling (installed by mise): `rust`, `protoc`, `zig`, `cargo-make`, `cargo-zigbuild`, `cargo-audit`

## Getting Started

```sh
# Build and install the ur CLI + container images
cargo make install

# Start the server (launches Docker Compose, builderd, Squid proxy)
ur server start

# Open the TUI
ur tui
```

Configure projects in `~/.ur/ur.toml` — each project specifies a git repository and optional container mounts. See `ur.toml` for available options.

## Development

```sh
cargo make ci        # Run all CI checks (fmt, clippy, build, test)
cargo make fmt-fix   # Auto-format code
cargo make clippy    # Run clippy lints
cargo make audit     # Check dependency vulnerabilities
```

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

See [LICENSE](LICENSE) for details.
