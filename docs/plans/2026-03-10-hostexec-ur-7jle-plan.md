# HostExec Implementation Plan (ur-7jle)

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace dedicated git/gh command passthrough with a general, Lua-configured host command execution gateway via a new ur-hostd host daemon.

**Architecture:** Workers call commands naturally (e.g. `git status`). Bash shims route to `ur-tools host-exec` which sends gRPC to ur-server. ur-server validates against an allowlist, runs optional Lua transforms, maps container CWD to host paths, then forwards to ur-hostd on the host for actual execution. Output streams back in real-time.

**Tech Stack:** Rust, tonic gRPC, mlua (Lua 5.4), TOML config, protobuf

**Spec:** `docs/plans/2026-03-10-hostexec-ur-7jle-design.md`

---

## Chunk 1: Protocol & RPC Foundation

### Task 1: Proto Definitions

**Files:**
- Create: `proto/hostexec.proto`
- Create: `proto/hostd.proto`
- Modify: `crates/ur_rpc/Cargo.toml`
- Modify: `crates/ur_rpc/build.rs`
- Modify: `crates/ur_rpc/src/lib.rs`

- [ ] **Step 1: Create hostexec.proto**

```protobuf
// proto/hostexec.proto
syntax = "proto3";
package ur.hostexec;
import "core.proto";

service HostExecService {
  rpc Exec(HostExecRequest) returns (stream ur.core.CommandOutput);
  rpc ListCommands(ListHostExecCommandsRequest) returns (ListHostExecCommandsResponse);
}

message HostExecRequest {
  string command = 1;
  repeated string args = 2;
  string working_dir = 3;
}

message ListHostExecCommandsRequest {}

message ListHostExecCommandsResponse {
  repeated string commands = 1;
}
```

- [ ] **Step 2: Create hostd.proto**

```protobuf
// proto/hostd.proto
syntax = "proto3";
package ur.hostd;
import "core.proto";

service HostDaemonService {
  rpc Exec(HostDaemonExecRequest) returns (stream ur.core.CommandOutput);
}

message HostDaemonExecRequest {
  string command = 1;
  repeated string args = 2;
  string working_dir = 3;
}
```

- [ ] **Step 3: Add features to ur_rpc/Cargo.toml**

Add to `[features]`:
```toml
hostexec = []
hostd = []
```

- [ ] **Step 4: Update ur_rpc/build.rs to compile new protos**

Follow the existing pattern: append to the `protos` vec using `cfg!()` guards, and add `cargo:rerun-if-changed` directives:

```rust
// Add these rerun-if-changed lines alongside the existing ones:
println!("cargo:rerun-if-changed=../../proto/hostexec.proto");
println!("cargo:rerun-if-changed=../../proto/hostd.proto");

// Add these blocks alongside the existing git/gh blocks:
if cfg!(feature = "hostexec") {
    protos.push(proto_dir.join("hostexec.proto"));
}

if cfg!(feature = "hostd") {
    protos.push(proto_dir.join("hostd.proto"));
}
```

- [ ] **Step 5: Add modules to ur_rpc/src/lib.rs**

```rust
#[cfg(feature = "hostexec")]
#[allow(clippy::excessive_nesting)]
pub mod hostexec {
    tonic::include_proto!("ur.hostexec");
}
#[cfg(feature = "hostd")]
#[allow(clippy::excessive_nesting)]
pub mod hostd {
    tonic::include_proto!("ur.hostd");
}
```

- [ ] **Step 6: Verify compilation**

Run: `cargo build -p ur_rpc --features hostexec,hostd`
Expected: Compiles successfully

- [ ] **Step 7: Commit**

```
feat(ur_rpc): add hostexec and hostd proto definitions (ur-7jle)
```

---

### Task 2: Extract Stream Utility to ur_rpc

**Files:**
- Modify: `crates/ur_rpc/Cargo.toml`
- Create: `crates/ur_rpc/src/stream.rs`
- Modify: `crates/ur_rpc/src/lib.rs`
- Modify: `crates/server/src/stream.rs`

- [ ] **Step 1: Add tokio dependency to ur_rpc**

Add to `[dependencies]` in `crates/ur_rpc/Cargo.toml`:
```toml
tokio = { workspace = true, optional = true }
tracing = { version = "0.1", optional = true }
```

Add to `[features]`:
```toml
stream = ["dep:tokio", "dep:tracing"]
```

- [ ] **Step 2: Create ur_rpc/src/stream.rs**

Move the `spawn_child_output_stream` function from `crates/server/src/stream.rs` into `crates/ur_rpc/src/stream.rs`. The function body is identical to what's currently in `crates/server/src/stream.rs:14-83`. Update imports to use `crate::proto::core::{CommandOutput, command_output::Payload}`.

```rust
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tracing::warn;

use crate::proto::core::{CommandOutput, command_output::Payload};

pub fn spawn_child_output_stream(
    mut child: tokio::process::Child,
    tx: mpsc::Sender<Result<CommandOutput, tonic::Status>>,
) {
    // ... identical body from crates/server/src/stream.rs:14-83
}
```

- [ ] **Step 3: Export stream module in ur_rpc/src/lib.rs**

```rust
#[cfg(feature = "stream")]
pub mod stream;
```

- [ ] **Step 4: Update server to use ur_rpc::stream**

In `crates/server/Cargo.toml`, add `stream` to the ur_rpc features:
```toml
ur_rpc = { path = "../ur_rpc", features = ["core", "git", "gh", "stream"] }
```

In `crates/server/src/stream.rs`, replace the function body with a re-export:
```rust
pub use ur_rpc::stream::spawn_child_output_stream;
```

- [ ] **Step 5: Verify compilation**

Run: `cargo build -p ur-server`
Expected: Compiles successfully, existing git/gh handlers still work

- [ ] **Step 6: Commit**

```
refactor(ur_rpc): extract spawn_child_output_stream to shared crate (ur-7jle)
```

---

### Task 3: ur_config Additions

**Files:**
- Modify: `crates/ur_config/src/lib.rs`

- [ ] **Step 1: Add constants and config fields**

Add constants:
```rust
pub const DEFAULT_HOSTD_PORT: u16 = 42070;
pub const HOSTD_PID_FILE: &str = "hostd.pid";
pub const HOSTD_ADDR_ENV: &str = "UR_HOSTD_ADDR";
pub const HOSTEXEC_DIR: &str = "hostexec";
pub const HOSTEXEC_ALLOWLIST_FILE: &str = "allowlist.toml";
```

Add `hostd_port` field to `RawConfig`:
```rust
pub hostd_port: Option<u16>,
```

Add `hostd_port` field to `Config`:
```rust
pub hostd_port: u16,
```

