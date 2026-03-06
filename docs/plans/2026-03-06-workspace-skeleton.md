# Workspace Skeleton Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Create the Cargo workspace with three crates (ur, agent_tools, ur_rpc), CI pipeline, cargo-make tasks, clippy config, and bacon background checker.

**Architecture:** Cargo workspace at repo root with `crates/` directory containing three crates. CI via GitHub Actions. Local dev tasks via cargo-make. Bacon for persistent background compilation.

**Tech Stack:** Rust 2024 edition, clap (derive) for CLIs, tokio (async runtime), cargo-make, bacon, GitHub Actions, cargo-audit

---

### Task 1: Root Workspace Cargo.toml

**Files:**
- Create: `Cargo.toml`
- Create: `Cargo.lock` (auto-generated)

**Step 1: Create workspace Cargo.toml**

```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
edition = "2024"
version = "0.1.0"

[workspace.dependencies]
clap = { version = "4", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
```

**Step 2: Verify it parses**

Run: `cargo metadata --format-version 1 --no-deps 2>&1 | head -1`
Expected: error about no members (no crates yet), but valid TOML

**Step 3: Commit**

```
git add Cargo.toml
git commit -m "feat: add workspace Cargo.toml"
```

---

### Task 2: ur_rpc Shared Library Crate

**Files:**
- Create: `crates/ur_rpc/Cargo.toml`
- Create: `crates/ur_rpc/src/lib.rs`

**Step 1: Create crate manifest**

```toml
[package]
name = "ur_rpc"
edition.workspace = true
version.workspace = true

[dependencies]
serde = { workspace = true }
```

**Step 2: Create lib.rs with a placeholder**

```rust
pub fn hello() -> &'static str {
    "ur_rpc"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        assert_eq!(hello(), "ur_rpc");
    }
}
```

**Step 3: Verify**

Run: `cargo test -p ur_rpc`
Expected: 1 test passes

**Step 4: Commit**

```
git add crates/ur_rpc/
git commit -m "feat: add ur_rpc shared library crate"
```

---

### Task 3: ur Host Monolith Crate

**Files:**
- Create: `crates/ur/Cargo.toml`
- Create: `crates/ur/src/main.rs`

**Step 1: Create crate manifest**

```toml
[package]
name = "ur"
edition.workspace = true
version.workspace = true

[dependencies]
clap = { workspace = true }
tokio = { workspace = true }
ur_rpc = { path = "../ur_rpc" }
```

**Step 2: Create main.rs with clap CLI skeleton**

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ur", about = "Coding LLM coordination framework")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the Ur background daemon
    Server,
    /// Launch the TUI dashboard
    Tui,
    /// Manage processes
    Process {
        #[command(subcommand)]
        command: ProcessCommands,
    },
    /// Manage tickets
    Ticket {
        #[command(subcommand)]
        command: TicketCommands,
    },
}

#[derive(Subcommand)]
enum ProcessCommands {
    /// Launch a new agent process
    Launch {
        ticket_id: String,
    },
    /// Show process status
    Status {
        process_id: Option<String>,
    },
    /// Attach to a running process
    Attach {
        process_id: String,
    },
}

#[derive(Subcommand)]
enum TicketCommands {
    /// Create a new ticket
    Create {
        title: String,
        #[arg(long)]
        parent: Option<String>,
    },
    /// List tickets
    Ls,
    /// Show ticket details
    Show {
        ticket_id: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Server => println!("Starting server..."),
        Commands::Tui => println!("Launching TUI..."),
        Commands::Process { command } => match command {
            ProcessCommands::Launch { ticket_id } => {
                println!("Launching process for ticket {ticket_id}...");
            }
            ProcessCommands::Status { process_id } => {
                println!("Status: {process_id:?}");
            }
            ProcessCommands::Attach { process_id } => {
                println!("Attaching to {process_id}...");
            }
        },
        Commands::Ticket { command } => match command {
            TicketCommands::Create { title, parent } => {
                println!("Creating ticket: {title} (parent: {parent:?})");
            }
            TicketCommands::Ls => println!("Listing tickets..."),
            TicketCommands::Show { ticket_id } => {
                println!("Showing ticket {ticket_id}...");
            }
        },
    }
}
```

**Step 3: Verify**

Run: `cargo run -p ur -- --help`
Expected: help output showing server, tui, process, ticket subcommands

Run: `cargo run -p ur -- process attach test-123`
Expected: "Attaching to test-123..."

**Step 4: Commit**

```
git add crates/ur/
git commit -m "feat: add ur host monolith crate with clap CLI"
```

---

### Task 4: agent_tools Worker CLI Crate

**Files:**
- Create: `crates/agent_tools/Cargo.toml`
- Create: `crates/agent_tools/src/main.rs`

**Step 1: Create crate manifest**

```toml
[package]
name = "agent_tools"
edition.workspace = true
version.workspace = true

