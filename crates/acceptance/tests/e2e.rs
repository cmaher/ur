//! End-to-end acceptance tests for the Ur gRPC + workercmd architecture.
//!
//! These tests exercise the full user-facing workflow via Docker Compose:
//!   1. `ur start` starts the server + squid in containers via docker compose
//!   2. `ur process launch` launches a worker container via the server
//!   3. Worker commands inside the container (`ur-ping`, `git`) connect to the server
//!      via tonic gRPC over TCP using `UR_SERVER_ADDR`
//!   4. `ur process stop` tears down the worker container
//!   5. `ur stop` stops the server + squid containers
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
/// Uses `UR_AGENT_ID` env var if set, otherwise generates 4 random hex chars.
fn test_run_id() -> String {
    std::env::var("UR_AGENT_ID").unwrap_or_else(|_| {
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

/// Force-remove a container if it exists (cleanup from prior failed runs).
fn force_remove_container(runtime: &str, name: &str) {
    let _ = Command::new(runtime)
        .args(["rm", "-f", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
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
    agent_prefix: String,
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
        agent_prefix: format!("ur-{id}-{label}-agent-"),
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
    daemon_port: u16,
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
        "api.anthropic.com\nplatform.claude.com\nraw.githubusercontent.com\n",
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
        "daemon_port = {daemon_port}\n\
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
         agent_prefix = \"{agent_prefix}\"\n\
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
        agent_prefix = names.agent_prefix,
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

/// Run `ur stop` for cleanup, ignoring errors.
fn stop_server(ur: &Path, config_dir: &Path) {
    let _ = run_cmd(
        ur,
        &["stop"],
        &[("UR_CONFIG", config_dir.to_str().unwrap())],
    );
}

#[test]
fn e2e_ping_and_git() {
    let runtime = detect_container_runtime();
    let names = test_names("default");
    let ticket_id = "acceptance-test";
    let container_name = format!("{}{ticket_id}", names.agent_prefix);
    let server_container = &names.server_hostname;
    let squid_container = &names.squid_hostname;
    let qdrant_container = &names.qdrant_hostname;

    // ---- (0) Clean up stale containers from prior failed runs ----
    force_remove_container(&runtime, &container_name);
    force_remove_container(&runtime, server_container);
    force_remove_container(&runtime, squid_container);
    force_remove_container(&runtime, qdrant_container);

    // ---- (1) Create temp UR_CONFIG dir with test-specific config ----
    let config_dir = tempfile::tempdir().expect("failed to create temp config dir");
    let config_path = config_dir.path();
    let daemon_port = 19860u16;

    write_test_config(config_path, daemon_port, &names, &[]);

    let ur = bin("ur");
    assert!(ur.exists(), "ur binary not found at {}", ur.display());
    let config_str = config_path.to_str().unwrap();
    let env = [("UR_CONFIG", config_str)];

    // Use catch_unwind so we always clean up via compose down even on panic.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- (2) ur start ----
        let up_output = run_cmd(&ur, &["start"], &env);
        assert!(
            up_output.status.success(),
            "ur start failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&up_output.stdout),
            String::from_utf8_lossy(&up_output.stderr),
        );

        // ---- (3) ur process launch ----
        // With -w, the server skips git init, so we must init the workspace ourselves.
        let workspace_dir = config_path.join("workspace");
        let git_init = Command::new("git")
            .args(["init", workspace_dir.to_str().unwrap()])
            .output()
            .expect("failed to run git init");
        assert!(git_init.status.success(), "git init failed");
        let workspace_str = workspace_dir.to_str().unwrap();
        let launch_output = run_cmd(
            &ur,
            &["worker", "launch", "-w", workspace_str, ticket_id],
            &env,
        );
        assert!(
            launch_output.status.success(),
            "ur process launch failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        let launch_stdout = String::from_utf8_lossy(&launch_output.stdout);
        assert!(
            launch_stdout.contains(&container_name),
            "launch output should contain container name '{container_name}'.\nGot: {launch_stdout}"
        );

        // ---- (4) exec ur-ping inside container ----
        let ping_output = Command::new(&runtime)
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

        // ---- (5) Test hostexec: git commands via worker → server → builderd → host ----
        // The git shim calls workertools host-exec git, which goes through the full
        // hostexec pipeline. This verifies builderd is running and reachable.
        let git_output = Command::new(&runtime)
            .args(["exec", &container_name, "git", "status"])
            .output()
            .expect("failed to exec git status in container");

        assert_eq!(
            git_output.status.code(),
            Some(0),
            "git status should exit 0 (hostexec pipeline: worker → server → builderd → host).\n\
             stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&git_output.stdout),
            String::from_utf8_lossy(&git_output.stderr),
        );

        let git_stdout = String::from_utf8_lossy(&git_output.stdout);
        assert!(
            git_stdout.contains("branch") || git_stdout.contains("No commits"),
            "git status should show repo info.\nGot: {git_stdout}"
        );

        // ---- (5b) Test hostexec Lua validation: -C flag blocking ----
        let blocked_output = Command::new(&runtime)
            .args(["exec", &container_name, "git", "-C", "/tmp", "status"])
            .output()
            .expect("failed to exec git -C /tmp status in container");
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

        // ---- (6) Squid proxy: blocked domain returns 403 ----
        // Use %{http_connect} to capture the proxy's CONNECT response code.
        // %{http_code} reports the final destination response which is 000 when
        // the CONNECT tunnel is denied (no destination connection is made).
        let blocked_curl = Command::new(&runtime)
            .args([
                "exec",
                &container_name,
                "curl",
                "-s",
                "-o",
                "/dev/null",
                "-w",
                "%{http_connect}",
                "--max-time",
                "10",
                "https://google.com",
            ])
            .output()
            .expect("failed to exec curl (blocked) in container");

        let blocked_code = String::from_utf8_lossy(&blocked_curl.stdout);
        assert_eq!(
            blocked_code.trim(),
            "403",
            "blocked domain should get 403 from squid.\nhttp_connect: {blocked_code}\nstderr: {}",
            String::from_utf8_lossy(&blocked_curl.stderr),
        );

        // ---- (6b) Squid proxy: allowed domain connects through ----
        // Use %{http_connect} to verify the CONNECT tunnel is established (200).
        let allowed_curl = Command::new(&runtime)
            .args([
                "exec",
                &container_name,
                "curl",
                "-s",
                "-o",
                "/dev/null",
                "-w",
                "%{http_connect}",
                "--max-time",
                "10",
                "https://api.anthropic.com",
            ])
            .output()
            .expect("failed to exec curl (allowed) in container");

        let allowed_code = String::from_utf8_lossy(&allowed_curl.stdout);
        let allowed_code_num: u16 = allowed_code.trim().parse().unwrap_or(0);
        assert!(
            allowed_code_num > 0 && allowed_code_num != 403,
            "allowed domain should connect through squid (not 000/403).\n\
             http_connect: {allowed_code}\nstderr: {}",
            String::from_utf8_lossy(&allowed_curl.stderr),
        );

        // ---- (7) ur process stop ----
        let stop_output = run_cmd(&ur, &["worker", "stop", ticket_id], &env);
        assert!(
            stop_output.status.success(),
            "ur process stop failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&stop_output.stdout),
            String::from_utf8_lossy(&stop_output.stderr),
        );
    }));

    // ---- (8) Always tear down server ----
    stop_server(&ur, config_path);

    // Re-raise any panic from the test body.
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

/// End-to-end test for project pool launches (`-p` flag).
///
/// Exercises the full workflow:
///   1. Create a bare git repo as clone source
///   2. Configure it as a project in ur.toml
///   3. `ur start` starts the server + builderd
///   4. `ur process launch <ticket> -p <project>` acquires a pool slot via builderd git clone
///   5. Verify the cloned workspace has the expected content
///   6. Verify git commands work inside the container (hostexec pipeline)
///   7. `ur process stop` releases the pool slot
///   8. `ur stop` tears down the server
#[test]
fn e2e_project_pool_launch() {
    let runtime = detect_container_runtime();
    let names = test_names("pool");
    let ticket_id = "pool-test";
    let project_key = "poolproj";
    let container_name = format!("{}{ticket_id}", names.agent_prefix);
    let server_container = &names.server_hostname;
    let squid_container = &names.squid_hostname;
    let qdrant_container = &names.qdrant_hostname;

    // ---- (0) Clean up stale containers from prior failed runs ----
    force_remove_container(&runtime, &container_name);
    force_remove_container(&runtime, server_container);
    force_remove_container(&runtime, squid_container);
    force_remove_container(&runtime, qdrant_container);

    // ---- (1) Create bare git repo and test config with project ----
    let config_dir = tempfile::tempdir().expect("failed to create temp config dir");
    let config_path = config_dir.path();
    let daemon_port = 19870u16; // spaced by 10 to avoid worker/builderd port overlap

    // Create bare repo before writing config (config needs the repo path)
    let bare_repo = create_bare_repo(config_path);

    write_test_config(
        config_path,
        daemon_port,
        &names,
        &[ProjectEntry {
            key: project_key.into(),
            repo: bare_repo.to_string_lossy().into_owned(),
        }],
    );

    let ur = bin("ur");
    assert!(ur.exists(), "ur binary not found at {}", ur.display());
    let config_str = config_path.to_str().unwrap();
    let env = [("UR_CONFIG", config_str)];

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- (2) ur start ----
        let up_output = run_cmd(&ur, &["start"], &env);
        assert!(
            up_output.status.success(),
            "ur start failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&up_output.stdout),
            String::from_utf8_lossy(&up_output.stderr),
        );

        // ---- (3) ur process launch -p <project> ----
        let launch_output = run_cmd(
            &ur,
            &["worker", "launch", "-p", project_key, ticket_id],
            &env,
        );
        assert!(
            launch_output.status.success(),
            "ur process launch -p {project_key} failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        let launch_stdout = String::from_utf8_lossy(&launch_output.stdout);
        assert!(
            launch_stdout.contains(&container_name),
            "launch output should contain container name '{container_name}'.\nGot: {launch_stdout}"
        );

        // ---- (4) Verify workspace has cloned content ----
        // The pool slot should have the README.md from the bare repo
        let ls_output = Command::new(&runtime)
            .args(["exec", &container_name, "ls", "/workspace/README.md"])
            .output()
            .expect("failed to exec ls in container");
        assert_eq!(
            ls_output.status.code(),
            Some(0),
            "pool slot should contain README.md from cloned repo.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&ls_output.stdout),
            String::from_utf8_lossy(&ls_output.stderr),
        );

        // ---- (5) Verify git commands work via hostexec ----
        let git_output = Command::new(&runtime)
            .args(["exec", &container_name, "git", "log", "--oneline", "-1"])
            .output()
            .expect("failed to exec git log in container");
        assert_eq!(
            git_output.status.code(),
            Some(0),
            "git log should work in pool slot.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&git_output.stdout),
            String::from_utf8_lossy(&git_output.stderr),
        );

        let git_stdout = String::from_utf8_lossy(&git_output.stdout);
        assert!(
            git_stdout.contains("initial commit"),
            "git log should show our commit.\nGot: {git_stdout}"
        );

        // ---- (6) Verify pool directory structure on host ----
        // The slot should be at $WORKSPACE/pool/<project-key>/0/
        let pool_slot = config_path
            .join("workspace")
            .join("pool")
            .join(project_key)
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

        // ---- (7) ur process stop ----
        let stop_output = run_cmd(&ur, &["worker", "stop", ticket_id], &env);
        assert!(
            stop_output.status.success(),
            "ur process stop failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&stop_output.stdout),
            String::from_utf8_lossy(&stop_output.stderr),
        );
    }));

    // ---- (8) Always tear down server ----
    stop_server(&ur, config_path);

    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

/// End-to-end test for design mode pool launches (`-p <project> -m design`).
///
/// Exercises:
///   1. Create a bare git repo as clone source, configure as project
///   2. `ur start` starts the server + builderd
///   3. `ur process launch -p <project> -m design <ticket>` acquires a shared design slot
///   4. Verify the `design/` slot directory is created under the pool path
///   5. Worker launches successfully
///   6. Stop first worker, launch a second design worker — reuses same slot path
///   7. Design launches do not consume exclusive pool slots (pool limit unaffected)
///   8. Tear down
#[test]
fn e2e_design_mode_pool_launch() {
    let runtime = detect_container_runtime();
    let names = test_names("design");
    let ticket_id_1 = "design-test-1";
    let ticket_id_2 = "design-test-2";
    let code_ticket_id = "design-code-test";
    let project_key = "designproj";
    let container_name_1 = format!("{}{ticket_id_1}", names.agent_prefix);
    let container_name_2 = format!("{}{ticket_id_2}", names.agent_prefix);
    let code_container_name = format!("{}{code_ticket_id}", names.agent_prefix);
    let server_container = &names.server_hostname;
    let squid_container = &names.squid_hostname;
    let qdrant_container = &names.qdrant_hostname;

    // ---- (0) Clean up stale containers from prior failed runs ----
    force_remove_container(&runtime, &container_name_1);
    force_remove_container(&runtime, &container_name_2);
    force_remove_container(&runtime, &code_container_name);
    force_remove_container(&runtime, server_container);
    force_remove_container(&runtime, squid_container);
    force_remove_container(&runtime, qdrant_container);

    // ---- (1) Create bare git repo and test config with project ----
    let config_dir = tempfile::tempdir().expect("failed to create temp config dir");
    let config_path = config_dir.path();
    let daemon_port = 19880u16; // spaced by 10 to avoid worker/builderd port overlap

    let bare_repo = create_bare_repo(config_path);

    write_test_config(
        config_path,
        daemon_port,
        &names,
        &[ProjectEntry {
            key: project_key.into(),
            repo: bare_repo.to_string_lossy().into_owned(),
        }],
    );

    let ur = bin("ur");
    assert!(ur.exists(), "ur binary not found at {}", ur.display());
    let config_str = config_path.to_str().unwrap();
    let env = [("UR_CONFIG", config_str)];

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- (2) ur start ----
        let up_output = run_cmd(&ur, &["start"], &env);
        assert!(
            up_output.status.success(),
            "ur start failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&up_output.stdout),
            String::from_utf8_lossy(&up_output.stderr),
        );

        // ---- (3) Launch first design worker: ur process launch -p <project> -m design ----
        let launch_output = run_cmd(
            &ur,
            &[
                "worker",
                "launch",
                "-p",
                project_key,
                "-m",
                "design",
                ticket_id_1,
            ],
            &env,
        );
        assert!(
            launch_output.status.success(),
            "ur process launch -p {project_key} -m design failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        let launch_stdout = String::from_utf8_lossy(&launch_output.stdout);
        assert!(
            launch_stdout.contains(&container_name_1),
            "launch output should contain container name '{container_name_1}'.\nGot: {launch_stdout}"
        );

        // ---- (4) Verify design/ slot directory exists on host ----
        let design_slot = config_path
            .join("workspace")
            .join("pool")
            .join(project_key)
            .join("design");
        assert!(
            design_slot.exists(),
            "design slot directory should exist at {}",
            design_slot.display()
        );
        assert!(
            design_slot.join(".git").exists(),
            "design slot should be a git repo (have .git)"
        );
        assert!(
            design_slot.join("README.md").exists(),
            "design slot should contain README.md from clone"
        );

        // ---- (5) Verify worker has cloned content ----
        let ls_output = Command::new(&runtime)
            .args(["exec", &container_name_1, "ls", "/workspace/README.md"])
            .output()
            .expect("failed to exec ls in container");
        assert_eq!(
            ls_output.status.code(),
            Some(0),
            "design slot should contain README.md from cloned repo.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&ls_output.stdout),
            String::from_utf8_lossy(&ls_output.stderr),
        );

        // ---- (6) Stop first worker ----
        let stop_output = run_cmd(&ur, &["worker", "stop", ticket_id_1], &env);
        assert!(
            stop_output.status.success(),
            "ur process stop failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&stop_output.stdout),
            String::from_utf8_lossy(&stop_output.stderr),
        );

        // ---- (7) Launch second design worker — should reuse same slot path ----
        let launch2_output = run_cmd(
            &ur,
            &[
                "worker",
                "launch",
                "-p",
                project_key,
                "-m",
                "design",
                ticket_id_2,
            ],
            &env,
        );
        assert!(
            launch2_output.status.success(),
            "second design launch failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch2_output.stdout),
            String::from_utf8_lossy(&launch2_output.stderr),
        );

        // The design/ slot should still exist (reused, not a new numbered slot)
        assert!(
            design_slot.exists(),
            "design slot should still exist after second launch at {}",
            design_slot.display()
        );

        // No numbered slots should have been created for design launches
        let slot_0 = config_path
            .join("workspace")
            .join("pool")
            .join(project_key)
            .join("0");
        assert!(
            !slot_0.exists(),
            "numbered slot 0 should NOT exist — design mode uses shared 'design/' slot, not exclusive numbered slots"
        );

        // ---- (8) Design launches don't consume exclusive pool slots ----
        // Launch a code worker — it should succeed because design didn't consume any exclusive slots
        let code_launch = run_cmd(
            &ur,
            &[
                "worker",
                "launch",
                "-p",
                project_key,
                "-m",
                "code",
                code_ticket_id,
            ],
            &env,
        );
        assert!(
            code_launch.status.success(),
            "code launch should succeed (design didn't consume exclusive slots).\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&code_launch.stdout),
            String::from_utf8_lossy(&code_launch.stderr),
        );

        // Now the numbered slot 0 should exist (created by the code launch)
        assert!(
            slot_0.exists(),
            "numbered slot 0 should exist after code launch at {}",
            slot_0.display()
        );

        // Stop both workers
        let stop2_output = run_cmd(&ur, &["worker", "stop", ticket_id_2], &env);
        assert!(
            stop2_output.status.success(),
            "ur process stop (second design) failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&stop2_output.stdout),
            String::from_utf8_lossy(&stop2_output.stderr),
        );

        let stop_code = run_cmd(&ur, &["worker", "stop", code_ticket_id], &env);
        assert!(
            stop_code.status.success(),
            "ur process stop (code) failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&stop_code.stdout),
            String::from_utf8_lossy(&stop_code.stderr),
        );
    }));

    // ---- (9) Always tear down server ----
    stop_server(&ur, config_path);

    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

/// End-to-end test for RAG search (`ur rag search`).
///
/// Exercises the full RAG pipeline:
///   1. `ur start` starts the server + Qdrant + squid in containers via docker compose
///   2. Small test markdown docs are written directly (not `ur rag docs` — too slow)
///   3. `ur rag index --language rust` indexes the docs into Qdrant via the server
///   4. `ur rag search "query" --language rust` searches indexed docs via the server
///   5. `ur stop` tears down the server
///
/// Requires:
///   - ONNX embedding model downloaded on host (`ur rag model download`)
///   - Qdrant service in docker compose
#[test]
fn e2e_rag_search() {
    let runtime = detect_container_runtime();
    let names = test_names("rag");
    let server_container = &names.server_hostname;
    let squid_container = &names.squid_hostname;
    let qdrant_container = &names.qdrant_hostname;

    // ---- (0) Clean up stale containers from prior failed runs ----
    force_remove_container(&runtime, server_container);
    force_remove_container(&runtime, squid_container);
    force_remove_container(&runtime, qdrant_container);

    // ---- (1) Create temp UR_CONFIG dir with test-specific config ----
    let config_dir = tempfile::tempdir().expect("failed to create temp config dir");
    let config_path = config_dir.path();
    let daemon_port = 19890u16; // spaced by 10 to avoid worker/builderd port overlap

    write_test_config(config_path, daemon_port, &names, &[]);

    // Create rag docs directory structure (normally done by `ur init`)
    let rag_docs_dir = config_path.join("rag").join("docs").join("rust");
    std::fs::create_dir_all(&rag_docs_dir).expect("failed to create rag docs dir");

    let ur = bin("ur");
    assert!(ur.exists(), "ur binary not found at {}", ur.display());
    let config_str = config_path.to_str().unwrap();
    let env = [("UR_CONFIG", config_str)];

    // Write small test docs directly instead of running `ur rag docs` (which
    // generates the full workspace — thousands of files, too slow for e2e).
    std::fs::write(
        rag_docs_dir.join("container.md"),
        "# Container Management\n\n\
         The container module manages Docker containers for worker agents.\n\
         It handles lifecycle operations: create, start, stop, and remove.\n\
         Each agent gets its own isolated container with mounted workspace.\n",
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

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- (2) ur start ----
        let up_output = run_cmd(&ur, &["start"], &env);
        assert!(
            up_output.status.success(),
            "ur start failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&up_output.stdout),
            String::from_utf8_lossy(&up_output.stderr),
        );

        // ---- (3) ur rag index — index test docs into Qdrant ----
        let index_output = run_cmd(&ur, &["rag", "index", "--language", "rust"], &env);
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

        // ---- (4) ur rag search — search indexed docs ----
        let search_output = run_cmd(
            &ur,
            &[
                "rag",
                "search",
                "container management",
                "--language",
                "rust",
            ],
            &env,
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
    }));

    // ---- (5) Always tear down server ----
    stop_server(&ur, config_path);

    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