Add to `Config` construction (in `load_from`):
```rust
hostd_port: raw.hostd_port.unwrap_or(DEFAULT_HOSTD_PORT),
```

Add helper method to `Config`:
```rust
pub fn hostexec_dir(&self) -> PathBuf {
    self.config_dir.join(HOSTEXEC_DIR)
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo build -p ur_config`
Expected: Compiles successfully

- [ ] **Step 3: Commit**

```
feat(ur_config): add hostd and hostexec configuration (ur-7jle)
```

---

## Chunk 2: ur-hostd

### Task 4: ur-hostd Crate

**Files:**
- Create: `crates/hostd/Cargo.toml`
- Create: `crates/hostd/src/main.rs`
- Create: `crates/hostd/src/handler.rs`
- Create: `crates/hostd/CLAUDE.md`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Add to workspace members**

In root `Cargo.toml`, add `"crates/hostd"` to both `members` and `default-members` arrays.

- [ ] **Step 2: Create Cargo.toml**

```toml
# crates/hostd/Cargo.toml
[package]
name = "ur-hostd"
edition.workspace = true
version.workspace = true

[dependencies]
anyhow = "1"
clap = { workspace = true }
prost = { workspace = true }
tokio = { workspace = true }
tokio-stream = { workspace = true }
tonic = { workspace = true }
tracing = "0.1"
tracing-subscriber = "0.3"
ur_config = { path = "../ur_config" }
ur_rpc = { path = "../ur_rpc", features = ["core", "hostd", "stream"] }
```

- [ ] **Step 3: Create CLAUDE.md**

```markdown
# ur-hostd

Host daemon for executing commands on the macOS host. Receives already-validated
requests from ur-server and spawns processes locally.

- Standalone binary, runs natively on host (not containerized)
- tonic gRPC server on `127.0.0.1:<hostd_port>`
- Trusts ur-server completely — no command validation
- Started/stopped by `ur start`/`ur stop`, PID tracked at `~/.ur/hostd.pid`
```

- [ ] **Step 4: Write failing test for handler**

Create `crates/hostd/src/handler.rs`:

```rust
use std::pin::Pin;
use std::process::Stdio;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::info;

use ur_rpc::proto::core::CommandOutput;
use ur_rpc::proto::hostd::host_daemon_service_server::HostDaemonService;
use ur_rpc::proto::hostd::HostDaemonExecRequest;

type CommandOutputStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<CommandOutput, Status>> + Send>>;

#[derive(Clone)]
pub struct HostDaemonHandler;

#[tonic::async_trait]
impl HostDaemonService for HostDaemonHandler {
    type ExecStream = CommandOutputStream;

    async fn exec(
        &self,
        req: Request<HostDaemonExecRequest>,
    ) -> Result<Response<Self::ExecStream>, Status> {
        let req = req.into_inner();

        info!(command = req.command, working_dir = req.working_dir, "host exec");

        let child = tokio::process::Command::new(&req.command)
            .args(&req.args)
            .current_dir(&req.working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Status::internal(format!("failed to spawn {}: {e}", req.command)))?;

        let (tx, rx) = mpsc::channel(32);
        ur_rpc::stream::spawn_child_output_stream(child, tx);

        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream) as Self::ExecStream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_stream::StreamExt;
    use ur_rpc::proto::core::command_output::Payload;

    #[tokio::test]
    async fn test_exec_echo() {
        let handler = HostDaemonHandler;
        let req = Request::new(HostDaemonExecRequest {
            command: "echo".into(),
            args: vec!["hello".into()],
            working_dir: "/tmp".into(),
        });

        let resp = handler.exec(req).await.unwrap();
        let mut stream = resp.into_inner();

        let mut stdout_data = Vec::new();
        let mut exit_code = None;

        while let Some(Ok(msg)) = stream.next().await {
            if let Some(payload) = msg.payload {
                match payload {
                    Payload::Stdout(data) => stdout_data.extend(data),
                    Payload::ExitCode(code) => exit_code = Some(code),
                    _ => {}
                }
            }
        }

        assert_eq!(String::from_utf8_lossy(&stdout_data).trim(), "hello");
        assert_eq!(exit_code, Some(0));
    }

    #[tokio::test]
    async fn test_exec_nonexistent_command() {
        let handler = HostDaemonHandler;
        let req = Request::new(HostDaemonExecRequest {
            command: "nonexistent_command_xyz".into(),
            args: vec![],
            working_dir: "/tmp".into(),
        });

        let result = handler.exec(req).await;
        assert!(result.is_err());
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p ur-hostd`
Expected: Both tests pass

- [ ] **Step 6: Write main.rs**

```rust
use std::net::SocketAddr;

use clap::Parser;
use tonic::transport::Server;
use tracing::info;

use ur_rpc::proto::hostd::host_daemon_service_server::HostDaemonServiceServer;

mod handler;

#[derive(Parser)]
#[command(name = "ur-hostd", about = "Ur host execution daemon")]
struct Cli {
    #[arg(long, default_value_t = ur_config::DEFAULT_HOSTD_PORT)]
    port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    let addr = SocketAddr::from(([127, 0, 0, 1], cli.port));
    info!(%addr, "ur-hostd starting");

    Server::builder()
        .add_service(HostDaemonServiceServer::new(handler::HostDaemonHandler))
        .serve(addr)
        .await?;

    Ok(())
}
```

- [ ] **Step 7: Verify build**

Run: `cargo build -p ur-hostd`
Expected: Compiles successfully

- [ ] **Step 8: Commit**

```
feat(hostd): add ur-hostd host execution daemon (ur-7jle)
```

---

## Chunk 3: HostExec Config & Lua

### Task 5: HostExec Configuration with Lua Transforms

**Files:**
- Modify: `crates/server/Cargo.toml`
- Create: `crates/server/src/hostexec/mod.rs`
- Create: `crates/server/src/hostexec/config.rs`
- Create: `crates/server/src/hostexec/lua_transform.rs`
- Create: `crates/server/src/hostexec/default_scripts/git.lua`
- Create: `crates/server/src/hostexec/default_scripts/gh.lua`
- Modify: `crates/server/src/lib.rs`

- [ ] **Step 1: Add mlua dependency**

In root `Cargo.toml` `[workspace.dependencies]`:
```toml
mlua = { version = "0.10", features = ["lua54", "vendored"] }
```

In `crates/server/Cargo.toml` `[dependencies]`:
```toml
mlua = { workspace = true }
serde = { workspace = true }
toml = "0.8"
```

Add to `[features]`:
```toml
hostexec = ["ur_rpc/hostexec"]
```

Add `hostexec` to the `default` feature list.

- [ ] **Step 2: Create default git.lua script**