[dependencies]
clap = { workspace = true }
tokio = { workspace = true }
ur_rpc = { path = "../ur_rpc" }
```

**Step 2: Create main.rs with clap CLI skeleton**

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agent_tools", about = "Worker CLI for Ur containers")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Ask a blocking question to the human operator
    Ask {
        question: String,
    },
    /// Proxy git commands to the host
    Git {
        /// Git arguments
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Interact with the ticket system
    Ticket {
        #[command(subcommand)]
        command: TicketCommands,
    },
}

#[derive(Subcommand)]
enum TicketCommands {
    /// Read the current ticket spec
    Read,
    /// Append a note to the current ticket
    Note {
        message: String,
    },
    /// Spawn a child ticket
    Spawn {
        #[arg(long)]
        title: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Update ticket status
    Status {
        status: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Ask { question } => {
            println!("Asking: {question}");
        }
        Commands::Git { args } => {
            println!("Git: {args:?}");
        }
        Commands::Ticket { command } => match command {
            TicketCommands::Read => println!("Reading ticket..."),
            TicketCommands::Note { message } => {
                println!("Adding note: {message}");
            }
            TicketCommands::Spawn { title, description } => {
                println!("Spawning: {title} ({description:?})");
            }
            TicketCommands::Status { status } => {
                println!("Setting status: {status}");
            }
        },
    }
}
```

**Step 3: Verify**

Run: `cargo run -p agent_tools -- --help`
Expected: help output showing ask, git, ticket subcommands

Run: `cargo run -p agent_tools -- ask "what color?"`
Expected: "Asking: what color?"

**Step 4: Commit**

```
git add crates/agent_tools/
git commit -m "feat: add agent_tools worker CLI crate"
```

---

### Task 5: Clippy Configuration

**Files:**
- Create: `clippy.toml`

**Step 1: Create clippy.toml**

```toml
excessive-nesting-threshold = 4
too-many-lines-threshold = 100
```

**Step 2: Verify clippy runs clean**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::excessive_nesting`
Expected: no warnings, exit 0

**Step 3: Commit**

```
git add clippy.toml
git commit -m "feat: add clippy configuration"
```

---

### Task 6: cargo-make Makefile.toml

**Files:**
- Create: `Makefile.toml`

**Step 1: Create Makefile.toml**

```toml
[config]
default_to_workspace = false

[tasks.fmt]
description = "Check code formatting"
command = "cargo"
args = ["fmt", "--all", "--check"]

[tasks.fmt-fix]
description = "Fix code formatting"
command = "cargo"
args = ["fmt", "--all"]

[tasks.clippy]
description = "Run clippy lints on all workspace crates"
command = "cargo"
args = ["clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings", "-W", "clippy::excessive_nesting"]

[tasks.build]
description = "Build all workspace crates"
command = "cargo"
args = ["build", "--workspace", "--all-features"]

[tasks.test]
description = "Run tests on all workspace crates"
command = "cargo"
args = ["test", "--workspace"]

[tasks.audit]
description = "Run cargo audit for dependency vulnerabilities"
command = "cargo"
args = ["audit"]

[tasks.ci]
description = "Run all CI checks"
dependencies = ["fmt", "clippy", "build", "test"]
```

**Step 2: Install cargo-make if needed and verify**

Run: `cargo make ci`
Expected: fmt, clippy, build, test all pass

**Step 3: Commit**

```
git add Makefile.toml
git commit -m "feat: add cargo-make CI tasks"
```

---

### Task 7: GitHub Actions CI

**Files:**
- Create: `.github/workflows/ci.yml`

**Step 1: Create CI workflow**

```yaml
name: CI

