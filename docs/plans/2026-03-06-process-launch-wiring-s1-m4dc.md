# Process Launch Wiring (s1-m4dc) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** `ur process launch <ticket>` creates an isolated agent with its own per-agent socket, registered repo, and git-initialized workspace — so `agent_tools git status` works inside the container.

**Architecture:** New `process_launch` and `process_stop` RPCs on urd that orchestrate the full agent lifecycle: create per-agent socket + accept_loop, register in RepoRegistry, git init repo dir, then run the container with the per-agent socket mounted. `ur` calls these instead of raw `container_run`/`container_stop`/`container_rm`. The `container_*` RPCs remain as generic building blocks.

**Tech Stack:** Rust, tarpc, tokio, Unix domain sockets, container crate

---

### Task 1: Add `ProcessLaunchRequest`/`ProcessLaunchResponse` and `ProcessStopRequest` to ur_rpc

**Files:**
- Modify: `crates/ur_rpc/src/lib.rs`

**Step 1: Add request/response types and trait methods**

Add after the existing `ContainerExecResponse` struct (around line 148):

```rust
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProcessLaunchRequest {
    pub process_id: String,
    pub image_id: String,
    pub cpus: u32,
    pub memory: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProcessLaunchResponse {
    pub container_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProcessStopRequest {
    pub process_id: String,
}
```

Add to the `UrAgentBridge` trait (around line 176):

```rust
async fn process_launch(req: ProcessLaunchRequest) -> Result<ProcessLaunchResponse, String>;
async fn process_stop(req: ProcessStopRequest) -> Result<(), String>;
```

**Step 2: Run `cargo check -p ur_rpc`**

Expected: SUCCESS (types compile, trait updated). Downstream crates (`urd`, `ur`, `agent_tools`) will fail until they implement/use the new methods.

**Step 3: Commit**

```
feat(ur_rpc): add process_launch and process_stop RPC types (s1-m4dc)
```

---

### Task 2: Add `ProcessManager` to urd

This is the core orchestration logic. A new module `crates/urd/src/process.rs` with a `ProcessManager` that handles per-agent socket spawning, repo registration, git init, container lifecycle, and cleanup.

**Files:**
- Create: `crates/urd/src/process.rs`
- Modify: `crates/urd/src/main.rs` (add `mod process;`)
- Modify: `crates/urd/src/git_exec.rs` (remove `#[allow(dead_code)]` from `unregister`)

**Step 1: Write tests for ProcessManager**

In `crates/urd/src/process.rs`, write the module with tests at the bottom. The tests validate:
- `launch` creates the repo dir, git-inits it, registers in RepoRegistry, creates the per-agent socket
- `stop` unregisters from RepoRegistry and removes the per-agent socket
- Double-launch with same process_id returns an error
- Stop of unknown process_id returns an error

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use tokio::task::JoinHandle;
use tracing::info;

use crate::RepoRegistry;

/// Tracks a running agent process.
struct ProcessEntry {
    container_id: String,
    socket_path: PathBuf,
    /// Handle to the per-agent accept_loop task.
    accept_handle: JoinHandle<()>,
}

/// Orchestrates the full lifecycle of agent processes:
/// per-agent socket, repo registration, git init, container run/stop.
#[derive(Clone)]
pub struct ProcessManager {
    config_dir: PathBuf,
    workspace: PathBuf,
    repo_registry: Arc<RepoRegistry>,
    processes: Arc<RwLock<HashMap<String, ProcessEntry>>>,
}