```lua
-- crates/server/src/hostexec/default_scripts/git.lua
-- Default git argument transform: blocks sandbox-escape flags

function transform(command, args, working_dir)
    local blocked_exact = {
        ["-C"] = true,
        ["--git-dir"] = true,
        ["--work-tree"] = true,
    }
    local blocked_prefix = {
        "--git-dir=",
        "--work-tree=",
    }
    local blocked_config_keys = {
        "core.worktree",
    }

    local i = 1
    while i <= #args do
        local arg = args[i]

        if blocked_exact[arg] then
            error("blocked flag: " .. arg)
        end

        for _, prefix in ipairs(blocked_prefix) do
            if arg:sub(1, #prefix) == prefix then
                error("blocked flag: " .. arg)
            end
        end

        -- Check -c key=value for blocked config keys
        if arg == "-c" and i + 1 <= #args then
            local config_val = args[i + 1]:lower()
            for _, key in ipairs(blocked_config_keys) do
                if config_val:sub(1, #key) == key:lower() then
                    error("blocked config key: " .. key)
                end
            end
        end
        if arg:sub(1, 2) == "-c" and #arg > 2 then
            local config_val = arg:sub(3):lower()
            for _, key in ipairs(blocked_config_keys) do
                if config_val:sub(1, #key) == key:lower() then
                    error("blocked config key: " .. key)
                end
            end
        end

        i = i + 1
    end

    return args
end
```

- [ ] **Step 3: Create default gh.lua script**

```lua
-- crates/server/src/hostexec/default_scripts/gh.lua
-- Default gh argument transform: passthrough (no blocked flags)

function transform(command, args, working_dir)
    return args
end
```

- [ ] **Step 4: Write config module with tests**

Create `crates/server/src/hostexec/config.rs`:

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
struct RawAllowlist {
    #[serde(default)]
    commands: HashMap<String, RawCommandConfig>,
}

#[derive(Debug, Deserialize, Default)]
struct RawCommandConfig {
    #[serde(default)]
    lua: Option<String>,
    #[serde(default)]
    default_script: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct CommandConfig {
    pub lua_source: Option<String>,
}

#[derive(Clone)]
pub struct HostExecConfigManager {
    commands: HashMap<String, CommandConfig>,
}

impl HostExecConfigManager {
    pub fn load(config_dir: &Path) -> Result<Self> {
        let mut commands = Self::defaults();

        let allowlist_path = config_dir
            .join(ur_config::HOSTEXEC_DIR)
            .join(ur_config::HOSTEXEC_ALLOWLIST_FILE);

        if allowlist_path.exists() {
            let content = std::fs::read_to_string(&allowlist_path)
                .with_context(|| format!("reading {}", allowlist_path.display()))?;
            let raw: RawAllowlist = toml::from_str(&content)
                .with_context(|| format!("parsing {}", allowlist_path.display()))?;

            let hostexec_dir = config_dir.join(ur_config::HOSTEXEC_DIR);

            for (name, raw_cfg) in raw.commands {
                let lua_source = if raw_cfg.default_script.unwrap_or(false) {
                    Self::default_script(&name)
                } else if let Some(lua_file) = &raw_cfg.lua {
                    let lua_path = hostexec_dir.join(lua_file);
                    let src = std::fs::read_to_string(&lua_path)
                        .with_context(|| format!("reading lua script {}", lua_path.display()))?;
                    Some(src)
                } else {
                    None
                };

                commands.insert(name, CommandConfig { lua_source });
            }
        }

        Ok(Self { commands })
    }

    fn defaults() -> HashMap<String, CommandConfig> {
        let mut commands = HashMap::new();
        commands.insert(
            "git".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/git.lua").into()),
            },
        );
        commands.insert(
            "gh".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/gh.lua").into()),
            },
        );
        commands
    }

    fn default_script(name: &str) -> Option<String> {
        match name {
            "git" => Some(include_str!("default_scripts/git.lua").into()),
            "gh" => Some(include_str!("default_scripts/gh.lua").into()),
            _ => None,
        }
    }

    pub fn is_allowed(&self, command: &str) -> bool {
        self.commands.contains_key(command)
    }

    pub fn get(&self, command: &str) -> Option<&CommandConfig> {
        self.commands.get(command)
    }

    pub fn command_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.commands.keys().cloned().collect();
        names.sort();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_defaults_include_git_and_gh() {
        let tmp = TempDir::new().unwrap();
        let mgr = HostExecConfigManager::load(tmp.path()).unwrap();

        assert!(mgr.is_allowed("git"));
        assert!(mgr.is_allowed("gh"));
        assert!(!mgr.is_allowed("tk"));
        assert_eq!(mgr.command_names(), vec!["gh", "git"]);
    }

    #[test]
    fn test_user_config_extends_defaults() {
        let tmp = TempDir::new().unwrap();
        let hostexec_dir = tmp.path().join(ur_config::HOSTEXEC_DIR);
        fs::create_dir_all(&hostexec_dir).unwrap();
        fs::write(
            hostexec_dir.join(ur_config::HOSTEXEC_ALLOWLIST_FILE),
            "[commands]\ntk = {}\n",
        )
        .unwrap();

        let mgr = HostExecConfigManager::load(tmp.path()).unwrap();

        assert!(mgr.is_allowed("git"));
        assert!(mgr.is_allowed("gh"));
        assert!(mgr.is_allowed("tk"));
        assert!(mgr.get("tk").unwrap().lua_source.is_none());
    }

    #[test]
    fn test_user_config_overrides_default_with_custom_lua() {
        let tmp = TempDir::new().unwrap();
        let hostexec_dir = tmp.path().join(ur_config::HOSTEXEC_DIR);
        fs::create_dir_all(&hostexec_dir).unwrap();
        fs::write(
            hostexec_dir.join(ur_config::HOSTEXEC_ALLOWLIST_FILE),
            "[commands]\ngit = { lua = \"my-git.lua\" }\n",
        )
        .unwrap();
        fs::write(
            hostexec_dir.join("my-git.lua"),
            "function transform(c, a, w) return a end",
        )
        .unwrap();

        let mgr = HostExecConfigManager::load(tmp.path()).unwrap();

        let git_cfg = mgr.get("git").unwrap();
        assert!(git_cfg.lua_source.as_ref().unwrap().contains("return a"));
    }

    #[test]
    fn test_default_script_flag() {
        let tmp = TempDir::new().unwrap();
        let hostexec_dir = tmp.path().join(ur_config::HOSTEXEC_DIR);
        fs::create_dir_all(&hostexec_dir).unwrap();
        fs::write(
            hostexec_dir.join(ur_config::HOSTEXEC_ALLOWLIST_FILE),
            "[commands]\ngit = { default_script = true }\n",
        )
        .unwrap();

        let mgr = HostExecConfigManager::load(tmp.path()).unwrap();

        let git_cfg = mgr.get("git").unwrap();
        assert!(git_cfg.lua_source.as_ref().unwrap().contains("blocked"));
    }
}
```

- [ ] **Step 5: Run config tests**

Run: `cargo test -p ur-server hostexec::config`
Expected: All 4 tests pass

- [ ] **Step 6: Write Lua transform module with tests**

Create `crates/server/src/hostexec/lua_transform.rs`:

```rust
use anyhow::{Context, Result};
use mlua::{Lua, StdLib, Value};

