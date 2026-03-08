# Workspace Mounting Implementation Plan (ur-m3tk)

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `-w`/`--workspace` flag to `ur process launch` that mounts a host directory into the container, and bundle the `tk` script into the container image.

**Architecture:** The CLI resolves the workspace path to absolute, passes it through the proto to urd. Urd skips git-init when a workspace is provided and mounts it as a volume. ProcessEntry tracks whether the workspace is externally managed to skip cleanup on stop. The `tk` script is a staged build artifact like `ur-ping` and `git`.

**Tech Stack:** Rust, tonic/protobuf, clap, container runtimes (Apple/Docker/nerdctl)

---

### Task 1: Add `workspace_dir` to proto

**Files:**
- Modify: `proto/core.proto:26-31`

**Step 1: Add the field**

In `ProcessLaunchRequest`, add field 5:

```proto
message ProcessLaunchRequest {
  string process_id = 1;
  string image_id = 2;
  uint32 cpus = 3;
  string memory = 4;
  string workspace_dir = 5;
}
```

**Step 2: Verify it compiles**

Run: `cargo build --workspace --all-features`
Expected: PASS (tonic-build regenerates the code)

**Step 3: Commit**

```
feat(ur_rpc): add workspace_dir to ProcessLaunchRequest
```

---

### Task 2: Add `workspace_dir` to `ProcessConfig` and `ProcessEntry`

**Files:**
- Modify: `crates/urd/src/process.rs:19-27` (ProcessConfig)
- Modify: `crates/urd/src/process.rs:10-17` (ProcessEntry)

**Step 1: Write the test**

Add to `crates/urd/src/process.rs` in the `tests` module:

```rust
#[tokio::test]
async fn prepare_with_workspace_skips_git_init() {
    let (mgr, workspace) = test_manager();
    let process_id = "ext-workspace";

    // Create a fake external workspace directory
    let ext_dir = workspace.path().join("external-repo");
    std::fs::create_dir_all(&ext_dir).unwrap();

    mgr.prepare_with_workspace(process_id, Some(ext_dir.clone()))
        .await
        .unwrap();

    // Verify: no git-init happened (no .git in the external dir)
    // The external dir should NOT have .git created by prepare
    // (it may already have one from the user, but prepare shouldn't create one)
    // Instead check that the workspace subdirectory was NOT created
    let managed_dir = workspace.path().join(process_id);
    assert!(!managed_dir.exists(), "should not create managed dir when workspace provided");

    // Verify registry resolves to the external dir
    let resolved = mgr.repo_registry.resolve(process_id).unwrap();
    assert_eq!(resolved, ext_dir);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p urd prepare_with_workspace`
Expected: FAIL — `prepare_with_workspace` doesn't exist

**Step 3: Implement**

In `ProcessConfig`, add `workspace_dir`:

```rust
pub struct ProcessConfig {
    pub process_id: String,
    pub image_id: String,
    pub cpus: u32,
    pub memory: String,
    pub grpc_port: u16,
    pub host_ip: String,
    pub workspace_dir: Option<PathBuf>,
}
```

In `ProcessEntry`, add `externally_managed`:

```rust
struct ProcessEntry {
    container_id: String,
    grpc_port: u16,
    server_handle: JoinHandle<()>,
    externally_managed: bool,
}
```

Rename `prepare` to `prepare_with_workspace` (or keep `prepare` and add the parameter). Better: add the parameter to `prepare`:

```rust
pub async fn prepare(&self, process_id: &str, workspace_dir: Option<PathBuf>) -> Result<(), String> {
    // Check for duplicate (unchanged)
    {
        let procs = self.processes.read().expect("process lock poisoned");
        if procs.contains_key(process_id) {
            return Err(format!("process already running: {process_id}"));
        }
    }

    match workspace_dir {
        Some(ref dir) => {
            // External workspace: register the absolute path directly.
            // Use an empty string as repo_name; override resolve to handle absolute paths.
            self.repo_registry.register_absolute(process_id, dir);
        }
        None => {
            // Managed workspace: create repo dir, git init, register
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

            self.repo_registry.register(process_id, process_id);
        }
    }

    Ok(())
}
```

Update `run_and_record` to use `workspace_dir` for volumes and `externally_managed`:

```rust
pub async fn run_and_record(
    &self,
    config: ProcessConfig,
    server_handle: JoinHandle<()>,
) -> Result<String, String> {
    let urd_addr = format!("{}:{}", config.host_ip, config.grpc_port);

    let volumes = match config.workspace_dir {
        Some(ref dir) => vec![(dir.clone(), PathBuf::from("/workspace"))],
        None => vec![],
    };

    let cid = {
        let rt = container::runtime_from_env();
        let container_name = format!("ur-agent-{}", config.process_id);
        let opts = container::RunOpts {
            image: container::ImageId(config.image_id.clone()),
            name: container_name,
            cpus: config.cpus,
            memory: config.memory.clone(),
            volumes,
            port_maps: vec![],
            env_vars: vec![(ur_config::URD_ADDR_ENV.into(), urd_addr)],
            workdir: Some(PathBuf::from("/workspace")),
            command: vec![],
        };
        rt.run(&opts).map_err(|e| e.to_string())?
    };

    info!(
        process_id = config.process_id,
        container_id = cid.0,
        grpc_port = config.grpc_port,
        "process launched"
    );

    {
        let mut procs = self.processes.write().expect("process lock poisoned");
        procs.insert(
            config.process_id,
            ProcessEntry {
                container_id: cid.0.clone(),
                grpc_port: config.grpc_port,
                server_handle,
                externally_managed: config.workspace_dir.is_some(),
            },
        );
    }

    Ok(cid.0)
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p urd prepare_with_workspace`
Expected: PASS

**Step 5: Fix the existing `prepare` test call sites**

Update `prepare_creates_repo_and_registers` to pass `None`:

```rust
mgr.prepare(process_id, None).await.unwrap();
```

Update `prepare_duplicate_returns_error` — the `prepare` call:

```rust
let result = mgr.prepare("dup-proc", None).await;
```

**Step 6: Run all urd tests**

Run: `cargo test -p urd`
Expected: PASS

**Step 7: Commit**

```
feat(urd): add workspace_dir to ProcessConfig, skip git-init for external workspaces
```

---

### Task 3: Add `register_absolute` to `RepoRegistry`

**Files:**
- Modify: `crates/urd/src/git_exec.rs:21-66`

**Step 1: Write the tests**

Add to the `tests` module in `git_exec.rs`:

```rust
#[test]
fn registry_register_absolute() {
    let reg = RepoRegistry::new(PathBuf::from("/workspace"));
    reg.register_absolute("p1", &PathBuf::from("/external/repo"));
    let path = reg.resolve("p1").unwrap();
    assert_eq!(path, PathBuf::from("/external/repo"));
}

#[test]
fn registry_absolute_does_not_join_workspace() {
    let reg = RepoRegistry::new(PathBuf::from("/workspace"));
    reg.register_absolute("p1", &PathBuf::from("/other/path"));
    let path = reg.resolve("p1").unwrap();
    // Should NOT be /workspace//other/path
    assert_eq!(path, PathBuf::from("/other/path"));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p urd registry_register_absolute`
Expected: FAIL — `register_absolute` doesn't exist

**Step 3: Implement**

The cleanest approach: change the registry to store `PathBuf` values instead of `String` repo names. `register` constructs the full path from workspace + name. `register_absolute` stores the path directly. `resolve` just returns the stored path.

```rust
pub struct RepoRegistry {
    workspace: PathBuf,
    /// process_id → full repo directory path
    repos: RwLock<HashMap<String, PathBuf>>,
}

impl RepoRegistry {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            repos: RwLock::new(HashMap::new()),
        }
    }

    /// Register a process with a repo subdirectory relative to the workspace.
    pub fn register(&self, process_id: &str, repo_name: &str) {
        self.repos
            .write()
            .expect("repo registry lock poisoned")
            .insert(process_id.to_string(), self.workspace.join(repo_name));
    }

    /// Register a process with an absolute repo path (external workspace).
    pub fn register_absolute(&self, process_id: &str, path: &PathBuf) {
        self.repos
            .write()
            .expect("repo registry lock poisoned")
            .insert(process_id.to_string(), path.clone());
    }

    /// Remove a process from the registry.
    pub fn unregister(&self, process_id: &str) {
        self.repos
            .write()
            .expect("repo registry lock poisoned")
            .remove(process_id);
    }

    /// Resolve a process_id to its full repo path.
    pub(crate) fn resolve(&self, process_id: &str) -> Result<PathBuf, String> {
        let repos = self.repos.read().expect("repo registry lock poisoned");
        repos
            .get(process_id)
            .cloned()
            .ok_or_else(|| format!("unknown process_id: {process_id}"))
    }

    /// Validate and execute `git <args>` in the process's repo directory.
    pub async fn exec_git(&self, process_id: &str, args: &[String]) -> Result<GitResponse, String> {
        let repo_path = self.resolve(process_id)?;
        validate_args(args)?;
        run_git(&repo_path, args).await
    }
}
```

**Step 4: Run all urd tests**

Run: `cargo test -p urd`
Expected: PASS (existing tests like `registry_resolve_known_process` still work because `register("p1", "my-repo")` now stores `/workspace/my-repo`)

**Step 5: Commit**

```
refactor(urd): RepoRegistry stores full paths, add register_absolute
```

