//! End-to-end acceptance tests for the Ur gRPC + workercmd architecture.
//!
//! All scenarios share a single `ur start` / `ur stop` cycle to avoid port
//! collisions between concurrent tests and to reduce total test runtime.
//!
//! The `e2e_all` test is the sole `#[test]` entry point. It:
//!   1. Sets up one shared `TestEnv` (config dir, bare repo, RAG docs, `ur start`)
//!   2. Calls each scenario sequentially as plain helper functions
//!   3. Tears down the server once at the end
//!
//! Gated behind `--features acceptance` so they never run in normal `cargo test`.
//! Requires:
//!   - Pre-built `ur` binary in `target/debug/`
//!   - Container images (`ur-server:latest`, `ur-worker:latest`) already built
//!   - A Docker-compatible container runtime
#![cfg(feature = "acceptance")]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

/// Generate a short unique prefix for this test run to avoid container/network
/// collisions with other concurrent CI runs or local stacks.
///
/// Uses `UR_WORKER_ID` env var if set, otherwise generates 4 random hex chars.
fn test_run_id() -> String {
    std::env::var("UR_WORKER_ID").unwrap_or_else(|_| {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(std::process::id() as u64);
        format!("{:04x}", hasher.finish() & 0xFFFF)
    })
}

static RUN_ID: LazyLock<String> = LazyLock::new(test_run_id);

/// Locate the workspace root (two levels up from this crate's manifest dir).
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent() // crates/
        .unwrap()
        .parent() // workspace root
        .unwrap()
        .to_path_buf()
}

/// Path to a debug binary built from this workspace.
/// Respects `CARGO_TARGET_DIR` if set, otherwise uses `target/` under workspace root.
fn bin(name: &str) -> PathBuf {
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace_root().join("target"));
    target_dir.join("debug").join(name)
}

/// Run a CLI command, returning its output. Panics on spawn failure.
fn run_cmd(cmd: &Path, args: &[&str], envs: &[(&str, &str)]) -> std::process::Output {
    let mut c = Command::new(cmd);
    c.args(args);
    c.current_dir(workspace_root());
    for &(k, v) in envs {
        c.env(k, v);
    }
    c.output()
        .unwrap_or_else(|e| panic!("failed to run {} {}: {e}", cmd.display(), args.join(" ")))
}

/// Detect the container runtime CLI command.
fn detect_container_runtime() -> String {
    if let Ok(val) = std::env::var("UR_CONTAINER") {
        return if val == "containerd" {
            "nerdctl".into()
        } else {
            val
        };
    }
    for cmd in ["docker", "nerdctl"] {
        if Command::new(cmd)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
        {
            return cmd.into();
        }
    }
    "docker".into()
}