#[derive(Clone)]
pub struct LuaTransformManager {
    // Lua VM is not Clone; create per-request or use a pool.
    // For simplicity, store scripts and create Lua VMs per-request.
    // Scripts are small and Lua VM creation is cheap.
}

impl LuaTransformManager {
    pub fn new() -> Self {
        Self {}
    }

    pub fn run_transform(
        &self,
        lua_source: &str,
        command: &str,
        args: &[String],
        working_dir: &str,
    ) -> Result<Vec<String>> {
        let lua = Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8,
            mlua::LuaOptions::default(),
        )
        .context("creating lua vm")?;

        lua.load(lua_source)
            .exec()
            .context("loading lua script")?;

        let transform: mlua::Function = lua
            .globals()
            .get("transform")
            .context("lua script must define a transform function")?;

        let lua_args = lua.create_table()?;
        for (i, arg) in args.iter().enumerate() {
            lua_args.set(i + 1, arg.as_str())?;
        }

        let result = transform
            .call::<Value>((command, lua_args, working_dir))
            .context("lua transform failed")?;

        match result {
            Value::Table(tbl) => {
                let mut out = Vec::new();
                for i in 1..=tbl.len()? {
                    let val: String = tbl.get(i)?;
                    out.push(val);
                }
                Ok(out)
            }
            _ => anyhow::bail!("lua transform must return a table"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_passthrough_transform() {
        let mgr = LuaTransformManager::new();
        let script = "function transform(c, a, w) return a end";
        let result = mgr
            .run_transform(script, "git", &["status".into()], "/workspace")
            .unwrap();
        assert_eq!(result, vec!["status"]);
    }

    #[test]
    fn test_git_default_blocks_dash_c() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let result = mgr.run_transform(script, "git", &["-C".into(), "/tmp".into()], "/workspace");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked flag: -C"));
    }

    #[test]
    fn test_git_default_blocks_git_dir() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let result = mgr.run_transform(
            script,
            "git",
            &["--git-dir=/tmp".into(), "status".into()],
            "/workspace",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_git_default_blocks_worktree_config() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let result = mgr.run_transform(
            script,
            "git",
            &["-c".into(), "core.worktree=/tmp".into(), "status".into()],
            "/workspace",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_git_default_allows_normal_args() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let args: Vec<String> = vec!["commit".into(), "-m".into(), "hello".into()];
        let result = mgr
            .run_transform(script, "git", &args, "/workspace")
            .unwrap();
        assert_eq!(result, args);
    }

    #[test]
    fn test_sandbox_no_io_access() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(c, a, w)
                io.open("/etc/passwd", "r")
                return a
            end
        "#;
        let result = mgr.run_transform(script, "test", &[], "/tmp");
        assert!(result.is_err());
    }

    #[test]
    fn test_sandbox_no_os_access() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(c, a, w)
                os.execute("whoami")
                return a
            end
        "#;
        let result = mgr.run_transform(script, "test", &[], "/tmp");
        assert!(result.is_err());
    }
}
```

- [ ] **Step 7: Run Lua transform tests**

Run: `cargo test -p ur-server hostexec::lua_transform`
Expected: All 7 tests pass

- [ ] **Step 8: Create hostexec module**

Create `crates/server/src/hostexec/mod.rs`:

```rust
pub mod config;
pub mod lua_transform;

pub use config::HostExecConfigManager;
pub use lua_transform::LuaTransformManager;
```

Add to `crates/server/src/lib.rs`:
```rust
#[cfg(feature = "hostexec")]
pub mod hostexec;
```

- [ ] **Step 9: Run all server tests**

Run: `cargo test -p ur-server`
Expected: All tests pass

- [ ] **Step 10: Commit**

```
feat(server): add hostexec config with Lua transforms (ur-7jle)
```

---

## Chunk 4: ur-server HostExec Service

### Task 6: HostExec gRPC Handler

**Files:**
- Create: `crates/server/src/grpc_hostexec.rs`
- Modify: `crates/server/src/grpc_server.rs`
- Modify: `crates/server/src/grpc.rs`
- Modify: `crates/server/src/main.rs`
- Modify: `crates/server/Cargo.toml`

- [ ] **Step 1: Add hostd feature and dependencies to server**

In `crates/server/Cargo.toml`, add to `[features]`:
```toml
hostd = ["ur_rpc/hostd"]
```

Add `hostd` to the `default` feature list.

The server needs a gRPC client to connect to ur-hostd. This is already available through `ur_rpc` with the `hostd` feature (tonic generates both server and client code).

- [ ] **Step 2: Write HostExecServiceHandler**

Create `crates/server/src/grpc_hostexec.rs`:

```rust
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{info, warn};

use ur_rpc::proto::core::CommandOutput;
use ur_rpc::proto::hostd::host_daemon_service_client::HostDaemonServiceClient;
use ur_rpc::proto::hostd::HostDaemonExecRequest;
use ur_rpc::proto::hostexec::host_exec_service_server::HostExecService;
use ur_rpc::proto::hostexec::{
    HostExecRequest, ListHostExecCommandsRequest, ListHostExecCommandsResponse,
};

use crate::hostexec::{HostExecConfigManager, LuaTransformManager};
use crate::RepoRegistry;

type CommandOutputStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<CommandOutput, Status>> + Send>>;

#[derive(Clone)]
pub struct HostExecServiceHandler {
    pub config: HostExecConfigManager,
    pub lua: LuaTransformManager,
    pub repo_registry: Arc<RepoRegistry>,
    pub process_id: String,
    pub hostd_addr: String,
}

#[tonic::async_trait]
impl HostExecService for HostExecServiceHandler {
    type ExecStream = CommandOutputStream;