on:
  pull_request:
    branches: [master]

env:
  CARGO_TERM_COLOR: always

jobs:
  ci:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5

      - name: Install Rust toolchain
        uses: actions-rust-lang/setup-rust-toolchain@v1
        with:
          components: rustfmt, clippy

      - name: Cache cargo registry
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}
          restore-keys: |
            ${{ runner.os }}-cargo-

      - name: Install cargo-make
        run: cargo install cargo-make

      - name: Run CI checks
        run: cargo make ci

  audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5

      - name: Install Rust toolchain
        uses: actions-rust-lang/setup-rust-toolchain@v1

      - name: Install cargo-audit
        run: cargo install cargo-audit

      - name: Run cargo audit
        run: cargo audit
```

**Step 2: Commit**

```
git add .github/
git commit -m "feat: add GitHub Actions CI with clippy and cargo audit"
```

---

### Task 8: Bacon Configuration

**Files:**
- Create: `bacon.toml`
- Modify: `.gitignore`

**Step 1: Run bacon --init to get default config**

Run: `bacon --init`
Expected: creates bacon.toml with defaults

**Step 2: Add ai job and exports.locations to bacon.toml**

Append to bacon.toml:

```toml
[jobs.ai]
command = ["cargo", "check", "--color", "never", "--message-format", "short"]
need_stdout = true
watch = ["src", "crates"]

[exports.locations]
auto = true
path = ".bacon-locations"
line_format = "{kind} {path}:{line}:{column} {message}"
```

**Step 3: Add .bacon-locations to .gitignore**

Append `.bacon-locations` to `.gitignore`.

**Step 4: Verify bacon runs**

Run: `bacon ai` (then quit)
Expected: compiles, produces .bacon-locations file

**Step 5: Commit**

```
git add bacon.toml .gitignore
git commit -m "feat: add bacon background checker config"
```

---

### Task 9: CLAUDE.md

**Files:**
- Create: `CLAUDE.md`

**Step 1: Create CLAUDE.md with project docs and bacon instructions**

```markdown
# Ur

Coding LLM coordination framework. Native macOS monolith managing containerized Claude Code workers via tarpc over Unix Domain Sockets.

## Structure

Cargo workspace with three crates:
- `crates/ur/` - Host monolith (CLI, TUI, orchestration, RPC server)
- `crates/agent_tools/` - Worker CLI (runs inside containers, tarpc client)
- `crates/ur_rpc/` - Shared RPC library (tarpc service traits, data types)

## Development

- `cargo make ci` - Run all CI checks (fmt, clippy, build, test)
- `cargo make fmt-fix` - Fix formatting
- `cargo make clippy` - Run clippy lints
- `cargo make audit` - Check dependency vulnerabilities

## Rust Verification (Bacon)

- Bacon runs as a **persistent background watcher** -- the user starts it once in a terminal. Do NOT launch `bacon` yourself.
- Read `.bacon-locations` to get current diagnostics (errors/warnings from the last compile). This file is auto-updated by bacon's export-locations feature.
- If `.bacon-locations` doesn't exist or is empty, bacon may not be running. Fall back to `cargo check --message-format short 2>&1`.
- If you need to see only errors (no warnings), filter lines starting with `error` from `.bacon-locations`.
```

**Step 2: Commit**

```
git add CLAUDE.md
git commit -m "feat: add CLAUDE.md with project docs and bacon instructions"
```

---

### Task 10: Update .gitignore

**Files:**
- Modify: `.gitignore`

**Step 1: Ensure .gitignore has all needed entries**

The final .gitignore should contain:

```
# Tickets (tracked on orphan 'tickets' branch via git worktree)
.tickets
.worktrees

# Build artifacts
target/

# Bacon
.bacon-locations
```

**Step 2: Commit (if not already committed in task 8)**

```
git add .gitignore
git commit -m "feat: update .gitignore for Rust workspace"
```