---

### Task 4: Wire `workspace_dir` through gRPC handler

**Files:**
- Modify: `crates/urd/src/grpc.rs:30-92`

**Step 1: Update `process_launch` handler**

Extract `workspace_dir` from the request and thread it through:

```rust
async fn process_launch(
    &self,
    req: Request<ProcessLaunchRequest>,
) -> Result<Response<ProcessLaunchResponse>, Status> {
    let req = req.into_inner();

    let workspace_dir = if req.workspace_dir.is_empty() {
        None
    } else {
        Some(PathBuf::from(&req.workspace_dir))
    };

    // Phase 1: prepare
    self.process_manager
        .prepare(&req.process_id, workspace_dir.clone())
        .await
        .map_err(Status::internal)?;

    // ... host_ip detection unchanged ...

    // ... per-agent gRPC server spawn unchanged ...

    // Phase 2: run container
    let config = crate::ProcessConfig {
        process_id: req.process_id,
        image_id: req.image_id,
        cpus: req.cpus,
        memory: req.memory,
        grpc_port,
        host_ip,
        workspace_dir,
    };
    let container_id = self
        .process_manager
        .run_and_record(config, server_handle)
        .await
        .map_err(Status::internal)?;

    Ok(Response::new(ProcessLaunchResponse { container_id }))
}
```

**Step 2: Verify compilation**

Run: `cargo build --workspace --all-features`
Expected: PASS

**Step 3: Commit**

```
feat(urd): wire workspace_dir from RPC through to container launch
```

---

### Task 5: Add `-w`/`--workspace` to CLI

**Files:**
- Modify: `crates/ur/src/main.rs:37-47` (ProcessCommands)
- Modify: `crates/ur/src/main.rs:90-119` (process_launch fn)

**Step 1: Add the flag to the enum**

```rust
#[derive(Subcommand)]
enum ProcessCommands {
    /// Launch a new agent process
    Launch {
        ticket_id: String,
        /// Mount a host directory as the container workspace
        #[arg(short = 'w', long = "workspace")]
        workspace: Option<PathBuf>,
    },
    // ... rest unchanged
}
```

**Step 2: Update `process_launch` to accept and send it**

```rust
async fn process_launch(
    client: &mut CoreServiceClient<Channel>,
    ticket_id: &str,
    workspace: Option<PathBuf>,
) -> Result<()> {
    // Resolve workspace to absolute path if provided
    let workspace_dir = match workspace {
        Some(p) => {
            let abs = std::fs::canonicalize(&p)
                .with_context(|| format!("workspace path does not exist: {}", p.display()))?;
            abs.to_string_lossy().into_owned()
        }
        None => String::new(),
    };

    // Build the container image
    let project_root = std::env::current_dir()?;
    let context_dir = project_root.join("containers/claude-worker");
    println!("Building worker image...");
    let rt = container::runtime_from_env();
    let image = rt.build(&container::BuildOpts {
        tag: "ur-worker:latest".into(),
        dockerfile: context_dir.join("Dockerfile"),
        context: context_dir.clone(),
    })?;

    let container_name = format!("ur-agent-{ticket_id}");
    println!("Launching agent {container_name}...");
    let resp = client
        .process_launch(ProcessLaunchRequest {
            process_id: ticket_id.into(),
            image_id: image.0,
            cpus: 2,
            memory: "8G".into(),
            workspace_dir,
        })
        .await?;

    println!(
        "Agent {container_name} running (container {})",
        resp.into_inner().container_id
    );
    Ok(())
}
```

**Step 3: Update the match arm that calls `process_launch`**

```rust
ProcessCommands::Launch { ticket_id, workspace } => {
    process_launch(&mut client, &ticket_id, workspace).await?;
}
```

**Step 4: Verify compilation**

Run: `cargo build --workspace --all-features`
Expected: PASS

**Step 5: Commit**

```
feat(ur): add -w/--workspace flag to process launch
```

---

### Task 6: Stage `tk` script and update Dockerfile

**Files:**
- Modify: `.gitignore` (add `containers/claude-worker/tk`)
- Modify: `containers/claude-worker/Dockerfile:19-22` (add COPY for tk)
- Modify: `Makefile.toml` (add `stage-tk` task)

**Step 1: Add `tk` to `.gitignore`**

Add after the existing `containers/claude-worker/git` line:

```
containers/claude-worker/tk
```

**Step 2: Add `stage-tk` task to `Makefile.toml`**

```toml
[tasks.stage-tk]
description = "Copy tk script to container build context"
script_runner = "@shell"
script = '''
DEST=containers/claude-worker
cp /opt/homebrew/bin/tk "$DEST/tk"
echo "Staged tk in $DEST/"
'''
```