    async fn exec(
        &self,
        req: Request<HostExecRequest>,
    ) -> Result<Response<Self::ExecStream>, Status> {
        let req = req.into_inner();

        // 1. Allowlist check
        let cmd_config = self
            .config
            .get(&req.command)
            .ok_or_else(|| Status::permission_denied(format!("command not allowed: {}", req.command)))?;

        // 2. CWD mapping: /workspace prefix -> host workspace path
        let host_working_dir = self.map_working_dir(&req.working_dir)?;

        // 3. Lua transform (if configured)
        let args = if let Some(lua_source) = &cmd_config.lua_source {
            self.lua
                .run_transform(lua_source, &req.command, &req.args, &host_working_dir)
                .map_err(|e| Status::invalid_argument(format!("transform rejected: {e}")))?
        } else {
            req.args
        };

        info!(
            command = req.command,
            process_id = self.process_id,
            host_working_dir,
            "host exec"
        );

        // 4. Forward to ur-hostd
        let mut client = HostDaemonServiceClient::connect(self.hostd_addr.clone())
            .await
            .map_err(|e| Status::unavailable(format!("hostd unavailable: {e}")))?;

        let hostd_req = HostDaemonExecRequest {
            command: req.command,
            args,
            working_dir: host_working_dir,
        };

        let response = client
            .exec(hostd_req)
            .await
            .map_err(|e| Status::internal(format!("hostd exec failed: {e}")))?;

        // Stream hostd response back to worker
        let mut inbound = response.into_inner();
        let (tx, rx) = mpsc::channel(32);

        tokio::spawn(async move {
            while let Ok(Some(msg)) = inbound.message().await {
                if tx.send(Ok(msg)).await.is_err() {
                    break;
                }
            }
        });

        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream) as Self::ExecStream))
    }

    async fn list_commands(
        &self,
        _req: Request<ListHostExecCommandsRequest>,
    ) -> Result<Response<ListHostExecCommandsResponse>, Status> {
        let commands = self.config.command_names();
        Ok(Response::new(ListHostExecCommandsResponse { commands }))
    }
}