/// Force-remove all containers and networks matching a name prefix.
/// Cleans up stale resources from prior failed runs regardless of run ID.
fn force_remove_by_prefix(runtime: &str, prefix: &str) {
    // Remove containers matching prefix
    let output = Command::new(runtime)
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("name={prefix}"),
            "--format",
            "{{.Names}}",
        ])
        .output();
    if let Ok(output) = output {
        let names = String::from_utf8_lossy(&output.stdout);
        for name in names.lines().filter(|l| !l.is_empty()) {
            let _ = Command::new(runtime)
                .args(["rm", "-f", name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    // Remove networks matching prefix
    let output = Command::new(runtime)
        .args([
            "network",
            "ls",
            "--filter",
            &format!("name={prefix}"),
            "--format",
            "{{.Name}}",
        ])
        .output();
    if let Ok(output) = output {
        let names = String::from_utf8_lossy(&output.stdout);
        for name in names.lines().filter(|l| !l.is_empty()) {
            let _ = Command::new(runtime)
                .args(["network", "rm", name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// Force-remove a container if it exists (cleanup from prior failed runs).
fn force_remove_container(runtime: &str, name: &str) {
    let _ = Command::new(runtime)
        .args(["rm", "-f", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Wait for a container to become healthy (Docker HEALTHCHECK passing).
/// Polls `docker inspect` up to 30s.
fn wait_for_healthy(runtime: &str, container: &str) {
    for i in 0..60 {
        let output = Command::new(runtime)
            .args(["inspect", "--format", "{{.State.Health.Status}}", container])
            .output()
            .expect("failed to inspect container health");
        let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if status == "healthy" {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        if i == 59 {
            panic!(
                "container '{container}' did not become healthy after 30s (last status: {status})"
            );
        }
    }
}

/// Extra project entries to append to ur.toml.
struct ProjectEntry {
    key: String,
    repo: String,
}

/// Configuration names for a test stack, preventing container/network collisions
/// between tests that may run concurrently.
struct TestNames {
    squid_hostname: String,
    network: String,
    worker_network: String,
    server_hostname: String,
    worker_prefix: String,
    qdrant_hostname: String,
}

/// Build test names with a unique prefix derived from the run ID.
/// `label` differentiates test stacks within the same run (e.g., "default", "pool").
fn test_names(label: &str) -> TestNames {
    let id = &*RUN_ID;
    TestNames {
        squid_hostname: format!("ur-{id}-{label}-squid"),
        network: format!("ur-{id}-{label}"),
        worker_network: format!("ur-{id}-{label}-workers"),
        server_hostname: format!("ur-{id}-{label}-server"),
        worker_prefix: format!("ur-{id}-{label}-worker-"),
        qdrant_hostname: format!("ur-{id}-{label}-qdrant"),
    }
}

/// Write a test-specific ur.toml and supporting files.
///
/// `ur start` renders the compose file from its embedded template, replacing
/// network name and container name placeholders with values from the config.
/// Uses unique container names so the acceptance test stack never collides
/// with a real running ur stack or other test stacks.
fn write_test_config(
    config_dir: &Path,
    server_port: u16,
    names: &TestNames,
    projects: &[ProjectEntry],
) {
    let workspace_dir = config_dir.join("workspace");
    std::fs::create_dir_all(&workspace_dir).expect("failed to create workspace dir");

    // Symlink the host's fastembed cache so the server container can find the
    // embedding model. The compose template mounts $UR_CONFIG/fastembed:/fastembed:ro.
    let host_fastembed = std::env::var("UR_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(std::env::var("HOME").expect("HOME env var")).join(".ur")
        })
        .join("fastembed");
    let test_fastembed = config_dir.join("fastembed");
    if host_fastembed.exists() && !test_fastembed.exists() {
        std::os::unix::fs::symlink(&host_fastembed, &test_fastembed)
            .expect("failed to symlink fastembed cache");
    }

    let squid_dir = config_dir.join("squid");
    std::fs::create_dir_all(&squid_dir).expect("failed to create squid dir");
    std::fs::write(
        squid_dir.join("allowlist.txt"),
        "api.anthropic.com\nplatform.claude.com\nmcp-proxy.anthropic.com\n",
    )
    .expect("failed to write allowlist.txt");

    let compose_file = config_dir.join("docker-compose.yml");

    let mut projects_toml = String::new();
    for proj in projects {
        projects_toml.push_str(&format!(
            "\n[projects.{key}]\nrepo = \"{repo}\"\n",
            key = proj.key,
            repo = proj.repo,
        ));
    }

    let toml_content = format!(
        "server_port = {server_port}\n\
         workspace = \"{workspace}\"\n\
         compose_file = \"{compose}\"\n\
         \n\
         [proxy]\n\
         hostname = \"{squid}\"\n\
         \n\
         [network]\n\
         name = \"{network}\"\n\
         worker_name = \"{worker_network}\"\n\
         server_hostname = \"{server}\"\n\
         worker_prefix = \"{worker_prefix}\"\n\
         \n\
         [rag]\n\
         qdrant_hostname = \"{qdrant}\"\n\
         {projects_toml}",
        workspace = workspace_dir.display(),
        compose = compose_file.display(),
        squid = names.squid_hostname,
        network = names.network,
        worker_network = names.worker_network,
        server = names.server_hostname,
        worker_prefix = names.worker_prefix,
        qdrant = names.qdrant_hostname,
    );
    std::fs::write(config_dir.join("ur.toml"), toml_content).expect("failed to write ur.toml");
}

/// Create a bare git repository with one commit, suitable as a clone source.
/// Returns the path to the bare repo directory.
fn create_bare_repo(parent_dir: &Path) -> PathBuf {
    let bare_repo = parent_dir.join("test-repo.git");
    let staging = parent_dir.join("staging");

    // Create the bare repo
    let output = Command::new("git")
        .args(["init", "--bare", bare_repo.to_str().unwrap()])
        .output()
        .expect("failed to run git init --bare");
    assert!(output.status.success(), "git init --bare failed");

    // Clone it into a staging dir, add a commit, push back
    let output = Command::new("git")
        .args([
            "clone",
            bare_repo.to_str().unwrap(),
            staging.to_str().unwrap(),
        ])
        .output()
        .expect("failed to clone bare repo into staging");
    assert!(output.status.success(), "git clone failed");

    // Configure git user for the commit
    let _ = Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&staging)
        .output();
    let _ = Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&staging)
        .output();

    // Create a file and commit
    std::fs::write(staging.join("README.md"), "# Test repo\n").expect("failed to write README");
    let output = Command::new("git")
        .args(["add", "README.md"])
        .current_dir(&staging)
        .output()
        .expect("failed to git add");
    assert!(output.status.success(), "git add failed");

    let output = Command::new("git")
        .args(["commit", "-m", "initial commit"])
        .current_dir(&staging)
        .output()
        .expect("failed to git commit");
    assert!(output.status.success(), "git commit failed");

    let output = Command::new("git")
        .args(["push", "origin", "HEAD"])
        .current_dir(&staging)
        .output()
        .expect("failed to git push");
    assert!(
        output.status.success(),
        "git push failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    bare_repo
}

/// Kill any process listening on the given TCP port. Used to clean up orphaned
/// builderd processes from prior failed test runs.
fn kill_process_on_port(port: u16) {
    // lsof -ti tcp:<port> prints PIDs of processes listening on the port
    let output = Command::new("lsof")
        .args(["-ti", &format!("tcp:{port}")])
        .output();
    if let Ok(output) = output {
        let pids = String::from_utf8_lossy(&output.stdout);
        for pid in pids.lines().filter(|l| !l.is_empty()) {
            let _ = Command::new("kill")
                .arg(pid)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// Run `ur stop` for cleanup, ignoring errors.
fn stop_server(ur: &Path, config_dir: &Path) {
    let _ = run_cmd(
        ur,
        &["server", "stop"],
        &[("UR_CONFIG", config_dir.to_str().unwrap())],
    );
}

/// Shared test environment holding everything needed across all scenarios.
struct TestEnv {
    /// Kept alive for the duration of the test (dropped at end of `e2e_all`).
    _config_dir: tempfile::TempDir,
    config_path: PathBuf,
    ur: PathBuf,
    runtime: String,
    names: TestNames,
    project_key: &'static str,
}

impl TestEnv {
    /// Shorthand for the UR_CONFIG env pair used in `run_cmd`.
    fn env(&self) -> Vec<(&str, &str)> {
        vec![("UR_CONFIG", self.config_path.to_str().unwrap())]
    }

    /// Build a container name from the shared worker prefix and a ticket ID.
    fn container_name(&self, ticket_id: &str) -> String {
        format!("{}{ticket_id}", self.names.worker_prefix)
    }
}

/// Single `#[test]` entry point. Sets up the environment once, runs all
/// scenarios sequentially, then tears down.
#[test]
fn e2e_all() {
    let runtime = detect_container_runtime();
    let names = test_names("e2e");
    let server_port = 19870u16;
    let project_key = "poolproj";

    // ---- (0) Clean up stale resources from ANY prior e2e run ----
    // Docker's name filter does substring matching, so "-e2e-" catches all
    // ur-{id}-e2e-{role} containers regardless of which random run ID created them.
    force_remove_by_prefix(&runtime, "-e2e-");
    // Kill any orphaned builderd processes from prior failed runs. Builderd runs
    // in its own process group (detached from the test), so it survives test crashes.
    // We identify stale test builderds by their port (builderd_port = server_port + 2).
    kill_process_on_port(server_port + 2);

    // ---- (1) Create temp UR_CONFIG dir ----
    let config_dir = tempfile::tempdir().expect("failed to create temp config dir");
    let config_path = config_dir.path().to_path_buf();

    // Create bare repo BEFORE writing config (config references it)
    let bare_repo = create_bare_repo(&config_path);

    // Create RAG docs directory and test documents
    let rag_docs_dir = config_path.join("rag").join("docs").join("rust");
    std::fs::create_dir_all(&rag_docs_dir).expect("failed to create rag docs dir");
    std::fs::write(
        rag_docs_dir.join("container.md"),
        "# Container Management\n\n\
         The container module manages Docker containers for workers.\n\
         It handles lifecycle operations: create, start, stop, and remove.\n\
         Each worker gets its own isolated container with mounted workspace.\n",
    )
    .expect("failed to write test doc");
    std::fs::write(
        rag_docs_dir.join("grpc.md"),
        "# gRPC Server\n\n\
         The gRPC server listens on TCP port 42069 for requests from the CLI\n\
         and from worker containers. It routes commands to the appropriate\n\
         handler: process management, git operations, or RAG search.\n",
    )
    .expect("failed to write test doc");

    // Write config with the pool project
    write_test_config(
        &config_path,
        server_port,
        &names,
        &[ProjectEntry {
            key: project_key.into(),
            repo: bare_repo.to_string_lossy().into_owned(),
        }],
    );

    let ur = bin("ur");
    assert!(ur.exists(), "ur binary not found at {}", ur.display());

    let env = TestEnv {
        _config_dir: config_dir,
        config_path: config_path.clone(),
        ur: ur.clone(),
        runtime,
        names,
        project_key,
    };

    // ---- (2) ur start, run scenarios, always tear down ----
    // Everything from `ur start` onward is wrapped in catch_unwind so that
    // teardown runs even if `ur start` itself fails (e.g., port conflict).
    let scenario_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let env_pairs = env.env();
        let env_slice = env_pairs.to_vec();
        let up_output = run_cmd(&ur, &["server", "start"], &env_slice);
        assert!(
            up_output.status.success(),
            "ur start failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&up_output.stdout),
            String::from_utf8_lossy(&up_output.stderr),
        );

        // Init workspace git repo (needed for workspace-mount scenario)
        let workspace_dir = config_path.join("workspace");
        let git_init = Command::new("git")
            .args(["init", workspace_dir.to_str().unwrap()])
            .output()
            .expect("failed to run git init");
        assert!(git_init.status.success(), "git init failed");

        // ---- (3) Run scenarios sequentially ----
        scenario_workspace_mount(&env);
        scenario_pool_launch(&env);
        scenario_design_mode_pool_launch(&env);
        scenario_rag_search(&env);
    }));

    // ---- (4) Always tear down: force-remove leftover worker containers, then stop server ----
    for ticket in ["ping-test", "pool-test", "design-test-1", "design-test-2"] {
        force_remove_container(&env.runtime, &env.container_name(ticket));
    }
    stop_server(&env.ur, &env.config_path);

    if let Err(e) = scenario_result {
        std::panic::resume_unwind(e);
    }
}

// ---------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------

/// Workspace mount: verify `-w` launches and mounts the host directory.
fn scenario_workspace_mount(env: &TestEnv) {
    let ticket_id = "ping-test";
    let container_name = env.container_name(ticket_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Launch worker with workspace mount ----
        let workspace_dir = env.config_path.join("workspace");
        let workspace_str = workspace_dir.to_str().unwrap();
        let launch_output = run_cmd(
            &env.ur,
            &["worker", "launch", "-w", workspace_str, ticket_id],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        let launch_stdout = String::from_utf8_lossy(&launch_output.stdout);
        assert!(
            launch_stdout.contains(&container_name),
            "launch output should contain container name '{container_name}'.\nGot: {launch_stdout}"
        );

        wait_for_healthy(&env.runtime, &container_name);

        // ---- exec ur-ping inside container ----
        let ping_output = Command::new(&env.runtime)
            .args(["exec", &container_name, "ur-ping"])
            .output()
            .expect("failed to exec ur-ping in container");

        assert_eq!(
            ping_output.status.code(),
            Some(0),
            "ur-ping should exit 0.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&ping_output.stdout),
            String::from_utf8_lossy(&ping_output.stderr),
        );
        let ping_stdout = String::from_utf8_lossy(&ping_output.stdout);
        assert_eq!(
            ping_stdout.trim(),
            "pong",
            "ur-ping should return 'pong', got: {ping_stdout}"
        );

        // ---- Stop worker ----
        let stop_output = run_cmd(&env.ur, &["worker", "stop", ticket_id], &env_slice);
        assert!(
            stop_output.status.success(),
            "ur worker stop failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&stop_output.stdout),
            String::from_utf8_lossy(&stop_output.stderr),
        );
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// Run a command inside a container and return its output.
fn exec_in_container(runtime: &str, container: &str, args: &[&str]) -> std::process::Output {
    Command::new(runtime)
        .arg("exec")
        .arg(container)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to exec {args:?} in container {container}: {e}"))
}

/// Assert that a container exec exited with code 0, panicking with stdout/stderr on failure.
fn assert_exec_success(output: &std::process::Output, context: &str) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{context}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Verify that ur-ping returns "pong" inside a container.
fn assert_ping_pong(runtime: &str, container: &str) {
    let ping_output = exec_in_container(runtime, container, &["ur-ping"]);
    assert_exec_success(&ping_output, "ur-ping should exit 0");
    assert_eq!(
        String::from_utf8_lossy(&ping_output.stdout).trim(),
        "pong",
        "ur-ping should return 'pong'"
    );
}

/// Verify git hostexec works and that Lua validation blocks the -C flag.
fn assert_git_hostexec(runtime: &str, container: &str) {
    // git log should succeed and show our commit
    let git_output = exec_in_container(runtime, container, &["git", "log", "--oneline", "-1"]);
    assert_exec_success(&git_output, "git log should work in pool slot");
    let git_stdout = String::from_utf8_lossy(&git_output.stdout);
    assert!(
        git_stdout.contains("initial commit"),
        "git log should show our commit.\nGot: {git_stdout}"
    );

    // git -C should be blocked by Lua validation
    let blocked_output = exec_in_container(runtime, container, &["git", "-C", "/tmp", "status"]);
    assert_ne!(
        blocked_output.status.code(),
        Some(0),
        "git -C should be blocked.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&blocked_output.stdout),
        String::from_utf8_lossy(&blocked_output.stderr),
    );
    let blocked_stderr = String::from_utf8_lossy(&blocked_output.stderr);
    assert!(
        blocked_stderr.contains("-C"),
        "error should mention -C.\nstderr: {blocked_stderr}"
    );
}

/// Verify squid proxy blocks disallowed domains and allows configured ones.
fn assert_squid_proxy_filtering(runtime: &str, container: &str) {
    let curl_args = |url: &str| -> Vec<String> {
        [
            "curl",
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_connect}",
            "--max-time",
            "10",
            url,
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
    };

    // Blocked domain should return 403
    let blocked_args = curl_args("https://google.com");
    let blocked_refs: Vec<&str> = blocked_args.iter().map(|s| s.as_str()).collect();
    let blocked_curl = exec_in_container(runtime, container, &blocked_refs);
    let blocked_code = String::from_utf8_lossy(&blocked_curl.stdout);
    assert_eq!(
        blocked_code.trim(),
        "403",
        "blocked domain should get 403 from squid.\nhttp_connect: {blocked_code}\nstderr: {}",
        String::from_utf8_lossy(&blocked_curl.stderr),
    );

    // Allowed domain should connect through
    let allowed_args = curl_args("https://api.anthropic.com");
    let allowed_refs: Vec<&str> = allowed_args.iter().map(|s| s.as_str()).collect();
    let allowed_curl = exec_in_container(runtime, container, &allowed_refs);
    let allowed_code = String::from_utf8_lossy(&allowed_curl.stdout);
    let allowed_code_num: u16 = allowed_code.trim().parse().unwrap_or(0);
    assert!(
        allowed_code_num > 0 && allowed_code_num != 403,
        "allowed domain should connect through squid (not 000/403).\n\
         http_connect: {allowed_code}\nstderr: {}",
        String::from_utf8_lossy(&allowed_curl.stderr),
    );
}

/// Pool launch: clone, ping, git hostexec, squid proxy, Lua validation, host dir structure.
/// All container-side tests run against a single pool container to avoid redundant launches.
fn scenario_pool_launch(env: &TestEnv) {
    let ticket_id = "pool-test";
    let container_name = env.container_name(ticket_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Launch pool worker ----
        let launch_output = run_cmd(
            &env.ur,
            &["worker", "launch", "-p", env.project_key, ticket_id],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch -p {} failed.\nstdout: {}\nstderr: {}",
            env.project_key,
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        let launch_stdout = String::from_utf8_lossy(&launch_output.stdout);
        assert!(
            launch_stdout.contains(&container_name),
            "launch output should contain container name '{container_name}'.\nGot: {launch_stdout}"
        );

        wait_for_healthy(&env.runtime, &container_name);

        // ---- Verify workspace has cloned content ----
        let ls_output = exec_in_container(
            &env.runtime,
            &container_name,
            &["ls", "/workspace/README.md"],
        );
        assert_exec_success(
            &ls_output,
            "pool slot should contain README.md from cloned repo",
        );

        // ---- exec ur-ping inside container ----
        assert_ping_pong(&env.runtime, &container_name);

        // ---- Test hostexec: git commands and Lua validation ----
        assert_git_hostexec(&env.runtime, &container_name);

        // ---- Squid proxy filtering ----
        assert_squid_proxy_filtering(&env.runtime, &container_name);

        // ---- Verify pool directory structure on host ----
        let pool_slot = env
            .config_path
            .join("workspace")
            .join("pool")
            .join(env.project_key)
            .join("0");
        assert!(
            pool_slot.exists(),
            "pool slot directory should exist at {}",
            pool_slot.display()
        );
        assert!(
            pool_slot.join(".git").exists(),
            "pool slot should be a git repo (have .git)"
        );
        assert!(
            pool_slot.join("README.md").exists(),
            "pool slot should contain README.md from clone"
        );

        // ---- Stop worker ----
        let stop_output = run_cmd(&env.ur, &["worker", "stop", ticket_id], &env_slice);
        assert!(
            stop_output.status.success(),
            "ur worker stop failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&stop_output.stdout),
            String::from_utf8_lossy(&stop_output.stderr),
        );
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// Design mode uses exclusive numbered slots (same as code mode).
/// Verify launch, slot reuse after stop, and coexistence with code workers.
fn scenario_design_mode_pool_launch(env: &TestEnv) {
    let ticket_id_1 = "design-test-1";
    let ticket_id_2 = "design-test-2";
    let container_name_1 = env.container_name(ticket_id_1);
    let container_name_2 = env.container_name(ticket_id_2);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Launch first design worker ----
        let launch_output = run_cmd(
            &env.ur,
            &[
                "worker",
                "launch",
                "-p",
                env.project_key,
                "-m",
                "design",
                ticket_id_1,
            ],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch -p {} -m design failed.\nstdout: {}\nstderr: {}",
            env.project_key,
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        let launch_stdout = String::from_utf8_lossy(&launch_output.stdout);
        assert!(
            launch_stdout.contains(&container_name_1),
            "launch output should contain container name '{container_name_1}'.\nGot: {launch_stdout}"
        );

        wait_for_healthy(&env.runtime, &container_name_1);

        // ---- Verify worker has cloned content ----
        let ls_output = Command::new(&env.runtime)
            .args(["exec", &container_name_1, "ls", "/workspace/README.md"])
            .output()
            .expect("failed to exec ls in container");
        assert_eq!(
            ls_output.status.code(),
            Some(0),
            "design worker should have README.md from cloned repo.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&ls_output.stdout),
            String::from_utf8_lossy(&ls_output.stderr),
        );

        // ---- Stop first worker ----
        let stop_output = run_cmd(&env.ur, &["worker", "stop", ticket_id_1], &env_slice);
        assert!(
            stop_output.status.success(),
            "ur worker stop failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&stop_output.stdout),
            String::from_utf8_lossy(&stop_output.stderr),
        );

        // ---- Launch second design worker — should reuse the released slot ----
        let launch2_output = run_cmd(
            &env.ur,
            &[
                "worker",
                "launch",
                "-p",
                env.project_key,
                "-m",
                "design",
                ticket_id_2,
            ],
            &env_slice,
        );
        assert!(
            launch2_output.status.success(),
            "second design launch failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch2_output.stdout),
            String::from_utf8_lossy(&launch2_output.stderr),
        );

        wait_for_healthy(&env.runtime, &container_name_2);

        // ---- Verify second worker also has cloned content ----
        let ls_output2 = Command::new(&env.runtime)
            .args(["exec", &container_name_2, "ls", "/workspace/README.md"])
            .output()
            .expect("failed to exec ls in container");
        assert_eq!(
            ls_output2.status.code(),
            Some(0),
            "second design worker should have README.md.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&ls_output2.stdout),
            String::from_utf8_lossy(&ls_output2.stderr),
        );

        // Stop second worker
        let stop2_output = run_cmd(&env.ur, &["worker", "stop", ticket_id_2], &env_slice);
        assert!(
            stop2_output.status.success(),
            "ur worker stop (second design) failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&stop2_output.stdout),
            String::from_utf8_lossy(&stop2_output.stderr),
        );
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name_1);
        force_remove_container(&env.runtime, &container_name_2);
        std::panic::resume_unwind(e);
    }
}

/// RAG index and search.
fn scenario_rag_search(env: &TestEnv) {
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    // No worker containers in this scenario — just CLI commands against the server.

    // ---- ur rag index — index test docs into Qdrant ----
    let index_output = run_cmd(&env.ur, &["rag", "index", "--language", "rust"], &env_slice);
    assert!(
        index_output.status.success(),
        "ur rag index failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&index_output.stdout),
        String::from_utf8_lossy(&index_output.stderr),
    );

    let index_stdout = String::from_utf8_lossy(&index_output.stdout);
    assert!(
        index_stdout.contains("Indexed") && index_stdout.contains("chunks"),
        "ur rag index should report indexed chunks.\nGot: {index_stdout}"
    );

    // ---- ur rag search — search indexed docs ----
    let search_output = run_cmd(
        &env.ur,
        &[
            "rag",
            "search",
            "container management",
            "--language",
            "rust",
        ],
        &env_slice,
    );
    assert!(
        search_output.status.success(),
        "ur rag search failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&search_output.stdout),
        String::from_utf8_lossy(&search_output.stderr),
    );

    let search_stdout = String::from_utf8_lossy(&search_output.stdout);
    // Search should return results (not "No results found")
    assert!(
        !search_stdout.contains("No results found"),
        "ur rag search should return results after indexing.\nGot: {search_stdout}"
    );
    // Results should contain the expected format fields
    assert!(
        search_stdout.contains("Result") && search_stdout.contains("score:"),
        "ur rag search output should contain Result and score fields.\nGot: {search_stdout}"
    );
    assert!(
        search_stdout.contains("Source:"),
        "ur rag search output should contain Source field.\nGot: {search_stdout}"
    );
}