impl ProcessManager {
    pub fn new(config_dir: PathBuf, workspace: PathBuf, repo_registry: Arc<RepoRegistry>) -> Self {
        Self {
            config_dir,
            workspace,
            repo_registry,
            processes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Launch a new agent process. Creates per-agent socket, registers repo,
    /// git-inits the workspace dir, and starts the container.
    pub async fn launch(
        &self,
        process_id: &str,
        image_id: &str,
        cpus: u32,
        memory: &str,
        spawn_accept_loop: impl FnOnce(PathBuf, String) -> JoinHandle<()>,
    ) -> Result<String, String> {
        // Check for duplicate
        {
            let procs = self.processes.read().expect("process lock poisoned");
            if procs.contains_key(process_id) {
                return Err(format!("process already running: {process_id}"));
            }
        }

        // 1. Create + git init repo dir
        let repo_dir = self.workspace.join(process_id);
        tokio::fs::create_dir_all(&repo_dir)
            .await
            .map_err(|e| format!("failed to create repo dir: {e}"))?;

        let git_init = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(&repo_dir)
            .output()
            .await
            .map_err(|e| format!("failed to run git init: {e}"))?;
        if !git_init.status.success() {
            return Err(format!(
                "git init failed: {}",
                String::from_utf8_lossy(&git_init.stderr)
            ));
        }

        // 2. Register in RepoRegistry
        self.repo_registry.register(process_id, process_id);

        // 3. Create per-agent socket + accept_loop
        let socket_path = self.config_dir.join(format!("{process_id}.sock"));
        let accept_handle = spawn_accept_loop(socket_path.clone(), process_id.to_string());

        // 4. Wait briefly for the socket file to appear
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
        while !socket_path.exists() {
            if tokio::time::Instant::now() > deadline {
                return Err("per-agent socket did not appear within 5s".into());
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // 5. Run the container with the per-agent socket mounted
        let rt = container::runtime_from_env();
        let container_name = format!("ur-agent-{process_id}");
        let opts = container::RunOpts {
            image: container::ImageId(image_id.to_string()),
            name: container_name.clone(),
            cpus,
            memory: memory.to_string(),
            volumes: vec![],
            socket_mounts: vec![(socket_path.clone(), PathBuf::from("/var/run/ur.sock"))],
            workdir: Some(PathBuf::from("/workspace")),
            command: vec![],
        };
        let cid = rt.run(&opts).map_err(|e| e.to_string())?;

        info!(
            process_id,
            container_id = cid.0,
            "process launched"
        );

        // 6. Record in process map
        {
            let mut procs = self.processes.write().expect("process lock poisoned");
            procs.insert(
                process_id.to_string(),
                ProcessEntry {
                    container_id: cid.0.clone(),
                    socket_path,
                    accept_handle,
                },
            );
        }

        Ok(cid.0)
    }

    /// Stop a running agent process. Stops + removes the container,
    /// unregisters from RepoRegistry, tears down the per-agent socket.
    pub async fn stop(&self, process_id: &str) -> Result<(), String> {
        let entry = {
            let mut procs = self.processes.write().expect("process lock poisoned");
            procs
                .remove(process_id)
                .ok_or_else(|| format!("unknown process: {process_id}"))?
        };

        // 1. Stop + remove container
        let rt = container::runtime_from_env();
        let cid = container::ContainerId(entry.container_id);
        rt.stop(&cid).map_err(|e| e.to_string())?;
        rt.rm(&cid).map_err(|e| e.to_string())?;

        // 2. Unregister from RepoRegistry
        self.repo_registry.unregister(process_id);

        // 3. Abort the accept_loop task
        entry.accept_handle.abort();

        // 4. Remove the socket file
        let _ = tokio::fs::remove_file(&entry.socket_path).await;

        info!(process_id, "process stopped");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a ProcessManager with temp dirs. Returns (manager, config_dir, workspace_dir).
    fn test_manager() -> (ProcessManager, tempfile::TempDir, tempfile::TempDir) {
        let config_dir = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let registry = Arc::new(RepoRegistry::new(workspace.path().to_path_buf()));
        let mgr = ProcessManager::new(
            config_dir.path().to_path_buf(),
            workspace.path().to_path_buf(),
            registry.clone(),
        );
        (mgr, config_dir, workspace)
    }

    /// Fake accept_loop that just creates the socket file.
    fn fake_accept_loop(socket_path: PathBuf, _process_id: String) -> JoinHandle<()> {
        tokio::spawn(async move {
            // Bind the socket so the file appears
            let _listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
            // Keep it alive until task is aborted
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
        })
    }

    #[tokio::test]
    async fn launch_creates_repo_and_socket() {
        let (mgr, config_dir, workspace) = test_manager();

        // We can't run a real container in unit tests, so test the pre-container steps.
        // Create the socket via fake_accept_loop, verify repo + registry.
        let socket_path = config_dir.path().join("test-proc.sock");
        let handle = fake_accept_loop(socket_path.clone(), "test-proc".into());

        // Wait for socket
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(2);
        while !socket_path.exists() {
            if tokio::time::Instant::now() > deadline {
                panic!("socket did not appear");
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Verify repo dir + git init
        let repo_dir = workspace.path().join("test-proc");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        let git_init = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(&repo_dir)
            .output()
            .await
            .unwrap();
        assert!(git_init.status.success());

        // Register in registry
        mgr.repo_registry.register("test-proc", "test-proc");

        // Verify registry resolves
        let resolved = mgr.repo_registry.exec_git("test-proc", &["status".into()]).await;
        assert!(resolved.is_ok());

        handle.abort();
    }

    #[tokio::test]
    async fn stop_unknown_process_returns_error() {
        let (mgr, _config_dir, _workspace) = test_manager();
        let result = mgr.stop("nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown process"));
    }
}
```

**Step 2: Wire the module into urd/src/main.rs**

Add `mod process;` and `pub use process::ProcessManager;` near the top of `crates/urd/src/main.rs`.

**Step 3: Remove `#[allow(dead_code)]` from `unregister` in git_exec.rs**

`unregister` is now used by `ProcessManager::stop`.

**Step 4: Run `cargo test -p urd`**

Expected: New tests pass. The `launch_creates_repo_and_socket` test validates repo creation, git init, and registry without needing a real container.

**Step 5: Commit**

```
feat(urd): add ProcessManager for per-agent lifecycle (s1-m4dc)
```

---

### Task 3: Implement `process_launch` and `process_stop` in BridgeServer

**Files:**
- Modify: `crates/urd/src/main.rs`

**Step 1: Add `ProcessManager` to `BridgeServer` and implement the new trait methods**

Add `process_manager: ProcessManager` field to `BridgeServer`.

Implement the two new trait methods:

```rust
async fn process_launch(
    self,
    _ctx: tarpc::context::Context,
    req: ProcessLaunchRequest,
) -> Result<ProcessLaunchResponse, String> {
    let repo_registry = self.repo_registry.clone();
    let socket_dir = self.socket_dir.clone();
    let process_manager = self.process_manager.clone();

    let container_id = process_manager
        .launch(
            &req.process_id,
            &req.image_id,
            req.cpus,
            &req.memory,
            |socket_path, process_id| {
                let server = BridgeServer {
                    repo_registry,
                    socket_dir,
                    process_id,
                    process_manager,
                };
                tokio::spawn(async move {
                    if let Err(e) = accept_loop(socket_path, server).await {
                        tracing::warn!("per-agent accept_loop error: {e}");
                    }
                })
            },
        )
        .await?;

    Ok(ProcessLaunchResponse { container_id })
}

async fn process_stop(
    self,
    _ctx: tarpc::context::Context,
    req: ProcessStopRequest,
) -> Result<(), String> {
    self.process_manager.stop(&req.process_id).await
}
```

**Step 2: Update `main()` to create `ProcessManager` and pass it to `BridgeServer`**

```rust
let process_manager = ProcessManager::new(
    cfg.config_dir.clone(),
    cfg.workspace.clone(),
    repo_registry.clone(),
);

let server = BridgeServer {
    repo_registry,
    socket_dir: cfg.config_dir.clone(),
    process_id: String::new(),
    process_manager,
};
```

**Step 3: Run `cargo check -p urd`**

Expected: Compiles. The new trait methods are implemented.

**Step 4: Commit**

```
feat(urd): wire ProcessManager into BridgeServer RPC handlers (s1-m4dc)
```

---

### Task 4: Update bridge_test.rs stub

**Files:**
- Modify: `crates/urd/tests/bridge_test.rs`

**Step 1: Add stub implementations for the two new trait methods**

```rust
async fn process_launch(
    self,
    _ctx: context::Context,
    req: ProcessLaunchRequest,
) -> Result<ProcessLaunchResponse, String> {
    Ok(ProcessLaunchResponse {
        container_id: format!("ur-agent-{}", req.process_id),
    })
}

async fn process_stop(
    self,
    _ctx: context::Context,
    _req: ProcessStopRequest,
) -> Result<(), String> {
    Ok(())
}
```

**Step 2: Run `cargo test -p urd`**

Expected: All existing tests pass, plus the new ProcessManager tests.

**Step 3: Commit**

```
test(urd): add stub process_launch/process_stop to bridge_test (s1-m4dc)
```

---

### Task 5: Update `ur process launch` and `ur process stop` to use new RPCs

**Files:**
- Modify: `crates/ur/src/main.rs`

**Step 1: Rewrite `process_launch` to use `ProcessLaunchRequest`**

The function should:
1. Build the worker image (same as before)
2. Call `process_launch` RPC instead of `container_run`
3. No longer manually specify socket_mounts — urd handles it

```rust
async fn process_launch(client: &UrAgentBridgeClient, ticket_id: &str) -> Result<()> {
    let ctx = tarpc::context::current();

    // Build the worker image
    let project_root = std::env::current_dir()?;
    let context_dir = project_root.join("containers/claude-worker");
    println!("Building worker image...");
    let build_resp = client
        .container_build(
            ctx,
            ContainerBuildRequest {
                tag: "ur-worker:latest".into(),
                dockerfile: context_dir.join("Dockerfile").display().to_string(),
                context: context_dir.display().to_string(),
            },
        )
        .await?
        .map_err(|e| anyhow::anyhow!(e))?;

    // Launch the agent process (urd handles socket, repo, container)
    println!("Launching agent for {ticket_id}...");
    let launch_resp = client
        .process_launch(
            tarpc::context::current(),
            ProcessLaunchRequest {
                process_id: ticket_id.into(),
                image_id: build_resp.image_id,
                cpus: 4,
                memory: "8G".into(),
            },
        )
        .await?
        .map_err(|e| anyhow::anyhow!(e))?;

    let container_name = format!("ur-agent-{ticket_id}");
    println!("Agent {container_name} running (container {})", launch_resp.container_id);
    Ok(())
}
```

**Step 2: Rewrite `process_stop` to use `ProcessStopRequest`**

```rust
async fn process_stop(client: &UrAgentBridgeClient, process_id: &str) -> Result<()> {
    println!("Stopping {process_id}...");
    client
        .process_stop(
            tarpc::context::current(),
            ProcessStopRequest {
                process_id: process_id.into(),
            },
        )
        .await?
        .map_err(|e| anyhow::anyhow!(e))?;

    println!("Agent {process_id} stopped.");
    Ok(())
}
```

Note: `process stop` now takes the process_id (ticket_id), not the container name. The `ProcessCommands::Stop` variant should be updated accordingly — the user runs `ur process stop <ticket_id>`.

**Step 3: Run `cargo check -p ur`**

Expected: Compiles.

**Step 4: Commit**

```
feat(ur): use process_launch/process_stop RPCs (s1-m4dc)
```

---

### Task 6: Update agent_tools stub for new trait methods (compile fix)

**Files:**
- Modify: `crates/agent_tools/src/main.rs` — no changes needed since agent_tools is a client, not a server. It doesn't implement the trait.

Verify: `cargo check -p agent_tools` — should already compile since agent_tools only uses `UrAgentBridgeClient`.

---

### Task 7: Update acceptance test

**Files:**
- Modify: `crates/acceptance/tests/e2e.rs`

**Step 1: Update the test to use the new flow**

The acceptance test currently:
1. Calls `ur process launch` — still works (but now urd creates the repo + socket)
2. Manually creates a repo dir and git inits it — **remove this**, urd does it now
3. Tests `agent_tools git status` expecting "unknown process_id" — **change to expect success** (exit 0)
4. Calls `ur process stop` with container name — **change to use ticket_id**

Updated test section (replace lines ~156-205):

```rust
// ---- (5) Test git commands via agent_tools ----
// urd has already created and git-init'd the repo for this process.
// agent_tools git status should succeed via the per-agent socket.
let git_output = Command::new(&runtime)
    .args([
        "exec",
        &container_name,
        "agent_tools",
        "--socket",
        "/var/run/ur.sock",
        "git",
        "status",
    ])
    .output()
    .expect("failed to exec agent_tools git in container");

assert_eq!(
    git_output.status.code(),
    Some(0),
    "agent_tools git status should exit 0.\nstdout: {}\nstderr: {}",
    String::from_utf8_lossy(&git_output.stdout),
    String::from_utf8_lossy(&git_output.stderr),
);

let git_stdout = String::from_utf8_lossy(&git_output.stdout);
assert!(
    git_stdout.contains("branch") || git_stdout.contains("No commits"),
    "git status should show repo info.\nGot: {git_stdout}"
);

// ---- (6) ur process stop (by ticket_id, not container name) ----
let stop_output = run_cmd(
    &ur,
    &["--socket", socket_str, "process", "stop", ticket_id],
    &[],
);
```

**Step 2: Run `cargo check -p acceptance --features acceptance`**

Expected: Compiles. (Full acceptance test requires container runtime, only runs in CI.)

**Step 3: Commit**

```
test(acceptance): expect git success with per-agent sockets (s1-m4dc)
```

---

### Task 8: Full CI check

**Step 1: Run `cargo make ci`**

Expected: All fmt, clippy, build, test pass.

**Step 2: Fix any issues**

**Step 3: Final commit if needed, then push**

```
fix: address CI feedback (s1-m4dc)
```