impl HostExecServiceHandler {
    fn map_working_dir(&self, container_dir: &str) -> Result<String, Status> {
        let host_base = self
            .repo_registry
            .resolve(&self.process_id)
            .map_err(Status::not_found)?;

        let host_base_str = host_base.to_string_lossy();

        // Replace /workspace prefix with host workspace path
        if let Some(suffix) = container_dir.strip_prefix("/workspace") {
            if suffix.is_empty() || suffix.starts_with('/') {
                let suffix = suffix.strip_prefix('/').unwrap_or(suffix);
                if suffix.is_empty() {
                    Ok(host_base_str.into_owned())
                } else {
                    Ok(format!("{host_base_str}/{suffix}"))
                }
            } else {
                Err(Status::invalid_argument(format!(
                    "invalid working_dir: {container_dir}"
                )))
            }
        } else {
            Err(Status::invalid_argument(format!(
                "working_dir must start with /workspace: {container_dir}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_registry(process_id: &str, path: &str) -> Arc<RepoRegistry> {
        let registry = Arc::new(RepoRegistry::new(PathBuf::from("/tmp")));
        registry.register_absolute(process_id, PathBuf::from(path));
        registry
    }

    #[test]
    fn test_map_working_dir_root() {
        let registry = test_registry("test", "/host/workspace/test");
        let handler = HostExecServiceHandler {
            config: HostExecConfigManager::load(std::path::Path::new("/nonexistent")).unwrap(),
            lua: LuaTransformManager::new(),
            repo_registry: registry,
            process_id: "test".into(),
            hostd_addr: "http://localhost:42070".into(),
        };

        let result = handler.map_working_dir("/workspace").unwrap();
        assert_eq!(result, "/host/workspace/test");
    }

    #[test]
    fn test_map_working_dir_subdir() {
        let registry = test_registry("test", "/host/workspace/test");
        let handler = HostExecServiceHandler {
            config: HostExecConfigManager::load(std::path::Path::new("/nonexistent")).unwrap(),
            lua: LuaTransformManager::new(),
            repo_registry: registry,
            process_id: "test".into(),
            hostd_addr: "http://localhost:42070".into(),
        };

        let result = handler.map_working_dir("/workspace/src/main").unwrap();
        assert_eq!(result, "/host/workspace/test/src/main");
    }

    #[test]
    fn test_map_working_dir_rejects_invalid() {
        let registry = test_registry("test", "/host/workspace/test");
        let handler = HostExecServiceHandler {
            config: HostExecConfigManager::load(std::path::Path::new("/nonexistent")).unwrap(),
            lua: LuaTransformManager::new(),
            repo_registry: registry,
            process_id: "test".into(),
            hostd_addr: "http://localhost:42070".into(),
        };

        assert!(handler.map_working_dir("/tmp").is_err());
        assert!(handler.map_working_dir("/workspacefoo").is_err());
    }
}
```

- [ ] **Step 3: Run handler tests**

Run: `cargo test -p ur-server grpc_hostexec`
Expected: All tests pass

- [ ] **Step 4: Register HostExecService in build_agent_routes**

In `crates/server/src/grpc_server.rs`, add a `hostexec` block to `build_agent_routes`. The handler needs the config, lua manager, and hostd address, so update the function signature to accept these. Add parameters:

```rust
fn build_agent_routes(
    core_handler: CoreServiceHandler,
    process_id: &str,
    #[cfg(feature = "hostexec")] hostexec_config: crate::hostexec::HostExecConfigManager,
    #[cfg(feature = "hostexec")] hostd_addr: String,
) -> Routes {
```

Add the hostexec service registration block:

```rust
#[cfg(feature = "hostexec")]
{
    use ur_rpc::proto::hostexec::host_exec_service_server::HostExecServiceServer;
    builder.add_service(HostExecServiceServer::new(
        crate::grpc_hostexec::HostExecServiceHandler {
            config: hostexec_config,
            lua: crate::hostexec::LuaTransformManager::new(),
            repo_registry: core_handler.repo_registry.clone(),
            process_id: process_id.to_owned(),
            hostd_addr,
        },
    ));
}
```

- [ ] **Step 5: Update serve_agent_grpc to pass new parameters**

Update `serve_agent_grpc` to accept and forward the hostexec config and hostd address to `build_agent_routes`. Thread these values through from `CoreServiceHandler` — add `hostexec_config: HostExecConfigManager` and `hostd_addr: String` fields to `CoreServiceHandler`.

- [ ] **Step 6: Update main.rs to load config and pass hostd_addr**

In `crates/server/src/main.rs`:

```rust
// After Config::load()
let hostexec_config = ur_server::hostexec::HostExecConfigManager::load(&cfg.config_dir)
    .expect("failed to load hostexec config");

let hostd_addr = std::env::var(ur_config::HOSTD_ADDR_ENV)
    .unwrap_or_else(|_| format!("http://host.docker.internal:{}", cfg.hostd_port));
```

Add these as fields to `CoreServiceHandler`:

```rust
let grpc_handler = ur_server::grpc::CoreServiceHandler {
    process_manager,
    repo_registry,
    workspace: cfg.workspace,
    proxy_hostname: cfg.proxy.hostname,
    hostexec_config,
    hostd_addr,
};
```

- [ ] **Step 7: Add module declaration**

In `crates/server/src/lib.rs`, add:
```rust
#[cfg(feature = "hostexec")]
pub mod grpc_hostexec;
```

- [ ] **Step 8: Verify build**

Run: `cargo build -p ur-server`
Expected: Compiles successfully

- [ ] **Step 9: Run all server tests**

Run: `cargo test -p ur-server`
Expected: All tests pass

- [ ] **Step 10: Commit**

```
feat(server): add HostExec gRPC service with Lua validation (ur-7jle)
```

---

## Chunk 5: Worker Side

### Task 7: ur-tools Binary

**Files:**
- Create: `crates/workercmd/tools/Cargo.toml`
- Create: `crates/workercmd/tools/src/main.rs`
- Create: `crates/workercmd/tools/CLAUDE.md`

- [ ] **Step 1: Create Cargo.toml**

```toml
# crates/workercmd/tools/Cargo.toml
[package]
name = "workercmd-tools"
edition.workspace = true
version.workspace = true

[[bin]]
name = "ur-tools"
path = "src/main.rs"

[dependencies]
clap = { workspace = true }
tokio = { workspace = true }
tonic = { workspace = true }
ur_config = { path = "../../ur_config" }
ur_rpc = { path = "../../ur_rpc", features = ["core", "hostexec"] }
```

- [ ] **Step 2: Create CLAUDE.md**

```markdown
# workercmd-tools (ur-tools)

Unified worker binary for container-side commands. Installed at `/usr/local/bin/ur-tools`
in worker containers. Bash shims at `/home/worker/.local/bin/<command>` call
`ur-tools host-exec <command> <args>`.

- Connects to ur-server via `$UR_SERVER_ADDR`
- Streams `CommandOutput` to stdout/stderr in real time
- Exits with remote exit code
```

- [ ] **Step 3: Write main.rs**

```rust
use std::io::Write;

use clap::{Parser, Subcommand};
use tonic::transport::Endpoint;

use ur_rpc::proto::core::command_output::Payload;
use ur_rpc::proto::hostexec::host_exec_service_client::HostExecServiceClient;
use ur_rpc::proto::hostexec::HostExecRequest;

#[derive(Parser)]
#[command(name = "ur-tools", about = "Ur worker toolkit")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Execute a command on the host via ur-server
    HostExec {
        /// The command to execute
        command: String,
        /// Arguments to the command
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::HostExec { command, args } => {
            std::process::exit(run_host_exec(&command, args).await);
        }
    }
}

async fn run_host_exec(command: &str, args: Vec<String>) -> i32 {
    let server_addr =
        std::env::var(ur_config::UR_SERVER_ADDR_ENV).expect("UR_SERVER_ADDR must be set");
    let addr = format!("http://{server_addr}");

    let channel = match Endpoint::try_from(addr).unwrap().connect().await {
        Ok(ch) => ch,
        Err(e) => {
            eprintln!("{command}: failed to connect to ur server: {e}");
            return 1;
        }
    };

    let mut client = HostExecServiceClient::new(channel);

    let working_dir = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/workspace".into());

    let response = match client
        .exec(HostExecRequest {
            command: command.into(),
            args,
            working_dir,
        })
        .await
    {
        Ok(resp) => resp,
        Err(status) => {
            eprintln!("{command}: {}", status.message());
            return 1;
        }
    };

    let mut stream = response.into_inner();
    let mut exit_code = 1;

    while let Ok(Some(msg)) = stream.message().await {
        let Some(payload) = msg.payload else {
            continue;
        };
        match payload {
            Payload::Stdout(data) => {
                let _ = std::io::stdout().write_all(&data);
                let _ = std::io::stdout().flush();
            }
            Payload::Stderr(data) => {
                let _ = std::io::stderr().write_all(&data);
                let _ = std::io::stderr().flush();
            }
            Payload::ExitCode(code) => exit_code = code,
        }
    }

    exit_code
}
```

- [ ] **Step 4: Verify build**

Run: `cargo build -p workercmd-tools`
Expected: Compiles successfully

- [ ] **Step 5: Commit**

```
feat(workercmd): add ur-tools binary with host-exec subcommand (ur-7jle)
```

---

### Task 8: ur-workerd Shim Generator

**Files:**
- Create: `crates/workercmd/workerd/Cargo.toml`
- Create: `crates/workercmd/workerd/src/main.rs`
- Create: `crates/workercmd/workerd/CLAUDE.md`

- [ ] **Step 1: Create Cargo.toml**

```toml
# crates/workercmd/workerd/Cargo.toml
[package]
name = "ur-workerd"
edition.workspace = true
version.workspace = true

[dependencies]
anyhow = "1"
tokio = { workspace = true }
tonic = { workspace = true }
tracing = "0.1"
tracing-subscriber = "0.3"
ur_config = { path = "../../ur_config" }
ur_rpc = { path = "../../ur_rpc", features = ["core", "hostexec"] }
```

- [ ] **Step 2: Create CLAUDE.md**

```markdown
# ur-workerd

Worker daemon running inside containers. Queries ur-server for available host-exec
commands and creates bash shims in `/home/worker/.local/bin/`.

- Started by container entrypoint as a background process
- Calls `ListHostExecCommands` RPC on ur-server at startup
- Generates shims that call `ur-tools host-exec <command> "$@"`
- Retries with backoff if ur-server is not ready
- Stays running for future daemon uses
```

- [ ] **Step 3: Write main.rs**

```rust
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use tonic::transport::Endpoint;
use tracing::{info, warn};

use ur_rpc::proto::hostexec::host_exec_service_client::HostExecServiceClient;
use ur_rpc::proto::hostexec::ListHostExecCommandsRequest;

const SHIM_DIR: &str = ".local/bin";
const MAX_RETRIES: u32 = 30;
const INITIAL_BACKOFF_MS: u64 = 500;
const MAX_BACKOFF_MS: u64 = 5000;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let shim_dir = resolve_shim_dir();
    tokio::fs::create_dir_all(&shim_dir)
        .await
        .with_context(|| format!("creating shim dir {}", shim_dir.display()))?;

    let commands = fetch_commands_with_retry().await?;

    for command in &commands {
        create_shim(&shim_dir, command).await?;
    }

    info!(count = commands.len(), ?commands, "shims created");

    // Stay alive for future daemon uses
    loop {
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}

fn resolve_shim_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ur_config::WORKER_HOME.into());
    PathBuf::from(home).join(SHIM_DIR)
}

async fn fetch_commands_with_retry() -> Result<Vec<String>> {
    let server_addr =
        std::env::var(ur_config::UR_SERVER_ADDR_ENV).context("UR_SERVER_ADDR must be set")?;
    let addr = format!("http://{server_addr}");

    let mut backoff_ms = INITIAL_BACKOFF_MS;

    for attempt in 1..=MAX_RETRIES {
        match try_fetch_commands(&addr).await {
            Ok(commands) => return Ok(commands),
            Err(e) => {
                warn!(attempt, "failed to fetch commands: {e}");
                if attempt == MAX_RETRIES {
                    return Err(e).context("exhausted retries fetching command list");
                }
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
            }
        }
    }

    unreachable!()
}

async fn try_fetch_commands(addr: &str) -> Result<Vec<String>> {
    let channel = Endpoint::try_from(addr.to_string())?.connect().await?;
    let mut client = HostExecServiceClient::new(channel);
    let resp = client
        .list_commands(ListHostExecCommandsRequest {})
        .await?;
    Ok(resp.into_inner().commands)
}

async fn create_shim(shim_dir: &PathBuf, command: &str) -> Result<()> {
    let shim_path = shim_dir.join(command);
    let content = format!(
        "#!/bin/sh\nexec ur-tools host-exec {command} \"$@\"\n"
    );
    tokio::fs::write(&shim_path, &content)
        .await
        .with_context(|| format!("writing shim {}", shim_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        tokio::fs::set_permissions(&shim_path, perms)
            .await
            .with_context(|| format!("chmod shim {}", shim_path.display()))?;
    }

    info!(command, path = %shim_path.display(), "shim created");
    Ok(())
}
```

- [ ] **Step 4: Verify build**

Run: `cargo build -p ur-workerd`
Expected: Compiles successfully

- [ ] **Step 5: Commit**

```
feat(workercmd): add ur-workerd shim generator daemon (ur-7jle)
```

---

## Chunk 6: Lifecycle, Container & Cleanup

### Task 9: ur CLI hostd Lifecycle

**Files:**
- Modify: `crates/ur/src/main.rs`
- Modify: `crates/ur/Cargo.toml`

- [ ] **Step 1: Add hostd process management functions**

Add functions to `crates/ur/src/main.rs` (or a new `crates/ur/src/hostd.rs` module if main.rs is too long):

```rust
fn start_hostd(config: &ur_config::Config) -> Result<()> {
    let pid_file = config.config_dir.join(ur_config::HOSTD_PID_FILE);

    // Check for stale PID
    if pid_file.exists() {
        let pid_str = std::fs::read_to_string(&pid_file)?;
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            // Check if process is alive
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if alive {
                println!("ur-hostd already running (pid {pid})");
                return Ok(());
            }
            // Stale PID file
            std::fs::remove_file(&pid_file)?;
        }
    }

    let child = std::process::Command::new("ur-hostd")
        .args(["--port", &config.hostd_port.to_string()])
        .stdout(std::fs::File::create(config.config_dir.join("hostd.log"))?)
        .stderr(std::fs::File::create(config.config_dir.join("hostd.err"))?)
        .spawn()
        .context("failed to spawn ur-hostd — is it installed and on PATH?")?;

    std::fs::write(&pid_file, child.id().to_string())?;
    println!("ur-hostd started (pid {})", child.id());

    Ok(())
}

fn stop_hostd(config: &ur_config::Config) -> Result<()> {
    let pid_file = config.config_dir.join(ur_config::HOSTD_PID_FILE);

    if !pid_file.exists() {
        return Ok(());
    }

    let pid_str = std::fs::read_to_string(&pid_file)?;
    if let Ok(pid) = pid_str.trim().parse::<u32>() {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .output();
    }

    std::fs::remove_file(&pid_file)?;
    println!("ur-hostd stopped");

    Ok(())
}
```

- [ ] **Step 2: Update start command**

In the `Commands::Start` match arm, call `start_hostd` before `start_server`:

```rust
Commands::Start => {
    let config = load_config()?;
    start_hostd(&config)?;
    let compose = compose_manager_from_config(&config);
    start_server(&compose)?;
}
```

- [ ] **Step 3: Update stop command**

In the `Commands::Stop` match arm, call `stop_hostd` after stopping containers:

```rust
Commands::Stop => {
    let config = load_config()?;
    kill_all_containers()?;
    let compose = compose_manager_from_config(&config);
    stop_server(&compose)?;
    stop_hostd(&config)?;
}
```

- [ ] **Step 4: Verify build**

Run: `cargo build -p ur`
Expected: Compiles successfully

- [ ] **Step 5: Commit**

```
feat(ur): manage ur-hostd lifecycle in start/stop (ur-7jle)
```

---

### Task 10: Container & Compose Updates

**Files:**
- Modify: `containers/claude-worker/Dockerfile`
- Modify: `containers/claude-worker/entrypoint.sh`
- Modify: `containers/docker-compose.yml`

- [ ] **Step 1: Update worker Dockerfile**

Replace the baked-in git/gh binary COPY lines with ur-tools and ur-workerd. Remove `COPY git` and `COPY gh` lines. Add:

```dockerfile
COPY ur-tools /usr/local/bin/ur-tools
RUN chmod +x /usr/local/bin/ur-tools

COPY ur-workerd /usr/local/bin/ur-workerd
RUN chmod +x /usr/local/bin/ur-workerd

# Shim directory — ur-workerd writes shims here at startup
RUN mkdir -p /home/worker/.local/bin && chown worker:worker /home/worker/.local/bin
ENV PATH="/home/worker/.local/bin:${PATH}"
```

- [ ] **Step 2: Update entrypoint.sh**

```bash
#!/bin/bash
set -e

mkdir -p ~/.claude
mkdir -p ~/.local/bin

# Start ur-workerd in background (creates command shims)
ur-workerd &

tmux -u new-session -d -s agent
exec sleep infinity
```

- [ ] **Step 3: Update docker-compose.yml**

Add `HOSTD_ADDR` environment variable to the ur-server service and `extra_hosts` for Linux compatibility:

```yaml
  ur-server:
    # ... existing config ...
    environment:
      - UR_CONFIG=/config
      - UR_HOST_CONFIG=${UR_CONFIG:-~/.ur}
      - UR_HOSTD_ADDR=http://host.docker.internal:${UR_HOSTD_PORT:-42070}
      - GH_TOKEN=${GH_TOKEN:-}
      - GITHUB_TOKEN=${GITHUB_TOKEN:-}
    extra_hosts:
      - "host.docker.internal:host-gateway"
```

- [ ] **Step 4: Commit**

```
feat(containers): add ur-tools, ur-workerd, hostd networking (ur-7jle)
```

---

### Task 11: Cleanup — Remove git/gh Passthrough

**Files:**
- Remove: `proto/git.proto`
- Remove: `proto/gh.proto`
- Remove: `crates/server/src/grpc_git.rs`
- Remove: `crates/server/src/grpc_gh.rs`
- Remove: `crates/workercmd/git/` (entire directory)
- Remove: `crates/workercmd/gh/` (entire directory)
- Modify: `crates/ur_rpc/Cargo.toml`
- Modify: `crates/ur_rpc/build.rs`
- Modify: `crates/ur_rpc/src/lib.rs`
- Modify: `crates/server/Cargo.toml`
- Modify: `crates/server/src/lib.rs`
- Modify: `crates/server/src/grpc_server.rs`
- Modify: `crates/server/src/git_exec.rs`
- Modify: `containers/server/Dockerfile`

- [ ] **Step 1: Remove proto files**

Delete `proto/git.proto` and `proto/gh.proto`.

- [ ] **Step 2: Remove workercmd binaries**

Delete entire directories `crates/workercmd/git/` and `crates/workercmd/gh/`.

- [ ] **Step 3: Remove git/gh features from ur_rpc**

In `crates/ur_rpc/Cargo.toml`, remove `git = []` and `gh = []` from `[features]`.

In `crates/ur_rpc/build.rs`, remove the `#[cfg(feature = "git")]` and `#[cfg(feature = "gh")]` compilation blocks.

In `crates/ur_rpc/src/lib.rs`, remove the `#[cfg(feature = "git")]` and `#[cfg(feature = "gh")]` module declarations.

- [ ] **Step 4: Remove git/gh from server**

In `crates/server/Cargo.toml`:
- Remove `git = ["ur_rpc/git"]` and `gh = ["ur_rpc/gh"]` from `[features]`
- Remove `"git"` and `"gh"` from the `default` feature list

In `crates/server/src/lib.rs`:
- Remove `#[cfg(feature = "gh")] pub mod grpc_gh;`
- Remove `#[cfg(feature = "git")] pub mod grpc_git;`

Delete `crates/server/src/grpc_git.rs` and `crates/server/src/grpc_gh.rs`.

In `crates/server/src/grpc_server.rs`:
- Remove the `#[cfg(feature = "git")]` and `#[cfg(feature = "gh")]` service registration blocks from `build_agent_routes`

- [ ] **Step 5: Clean up git_exec.rs**

Remove `validate_args`, `run_git`, `exec_git`, and the `GitResponse` struct from `crates/server/src/git_exec.rs`. Keep `RepoRegistry` (still used for CWD mapping). If the file becomes trivially small, consider renaming it to `registry.rs` and updating `lib.rs`.

- [ ] **Step 6: Remove git/github-cli from server Dockerfile**

In `containers/server/Dockerfile`, remove `git` and `github-cli` from the `apk add` line:

```dockerfile
RUN apk add --no-cache \
    docker-cli \
    ca-certificates \
    tini \
    netcat-openbsd
```

- [ ] **Step 7: Verify full build**

Run: `cargo make ci`
Expected: fmt, clippy, build, and all tests pass

- [ ] **Step 8: Commit**

```
refactor: remove git/gh passthrough, replaced by hostexec (ur-7jle)
```

---

### Task 12: ur-pc18 Tickets for -C Handling

**Files:** None (ticket management only)

- [ ] **Step 1: Create ticket for git -C nested repo support**

```
tk create "git -C nested repo name mapping" --parent ur-pc18 -t task -p 2 -a "Christian Maher" --tags "ur3"
```

Add note: "When processing git -C <path>, extract the final path component (repo name) and map it to the mounted repo path. Currently -C is stripped entirely by the default git.lua transform. See hostexec design: docs/plans/2026-03-10-hostexec-ur-7jle-design.md"

- [ ] **Step 2: Commit ticket**

Tickets are managed outside git (symlinked .tickets/), so no git commit needed.

---

### Task 13: Integration Codeflow Doc

**Files:**
- Create: `docs/codeflows/host-exec-flow.md`

- [ ] **Step 1: Write codeflow document**

Document the full worker -> ur-server -> ur-hostd pipeline:

```markdown
# Host Exec Flow (ur-7jle)

## Overview

Workers execute host commands (git, gh, tk, etc.) through a three-hop gRPC pipeline
with Lua-based validation and CWD mapping.

## Flow

1. Worker calls `git status` (or any configured command)
2. Bash shim at `/home/worker/.local/bin/git` runs `ur-tools host-exec git status`
3. `ur-tools` captures CWD, sends `HostExecRequest` to ur-server (per-agent gRPC)
4. ur-server `HostExecServiceHandler`:
   a. Checks command against merged allowlist (defaults + ~/.ur/hostexec/allowlist.toml)
   b. Maps CWD: /workspace/... -> host workspace path via RepoRegistry
   c. Runs Lua transform if configured (validates/modifies args)
   d. Forwards `HostDaemonExecRequest` to ur-hostd
5. ur-hostd spawns the actual process on the host, streams CommandOutput
6. Output streams back: ur-hostd -> ur-server -> ur-tools -> stdout/stderr

## Shim Generation

At container startup, ur-workerd calls `ListHostExecCommands` on ur-server and
creates bash shims in `/home/worker/.local/bin/` (on PATH, writable by worker user).

## Configuration

- Built-in defaults: git (with git.lua), gh (with gh.lua)
- User extensions: ~/.ur/hostexec/allowlist.toml
- Custom Lua scripts: ~/.ur/hostexec/<name>.lua
- Passthrough commands: `command = {}` in allowlist (no Lua transform)

## Key Files

- Proto: proto/hostexec.proto, proto/hostd.proto
- Server handler: crates/server/src/grpc_hostexec.rs
- Config: crates/server/src/hostexec/
- Host daemon: crates/hostd/
- Worker tools: crates/workercmd/tools/, crates/workercmd/workerd/
```

- [ ] **Step 2: Commit**

```
docs: add host-exec codeflow documentation (ur-7jle)
```

---

### Task 14: Init Command Updates

**Files:**
- Modify: `crates/ur/src/init.rs`

- [ ] **Step 1: Add hostexec directory initialization**

In the `run_in` function, add creation of the hostexec config directory:

```rust
let hostexec_dir = config_dir.join(ur_config::HOSTEXEC_DIR);
init_dir(&hostexec_dir)?;
```

This ensures `~/.ur/hostexec/` exists for users to place their `allowlist.toml` and custom Lua scripts.

- [ ] **Step 2: Verify build**

Run: `cargo build -p ur`
Expected: Compiles successfully

- [ ] **Step 3: Commit**

```
feat(ur): init creates hostexec config directory (ur-7jle)
```