**Step 3: Update `stage-workercmd` and `stage-workercmd-native` to also stage tk**

In `stage-workercmd`, after the `cp` lines, add:

```bash
# Copy tk script (temporary until ur-o79g replaces it)
if [ -f /opt/homebrew/bin/tk ]; then
    cp /opt/homebrew/bin/tk "$DEST/tk"
fi
```

In `stage-workercmd-native`, add the same but source from a PATH lookup since CI won't have homebrew:

```bash
# Copy tk script if available (CI may not have it)
if command -v tk >/dev/null 2>&1; then
    cp "$(command -v tk)" "$DEST/tk"
fi
```

**Step 4: Add COPY to Dockerfile**

After the `COPY git` line, add:

```dockerfile
# tk ticket manager (temporary, replaced by ur-o79g)
COPY tk /usr/local/bin/tk
RUN chmod +x /usr/local/bin/tk
```

**Step 5: Stage the file locally and verify build**

Run: `cp /opt/homebrew/bin/tk containers/claude-worker/tk`
Run: `cargo make stage-workercmd` (or just verify Dockerfile syntax)

**Step 6: Commit**

```
feat(container): add tk script to worker image
```

---

### Task 7: Update acceptance test for workspace mounting

**Files:**
- Modify: `crates/acceptance/tests/e2e.rs`

**Step 1: Add workspace mounting test**

Add a new test function after `e2e_ping_and_git`:

```rust
#[test]
fn e2e_workspace_mount() {
    let runtime = detect_container_runtime();
    let process_id = "ws-mount-test";
    let container_name = format!("ur-agent-{process_id}");
    force_remove_container(&runtime, &container_name);

    let config_dir = tempfile::tempdir().expect("failed to create temp config dir");
    let config_path = config_dir.path();
    let daemon_port = 19877u16;

    // Create a temp directory with a marker file to verify mounting
    let workspace_dir = tempfile::tempdir().expect("failed to create temp workspace");
    std::fs::write(workspace_dir.path().join("marker.txt"), "mounted").unwrap();
    // Initialize git so git commands work
    let _ = Command::new("git")
        .args(["init"])
        .current_dir(workspace_dir.path())
        .output()
        .unwrap();

    let (urd_child, port) = start_urd(config_path, daemon_port);
    let port_str = port.to_string();
    let ws_path = workspace_dir.path().to_str().unwrap().to_string();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let ur = bin("ur");

        // Launch with --workspace flag
        let launch_output = run_cmd(
            &ur,
            &["--port", &port_str, "process", "launch", process_id, "-w", &ws_path],
            &[("UR_CONFIG", config_path.to_str().unwrap())],
        );
        assert!(
            launch_output.status.success(),
            "ur process launch -w failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        // Verify marker file is visible inside container
        let cat_output = Command::new(&runtime)
            .args(["exec", &container_name, "cat", "/workspace/marker.txt"])
            .output()
            .expect("failed to exec cat in container");
        assert_eq!(
            cat_output.status.code(),
            Some(0),
            "cat marker.txt failed.\nstderr: {}",
            String::from_utf8_lossy(&cat_output.stderr),
        );
        assert_eq!(
            String::from_utf8_lossy(&cat_output.stdout).trim(),
            "mounted",
        );

        // Verify git works through the mount
        let git_output = Command::new(&runtime)
            .args(["exec", &container_name, "git", "status"])
            .output()
            .expect("failed to exec git status");
        assert_eq!(
            git_output.status.code(),
            Some(0),
            "git status failed.\nstderr: {}",
            String::from_utf8_lossy(&git_output.stderr),
        );

        // Verify tk is available
        let tk_output = Command::new(&runtime)
            .args(["exec", &container_name, "which", "tk"])
            .output()
            .expect("failed to exec which tk");
        assert_eq!(
            tk_output.status.code(),
            Some(0),
            "tk not found in container",
        );

        // Stop
        let stop_output = run_cmd(
            &ur,
            &["--port", &port_str, "process", "stop", process_id],
            &[],
        );
        assert!(stop_output.status.success());
    }));

    kill_and_wait(urd_child);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
```

**Step 2: Commit**

```
test(acceptance): add workspace mounting e2e test
```

---

### Task 8: Final verification

**Step 1: Run unit tests**

Run: `cargo test --workspace`
Expected: PASS

**Step 2: Run clippy**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::excessive_nesting`
Expected: PASS

**Step 3: Run fmt**

Run: `cargo fmt --all --check`
Expected: PASS

**Step 4: Run acceptance tests (manual, requires container runtime)**

Run: `cargo make stage-workercmd`
Run: `cargo test -p acceptance --features acceptance`
Expected: Both `e2e_ping_and_git` and `e2e_workspace_mount` PASS

**Step 5: Commit any fixups, then done**
