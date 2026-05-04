//! End-to-end acceptance tests for the Ur gRPC + workercmd architecture.
//!
//! All scenarios share a single `ur start` / `ur stop` cycle to avoid port
//! collisions between concurrent tests and to reduce total test runtime.
//!
//! The `e2e_all` test is the sole `#[test]` entry point. It:
//!   1. Sets up one shared `TestEnv` (config dir, bare repo, `ur start`)
//!   2. Calls each scenario sequentially as plain helper functions
//!   3. Tears down the server once at the end
//!
//! Gated behind `--features acceptance` so they never run in normal `cargo test`.
//! Requires:
//!   - Pre-built `ur` binary in `target/debug/`
//!   - Container images (`ur-server`, `ur-worker`) already built (tag via `UR_IMAGE_TAG`, default: `latest`)
//!   - A Docker-compatible container runtime
#![cfg(feature = "acceptance")]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::Duration;

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Read the image tag to use for all container images in tests.
/// Uses `UR_IMAGE_TAG` env var if set, otherwise defaults to "latest".
fn image_tag() -> String {
    std::env::var("UR_IMAGE_TAG").unwrap_or_else(|_| "latest".into())
}

static IMAGE_TAG: LazyLock<String> = LazyLock::new(image_tag);

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
    /// Container image alias (e.g. "ur-worker", "ur-worker-rust") or full reference.
    image: String,
    /// Paths to hostexec scripts declared for this project (e.g. `["host-only.sh"]`).
    hostexec_scripts: Vec<String>,
}

/// Configuration names for a test stack, preventing container/network collisions
/// between tests that may run concurrently.
struct TestNames {
    squid_hostname: String,
    postgres_hostname: String,
    network: String,
    worker_network: String,
    server_hostname: String,
    worker_prefix: String,
}

/// Build test names with a unique prefix derived from the run ID.
/// `label` differentiates test stacks within the same run (e.g., "default", "pool").
fn test_names(label: &str) -> TestNames {
    let id = &*RUN_ID;
    TestNames {
        squid_hostname: format!("ur-{id}-{label}-squid"),
        postgres_hostname: format!("ur-{id}-{label}-postgres"),
        network: format!("ur-{id}-{label}"),
        worker_network: format!("ur-{id}-{label}-workers"),
        server_hostname: format!("ur-{id}-{label}-server"),
        worker_prefix: format!("ur-{id}-{label}-worker-"),
    }
}

/// Write a test-specific ur.toml and supporting files.
///
/// `ur start` renders the compose file from its embedded template, replacing
/// network name and container name placeholders with values from the config.
/// Uses unique container names so the acceptance test stack never collides
/// with a real running ur stack or other test stacks.
///
/// `extra_toml` is appended verbatim to the generated toml content, allowing
/// callers to inject additional sections (e.g. `[skills.code]`) without
/// touching the shared base config.
fn write_test_config(
    config_dir: &Path,
    server_port: u16,
    names: &TestNames,
    projects: &[ProjectEntry],
    extra_toml: &str,
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

    let tag = &*IMAGE_TAG;
    let mut projects_toml = String::new();
    for proj in projects {
        // Resolve image alias to a full reference with the configured tag.
        // If the image already contains ':' or '/', it is a full reference and used as-is.
        let image_ref = if proj.image.contains(':') || proj.image.contains('/') {
            proj.image.clone()
        } else {
            format!("{}:{}", proj.image, tag)
        };
        let scripts_line = if proj.hostexec_scripts.is_empty() {
            String::new()
        } else {
            let quoted: Vec<String> = proj
                .hostexec_scripts
                .iter()
                .map(|s| format!("\"{}\"", s))
                .collect();
            format!("hostexec_scripts = [{}]\n", quoted.join(", "))
        };
        projects_toml.push_str(&format!(
            "\n[projects.{key}]\nrepo = \"{repo}\"\n{scripts}\n[projects.{key}.container]\nimage = \"{image}\"\n",
            key = proj.key,
            repo = proj.repo,
            scripts = scripts_line,
            image = image_ref,
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
         [ticket_db]\n\
         host = \"{postgres}\"\n\
         \n\
         [workflow_db]\n\
         host = \"{postgres}\"\n\
         \n\
         [worker_modes.custommodel]\n\
         base = \"code\"\n\
         skills = [\"implement\"]\n\
         model = \"my-custom-model\"\n\
         \n\
         {projects_toml}\n\
         {extra_toml}",
        workspace = workspace_dir.display(),
        compose = compose_file.display(),
        squid = names.squid_hostname,
        network = names.network,
        worker_network = names.worker_network,
        server = names.server_hostname,
        worker_prefix = names.worker_prefix,
        postgres = names.postgres_hostname,
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

    // Allow partial clones (--filter=blob:none) against this bare repo,
    // matching what GitHub serves by default.
    let output = Command::new("git")
        .args([
            "-C",
            bare_repo.to_str().unwrap(),
            "config",
            "uploadpack.allowFilter",
            "true",
        ])
        .output()
        .expect("failed to set uploadpack.allowFilter");
    assert!(
        output.status.success(),
        "git config uploadpack.allowFilter failed"
    );

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

/// Initialize file logging for acceptance tests.
///
/// Writes structured JSON logs to `<logs_dir>/acceptance.log`. The returned
/// guard must be held for the lifetime of the test — dropping it flushes and
/// stops the background writer.
fn init_test_logging(logs_dir: &Path) -> tracing_appender::non_blocking::WorkerGuard {
    std::fs::create_dir_all(logs_dir).expect("failed to create logs dir");
    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::NEVER)
        .filename_prefix("acceptance.log")
        .build(logs_dir)
        .expect("failed to create acceptance log appender");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .json()
                .with_target(true)
                .with_thread_ids(true)
                .with_writer(non_blocking),
        )
        .init();

    guard
}

/// Timeout for the entire acceptance test run (10 minutes).
const TEST_TIMEOUT: Duration = Duration::from_secs(600);

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

    // ---- (1a) Initialize file logging ----
    let logs_dir = config_path.join("logs");
    let _log_guard = init_test_logging(&logs_dir);

    // Create bare repo BEFORE writing config (config references it)
    let bare_repo = create_bare_repo(&config_path);

    // Write config with the pool project
    // Create a second bare repo for the rust-image project
    let rust_repos_dir = config_path.join("rust-repos");
    std::fs::create_dir_all(&rust_repos_dir).expect("failed to create rust-repos dir");
    let bare_repo_rust = create_bare_repo(&rust_repos_dir);

    // Create a bare repo for the hostexec script project (contains host-only.sh)
    let script_repos_dir = config_path.join("script-repos");
    std::fs::create_dir_all(&script_repos_dir).expect("failed to create script-repos dir");
    let bare_repo_script = create_bare_repo_with_script(&script_repos_dir);

    // Create a global skill directory for the global-skill-injection scenario.
    // The absolute path is used directly in ur.toml so the server (which resolves
    // absolute paths as host paths) and Docker (host daemon) both see the real
    // filesystem path without any %URCONFIG% → /config remapping.
    let skill_dir = config_path.join("skills").join("test-skill");
    std::fs::create_dir_all(&skill_dir).expect("failed to create test-skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "# Test Skill\nA skill for acceptance testing.\n",
    )
    .expect("failed to write SKILL.md");
    let skills_extra_toml = format!(
        "[skills.code]\ntest-skill = \"{skill_path}\"\n",
        skill_path = skill_dir.display(),
    );

    write_test_config(
        &config_path,
        server_port,
        &names,
        &[
            ProjectEntry {
                key: project_key.into(),
                repo: bare_repo.to_string_lossy().into_owned(),
                image: "ur-worker".into(),
                hostexec_scripts: vec![],
            },
            ProjectEntry {
                key: "rustproj".into(),
                repo: bare_repo_rust.to_string_lossy().into_owned(),
                image: "ur-worker-rust".into(),
                hostexec_scripts: vec![],
            },
            ProjectEntry {
                key: "scriptproj".into(),
                repo: bare_repo_script.to_string_lossy().into_owned(),
                image: "ur-worker".into(),
                hostexec_scripts: vec!["host-only.sh".into()],
            },
        ],
        &skills_extra_toml,
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

    // ---- (2) ur start, run scenarios, always tear down (with timeout) ----
    // The entire test body runs in a spawned thread with a 10-minute join
    // deadline. If the deadline expires, we panic — which triggers the
    // existing catch_unwind cleanup logic in each scenario.
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_scenarios(env, ur, config_path);
        }));
        let _ = tx.send(());
        result
    });

    match rx.recv_timeout(TEST_TIMEOUT) {
        Ok(()) => match handle.join().expect("scenario thread panicked") {
            Ok(()) => {}
            Err(e) => std::panic::resume_unwind(e),
        },
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!(
                "acceptance tests exceeded {}-minute timeout",
                TEST_TIMEOUT.as_secs() / 60
            );
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            // Thread dropped the sender without sending — it panicked before send.
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => std::panic::resume_unwind(e),
                Err(e) => std::panic::resume_unwind(e),
            }
        }
    }
}

/// Run all scenarios with a 10-minute timeout, cleaning up on completion or failure.
fn run_scenarios(env: TestEnv, ur: PathBuf, config_path: PathBuf) {
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
        scenario_custom_mode_model_override(&env);
        scenario_launch_without_project(&env);
        scenario_project_image_rust(&env);
        scenario_project_add_image_flag(&env);
        scenario_project_add_then_launch(&env);
        scenario_dispatch_creates_workflow(&env);
        scenario_ticket_close_preserves_workflow(&env);
        scenario_flow_list_and_cancel(&env);
        scenario_hostexec_script_pool(&env);
        scenario_hostexec_script_workspace(&env, &config_path);
        scenario_global_skill_injection(&env);
        scenario_worker_label_pr_status(&env);
        scenario_manual_worker(&env);
    }));

    // ---- (4) Always tear down: force-remove leftover worker containers, then stop server ----
    for ticket in [
        "ping-test",
        "pool-test",
        "design-test-1",
        "design-test-2",
        "custom-model-test",
        "rust-image-test",
        "hotreload-test",
        "script-pool-test",
        "scriptproj-ws-test",
        "global-skill-test",
    ] {
        force_remove_container(&env.runtime, &env.container_name(ticket));
    }
    // Manual worker uses a generated process_id (poolproj-man-0), not a ticket ID
    force_remove_container(
        &env.runtime,
        &env.container_name(&format!("{}-man-0", env.project_key)),
    );
    stop_server(&env.ur, &env.config_path);

    if let Err(e) = scenario_result {
        // Reprint the panic message near the end of output so it's visible
        // in tail-truncated logs (e.g., the pre-push hook shows only the last 30 lines).
        if let Some(msg) = e.downcast_ref::<String>() {
            eprintln!("\n=== SCENARIO FAILURE ===\n{msg}\n=== END ===\n");
        } else if let Some(msg) = e.downcast_ref::<&str>() {
            eprintln!("\n=== SCENARIO FAILURE ===\n{msg}\n=== END ===\n");
        }
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

/// Read a single env var from a running container via `printenv`.
/// Returns the trimmed value, panicking if the var is unset or the exec fails.
fn container_env_var(runtime: &str, container: &str, var: &str) -> String {
    let output = exec_in_container(runtime, container, &["printenv", var]);
    assert_exec_success(
        &output,
        &format!("printenv {var} should succeed in container {container}"),
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Assert that a running container has `UR_WORKER_MODEL=<expected>` and that
/// the running `claude` process was launched with `--model <expected>`.
///
/// Settings.json is NOT checked: Claude Code rewrites `~/.claude/settings.json`
/// on startup and silently drops the `model` key, so model selection is passed
/// via the `--model` CLI flag instead (see workerd `run_daemon_only`).
fn assert_worker_model(runtime: &str, container: &str, expected: &str) {
    let env_val = container_env_var(runtime, container, "UR_WORKER_MODEL");
    assert_eq!(
        env_val, expected,
        "container {container} should have UR_WORKER_MODEL={expected}, got {env_val:?}"
    );

    // Verify the launched claude command includes `--model <expected>`. We
    // grep tmux's pane history (the visible buffer) for the launch line.
    let pane_output = exec_in_container(
        runtime,
        container,
        &["tmux", "capture-pane", "-t", "agent", "-p", "-S", "-200"],
    );
    assert_exec_success(
        &pane_output,
        &format!("tmux capture-pane should succeed in container {container}"),
    );
    let pane = String::from_utf8_lossy(&pane_output.stdout);
    let needle = format!("claude --model {expected}");
    assert!(
        pane.contains(&needle),
        "container {container} tmux pane should show '{needle}', got pane:\n{pane}"
    );
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

        // ---- Verify code mode resolves UR_WORKER_MODEL=sonnet and settings.json "model": "sonnet" ----
        assert_worker_model(&env.runtime, &container_name, "sonnet");

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

        // ---- Verify design mode resolves UR_WORKER_MODEL=opus and settings.json "model": "opus" ----
        assert_worker_model(&env.runtime, &container_name_1, "opus");

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

/// Custom mode with explicit `model` override: verify `UR_WORKER_MODEL` and
/// settings.json inside the container reflect the custom model value from ur.toml.
fn scenario_custom_mode_model_override(env: &TestEnv) {
    let ticket_id = "custom-model-test";
    let container_name = env.container_name(ticket_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Launch worker with custom mode (defined in ur.toml as "custommodel") ----
        let launch_output = run_cmd(
            &env.ur,
            &[
                "worker",
                "launch",
                "-p",
                env.project_key,
                "-m",
                "custommodel",
                ticket_id,
            ],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch -p {} -m custommodel failed.\nstdout: {}\nstderr: {}",
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

        // ---- Verify custom model flows through ----
        assert_worker_model(&env.runtime, &container_name, "my-custom-model");

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

/// Launching without `-p` (no project) should fail with a clear error message.
fn scenario_launch_without_project(env: &TestEnv) {
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    // Launch without -p or -w should fail
    let launch_output = run_cmd(
        &env.ur,
        &["worker", "launch", "no-project-test"],
        &env_slice,
    );
    assert!(
        !launch_output.status.success(),
        "launch without -p should fail.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&launch_output.stdout),
        String::from_utf8_lossy(&launch_output.stderr),
    );

    let stderr = String::from_utf8_lossy(&launch_output.stderr);
    assert!(
        stderr.contains("-p") || stderr.contains("project"),
        "error message should mention -p or project.\nstderr: {stderr}"
    );
}

/// Launch with `image = "ur-worker-rust"` project config and verify the container uses the `ur-worker-rust` image.
fn scenario_project_image_rust(env: &TestEnv) {
    let ticket_id = "rust-image-test";
    let container_name = env.container_name(ticket_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Launch worker with rust-image project ----
        let launch_output = run_cmd(
            &env.ur,
            &["worker", "launch", "-p", "rustproj", ticket_id],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch -p rustproj failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        let launch_stdout = String::from_utf8_lossy(&launch_output.stdout);
        assert!(
            launch_stdout.contains(&container_name),
            "launch output should contain container name '{container_name}'.\nGot: {launch_stdout}"
        );

        wait_for_healthy(&env.runtime, &container_name);

        // ---- Verify the container is running the rust image ----
        // Inspect the container image to confirm it uses ur-worker-rust
        let inspect_output = Command::new(&env.runtime)
            .args(["inspect", "--format", "{{.Config.Image}}", &container_name])
            .output()
            .expect("failed to inspect container image");
        let image = String::from_utf8_lossy(&inspect_output.stdout)
            .trim()
            .to_string();
        assert!(
            image.contains("ur-worker-rust"),
            "container should use ur-worker-rust image, got: {image}"
        );

        // ---- Verify workspace has cloned content ----
        let ls_output = exec_in_container(
            &env.runtime,
            &container_name,
            &["ls", "/workspace/README.md"],
        );
        assert_exec_success(
            &ls_output,
            "rust pool slot should contain README.md from cloned repo",
        );

        // ---- exec ur-ping inside container ----
        assert_ping_pong(&env.runtime, &container_name);

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

/// Create a bare git repository with one commit containing README.md and a
/// hostexec fixture script (`host-only.sh`), suitable as a clone source.
/// Returns the path to the bare repo directory.
fn create_bare_repo_with_script(parent_dir: &Path) -> PathBuf {
    let fixture_script = workspace_root().join("crates/acceptance/tests/fixtures/host-only.sh");
    let bare_repo = parent_dir.join("script-repo.git");
    let staging = parent_dir.join("script-staging");

    let output = Command::new("git")
        .args(["init", "--bare", bare_repo.to_str().unwrap()])
        .output()
        .expect("failed to run git init --bare");
    assert!(output.status.success(), "git init --bare failed");

    let output = Command::new("git")
        .args([
            "-C",
            bare_repo.to_str().unwrap(),
            "config",
            "uploadpack.allowFilter",
            "true",
        ])
        .output()
        .expect("failed to set uploadpack.allowFilter");
    assert!(
        output.status.success(),
        "git config uploadpack.allowFilter failed"
    );

    let output = Command::new("git")
        .args([
            "clone",
            bare_repo.to_str().unwrap(),
            staging.to_str().unwrap(),
        ])
        .output()
        .expect("failed to clone bare repo into staging");
    assert!(output.status.success(), "git clone failed");

    let _ = Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&staging)
        .output();
    let _ = Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&staging)
        .output();

    std::fs::write(staging.join("README.md"), "# Script test repo\n")
        .expect("failed to write README");

    // Copy the fixture script into the repo
    let script_content = std::fs::read_to_string(&fixture_script).unwrap_or_else(|e| {
        panic!(
            "failed to read fixture script {}: {e}",
            fixture_script.display()
        )
    });
    let script_dest = staging.join("host-only.sh");
    std::fs::write(&script_dest, &script_content).expect("failed to write host-only.sh");
    // Make it executable
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script_dest, std::fs::Permissions::from_mode(0o755))
        .expect("failed to set script permissions");

    let output = Command::new("git")
        .args(["add", "README.md", "host-only.sh"])
        .current_dir(&staging)
        .output()
        .expect("failed to git add");
    assert!(output.status.success(), "git add failed");

    let output = Command::new("git")
        .args(["commit", "-m", "initial commit with script"])
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

/// Initialize a bare git repo at `path` suitable for use as a project in `ur project add`.
fn init_project_git_repo(path: &std::path::Path) {
    std::fs::create_dir_all(path).expect("failed to create repo dir");
    let git_init = Command::new("git")
        .args(["init", "--initial-branch=main"])
        .current_dir(path)
        .output()
        .expect("failed to git init");
    assert!(git_init.status.success(), "git init failed");
    let _ = Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(path)
        .output();
    let _ = Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .output();
    std::fs::write(path.join("README.md"), "# Add test\n").expect("write readme");
    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .output();
    let _ = Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(path)
        .output();
    let _ = Command::new("git")
        .args(["remote", "add", "origin", "git@github.com:test/addtest.git"])
        .current_dir(path)
        .output();
}

/// Verify `ur project add` CLI writes correct TOML with `[container]` section,
/// and that omitting `--image` defaults to `ur-worker`.
fn scenario_project_add_image_flag(env: &TestEnv) {
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let repo_dir = env.config_path.join("add-test-repo");
    init_project_git_repo(&repo_dir);

    // ---- `ur project add` without --image should succeed and default to ur-worker ----
    let no_image_output = run_cmd(
        &env.ur,
        &[
            "project",
            "add",
            repo_dir.to_str().unwrap(),
            "--key",
            "addtest-default",
        ],
        &env_slice,
    );
    assert!(
        no_image_output.status.success(),
        "project add without --image should succeed with default image.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&no_image_output.stdout),
        String::from_utf8_lossy(&no_image_output.stderr),
    );

    // ---- Verify the default image resolves to ur-worker:latest in TOML ----
    let toml_content =
        std::fs::read_to_string(env.config_path.join("ur.toml")).expect("failed to read ur.toml");
    assert!(
        toml_content.contains("[projects.addtest-default.container]"),
        "ur.toml should contain [projects.addtest-default.container] section.\nGot:\n{toml_content}"
    );
    assert!(
        toml_content.contains("image = \"ur-worker\""),
        "ur.toml should contain image = \"ur-worker\" for the default image project.\nGot:\n{toml_content}"
    );

    // ---- Clean up the default-image project ----
    let remove_default_output = run_cmd(
        &env.ur,
        &["project", "remove", "addtest-default", "--force"],
        &env_slice,
    );
    assert!(
        remove_default_output.status.success(),
        "project remove addtest-default failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&remove_default_output.stdout),
        String::from_utf8_lossy(&remove_default_output.stderr),
    );

    // ---- `ur project add --image rust` should succeed and write correct TOML ----
    let add_output = run_cmd(
        &env.ur,
        &[
            "project",
            "add",
            repo_dir.to_str().unwrap(),
            "--image",
            "ur-worker-rust",
            "--key",
            "addtest",
        ],
        &env_slice,
    );
    assert!(
        add_output.status.success(),
        "project add --image rust failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&add_output.stdout),
        String::from_utf8_lossy(&add_output.stderr),
    );

    // ---- Verify the TOML was written correctly ----
    let toml_content =
        std::fs::read_to_string(env.config_path.join("ur.toml")).expect("failed to read ur.toml");
    assert!(
        toml_content.contains("[projects.addtest.container]"),
        "ur.toml should contain [projects.addtest.container] section.\nGot:\n{toml_content}"
    );
    assert!(
        toml_content.contains("image = \"ur-worker-rust\""),
        "ur.toml should contain image = \"ur-worker-rust\" in the addtest project.\nGot:\n{toml_content}"
    );

    // ---- Clean up: remove the added project so it doesn't affect other tests ----
    let remove_output = run_cmd(
        &env.ur,
        &["project", "remove", "addtest", "--force"],
        &env_slice,
    );
    assert!(
        remove_output.status.success(),
        "project remove failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&remove_output.stdout),
        String::from_utf8_lossy(&remove_output.stderr),
    );
}

/// Helper: parse the ticket ID from `ur ticket create --output json` output.
fn parse_ticket_id_from_create(stdout: &[u8]) -> String {
    let json: serde_json::Value =
        serde_json::from_slice(stdout).expect("ticket create output should be valid JSON");
    json["data"]["id"]
        .as_str()
        .expect("ticket create output should have data.id")
        .to_owned()
}

/// Helper: get the ticket status from `ur ticket show --output json`.
fn get_ticket_status(ur: &Path, envs: &[(&str, &str)], ticket_id: &str) -> Option<String> {
    let output = run_cmd(ur, &["--output", "json", "ticket", "show", ticket_id], envs);
    if !output.status.success() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    json["data"]["ticket"]["status"]
        .as_str()
        .map(|s| s.to_owned())
}

/// Helper: create a ticket and return its ID.
fn create_test_ticket(env: &TestEnv, title: &str) -> String {
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();
    let create_output = run_cmd(
        &env.ur,
        &[
            "--output",
            "json",
            "ticket",
            "create",
            title,
            "-p",
            env.project_key,
        ],
        &env_slice,
    );
    assert!(
        create_output.status.success(),
        "ur ticket create failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&create_output.stdout),
        String::from_utf8_lossy(&create_output.stderr),
    );
    parse_ticket_id_from_create(&create_output.stdout)
}

/// Helper: launch a worker with dispatch (-d) and wait for it to become healthy.
fn launch_dispatched_worker(env: &TestEnv, ticket_id: &str, container_name: &str) {
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();
    let launch_output = run_cmd(
        &env.ur,
        &["worker", "launch", "-p", env.project_key, "-d", ticket_id],
        &env_slice,
    );
    assert!(
        launch_output.status.success(),
        "ur worker launch -p {} -d failed.\nstdout: {}\nstderr: {}",
        env.project_key,
        String::from_utf8_lossy(&launch_output.stdout),
        String::from_utf8_lossy(&launch_output.stderr),
    );
    wait_for_healthy(&env.runtime, container_name);
}

/// Helper: run `ur flow show` and return the parsed JSON, or None if the command failed.
fn flow_show(env: &TestEnv, ticket_id: &str) -> Option<serde_json::Value> {
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();
    let output = run_cmd(
        &env.ur,
        &["--output", "json", "flow", "show", ticket_id],
        &env_slice,
    );
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// Helper: run `ur flow list` and return the parsed JSON value.
fn flow_list(env: &TestEnv) -> serde_json::Value {
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();
    let output = run_cmd(&env.ur, &["--output", "json", "flow", "list"], &env_slice);
    assert!(
        output.status.success(),
        "ur flow list failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    serde_json::from_slice(&output.stdout).expect("flow list should return valid JSON")
}

/// Helper: run `ur flow cancel` and assert it succeeds.
fn flow_cancel(env: &TestEnv, ticket_id: &str) {
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();
    let output = run_cmd(
        &env.ur,
        &["--output", "json", "flow", "cancel", ticket_id],
        &env_slice,
    );
    assert!(
        output.status.success(),
        "ur flow cancel failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("flow cancel should return valid JSON");
    assert_eq!(
        json["data"]["kind"].as_str(),
        Some("cancelled"),
        "flow cancel should return 'cancelled' kind.\nJSON: {json}"
    );
}

/// Helper: check if a workflow for a given ticket_id exists in `ur flow list` output.
fn flow_list_contains(list_json: &serde_json::Value, ticket_id: &str) -> bool {
    list_json["data"]["workflows"]
        .as_array()
        .map(|workflows| {
            workflows
                .iter()
                .any(|w| w["ticket_id"].as_str() == Some(ticket_id))
        })
        .unwrap_or(false)
}

/// Dispatch creates workflow: create a ticket, launch a worker with `-d` (dispatch),
/// verify the launch succeeds (which implies the CreateWorkflow RPC succeeded and a
/// workflow row was created in the database).
fn scenario_dispatch_creates_workflow(env: &TestEnv) {
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    // ---- Create a ticket via the ticket service ----
    let create_output = run_cmd(
        &env.ur,
        &[
            "--output",
            "json",
            "ticket",
            "create",
            "Dispatch workflow test",
            "-p",
            env.project_key,
        ],
        &env_slice,
    );
    assert!(
        create_output.status.success(),
        "ur ticket create failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&create_output.stdout),
        String::from_utf8_lossy(&create_output.stderr),
    );
    let ticket_id = parse_ticket_id_from_create(&create_output.stdout);
    let container_name = env.container_name(&ticket_id);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Launch worker with dispatch (-d) ----
        // The -d flag calls CreateWorkflow RPC before launching.
        // If the workflow creation fails, the launch command itself fails.
        let launch_output = run_cmd(
            &env.ur,
            &["worker", "launch", "-p", env.project_key, "-d", &ticket_id],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch -p {} -d failed (workflow creation should succeed).\n\
             stdout: {}\nstderr: {}",
            env.project_key,
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        wait_for_healthy(&env.runtime, &container_name);

        // ---- Verify ticket is still open (dispatch does not close it) ----
        let status = get_ticket_status(&env.ur, &env_slice, &ticket_id);
        assert_eq!(
            status.as_deref(),
            Some("open"),
            "ticket should still be open after dispatch.\nticket_id: {ticket_id}"
        );

        // ---- Verify dispatching the same ticket again fails ----
        // The workflow table has a UNIQUE constraint on ticket_id, so a second
        // CreateWorkflow for the same ticket should fail.
        let dup_output = run_cmd(
            &env.ur,
            &["worker", "launch", "-p", env.project_key, "-d", &ticket_id],
            &env_slice,
        );
        assert!(
            !dup_output.status.success(),
            "second dispatch of the same ticket should fail (workflow already exists).\n\
             stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&dup_output.stdout),
            String::from_utf8_lossy(&dup_output.stderr),
        );

        // ---- Stop worker ----
        let stop_output = run_cmd(&env.ur, &["worker", "stop", &ticket_id], &env_slice);
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

/// Ticket close preserves workflow: dispatch a ticket (creating a workflow), use
/// `ur flow show` to verify the workflow exists, then close the ticket. Verify
/// the ticket is closed, the workflow still exists (so the push/PR phase can
/// still run), and that re-dispatching a closed ticket fails.
fn scenario_ticket_close_preserves_workflow(env: &TestEnv) {
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let ticket_id = create_test_ticket(env, "Close workflow test");
    let container_name = env.container_name(&ticket_id);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        launch_dispatched_worker(env, &ticket_id, &container_name);

        // ---- Verify workflow exists via ur flow show ----
        let flow_json =
            flow_show(env, &ticket_id).expect("ur flow show should succeed after dispatch");
        assert_eq!(
            flow_json["data"]["workflow"]["ticket_id"].as_str(),
            Some(ticket_id.as_str()),
            "flow show workflow should reference the dispatched ticket.\nJSON: {flow_json}"
        );

        // ---- Close the ticket (workflow should survive) ----
        let close_output = run_cmd(&env.ur, &["ticket", "close", &ticket_id], &env_slice);
        assert!(
            close_output.status.success(),
            "ur ticket close failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&close_output.stdout),
            String::from_utf8_lossy(&close_output.stderr),
        );

        // ---- Verify ticket is now closed ----
        let status = get_ticket_status(&env.ur, &env_slice, &ticket_id);
        assert_eq!(
            status.as_deref(),
            Some("closed"),
            "ticket should be closed after ur ticket close.\nticket_id: {ticket_id}"
        );

        // ---- Verify workflow still exists after ticket close ----
        assert!(
            flow_show(env, &ticket_id).is_some(),
            "ur flow show should succeed after ticket close (workflow preserved)"
        );

        // ---- Verify dispatching a closed ticket fails ----
        let dispatch_closed_output = run_cmd(
            &env.ur,
            &["worker", "launch", "-p", env.project_key, "-d", &ticket_id],
            &env_slice,
        );
        assert!(
            !dispatch_closed_output.status.success(),
            "dispatching a closed ticket should fail.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&dispatch_closed_output.stdout),
            String::from_utf8_lossy(&dispatch_closed_output.stderr),
        );

        // ---- Stop worker ----
        let stop_output = run_cmd(&env.ur, &["worker", "stop", &ticket_id], &env_slice);
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

/// Flow list and cancel: dispatch a ticket (creating a workflow), verify the
/// workflow appears in `ur flow list`, cancel it with `ur flow cancel`, and
/// verify it no longer appears in the list.
fn scenario_flow_list_and_cancel(env: &TestEnv) {
    let ticket_id = create_test_ticket(env, "Flow list cancel test");
    let container_name = env.container_name(&ticket_id);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        launch_dispatched_worker(env, &ticket_id, &container_name);

        // ---- Verify workflow appears in ur flow list ----
        let list_json = flow_list(env);
        assert!(
            flow_list_contains(&list_json, &ticket_id),
            "flow list should include workflow for ticket {ticket_id}.\nJSON: {list_json}"
        );

        // ---- Cancel workflow via ur flow cancel ----
        flow_cancel(env, &ticket_id);

        // ---- Verify cancelled workflow no longer appears in ur flow list (active only) ----
        let list_after = flow_list(env);
        assert!(
            !flow_list_contains(&list_after, &ticket_id),
            "flow list should not include cancelled workflow for {ticket_id}.\nJSON: {list_after}"
        );

        // ---- Verify ur flow show returns the workflow with cancelled status ----
        let show =
            flow_show(env, &ticket_id).expect("ur flow show should return cancelled workflow");
        assert_eq!(
            show["data"]["workflow"]["status"].as_str(),
            Some("cancelled"),
            "cancelled workflow should have status 'cancelled'.\nJSON: {show}"
        );

        // Worker is already killed by flow_cancel (CancelWorkflow kills the
        // associated worker container). Just force-remove to clean up.
        force_remove_container(&env.runtime, &container_name);
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// Verify that a hot-reloaded worker has correct workspace content, gRPC connectivity,
/// pool directory structure, then stop the worker and remove the project.
fn verify_hot_reloaded_worker(
    env: &TestEnv,
    container_name: &str,
    project_key: &str,
    ticket_id: &str,
    env_slice: &[(&str, &str)],
) {
    let ls_output = exec_in_container(
        &env.runtime,
        container_name,
        &["ls", "/workspace/README.md"],
    );
    assert_exec_success(
        &ls_output,
        "hot-reloaded project pool slot should contain README.md from cloned repo",
    );

    assert_ping_pong(&env.runtime, container_name);

    let pool_slot = env
        .config_path
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

    let stop_output = run_cmd(&env.ur, &["worker", "stop", ticket_id], env_slice);
    assert!(
        stop_output.status.success(),
        "ur worker stop failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&stop_output.stdout),
        String::from_utf8_lossy(&stop_output.stderr),
    );

    let remove_output = run_cmd(
        &env.ur,
        &["project", "remove", project_key, "--force"],
        env_slice,
    );
    assert!(
        remove_output.status.success(),
        "project remove failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&remove_output.stdout),
        String::from_utf8_lossy(&remove_output.stderr),
    );
}

/// Project add then launch: add a project via `ur project add` while the server
/// is running, then immediately launch a worker for that project without restart.
/// Verifies the hot-reload flow: config write → gRPC ReloadProjects → pool slot
/// acquisition for the newly added project.
fn scenario_project_add_then_launch(env: &TestEnv) {
    let ticket_id = "hotreload-test";
    let container_name = env.container_name(ticket_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();
    let project_key = "hotreload";

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Create a bare git repo and a working clone for the new project ----
        let repos_dir = env.config_path.join("hotreload-repos");
        std::fs::create_dir_all(&repos_dir).expect("failed to create hotreload-repos dir");
        let bare_repo = create_bare_repo(&repos_dir);

        // Clone the bare repo to get a working dir with origin pointing at it.
        // `ur project add` resolves the git remote origin from the path argument,
        // so we need a working clone rather than the bare repo itself.
        let clone_dir = repos_dir.join("hotreload-clone");
        let clone_output = Command::new("git")
            .args([
                "clone",
                bare_repo.to_str().unwrap(),
                clone_dir.to_str().unwrap(),
            ])
            .output()
            .expect("failed to clone bare repo");
        assert!(
            clone_output.status.success(),
            "git clone failed: {}",
            String::from_utf8_lossy(&clone_output.stderr)
        );

        // ---- Add the project via `ur project add` (triggers ReloadProjects RPC) ----
        let image_ref = format!("ur-worker:{}", &*IMAGE_TAG);
        let add_output = run_cmd(
            &env.ur,
            &[
                "project",
                "add",
                clone_dir.to_str().unwrap(),
                "--image",
                &image_ref,
                "--key",
                project_key,
            ],
            &env_slice,
        );
        assert!(
            add_output.status.success(),
            "ur project add failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&add_output.stdout),
            String::from_utf8_lossy(&add_output.stderr),
        );

        // Verify the add output confirms the server registered the project.
        // The exact text is "Server reloaded — project {key} now added".
        let add_stdout = String::from_utf8_lossy(&add_output.stdout);
        assert!(
            add_stdout.contains("Server reloaded"),
            "project add output should confirm server reload succeeded.\nGot: {add_stdout}"
        );

        // ---- Launch a worker for the newly added project (no server restart) ----
        let launch_output = run_cmd(
            &env.ur,
            &["worker", "launch", "-p", project_key, ticket_id],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch -p {project_key} failed \
             (hot-reload should make project available).\n\
             stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        let launch_stdout = String::from_utf8_lossy(&launch_output.stdout);
        assert!(
            launch_stdout.contains(&container_name),
            "launch output should contain container name '{container_name}'.\n\
             Got: {launch_stdout}"
        );

        wait_for_healthy(&env.runtime, &container_name);

        verify_hot_reloaded_worker(env, &container_name, project_key, ticket_id, &env_slice);
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// Invoke a declared hostexec script from inside a container and verify the
/// end-to-end dispatch flow.
///
/// - Runs `host-only.sh <tag> <marker_path>` via `workertools host-exec --script`.
/// - Asserts the exec exits 0 and stdout contains the expected "wrote tag" line.
/// - Asserts the marker file exists on the host and contains exactly `tag`.
fn assert_hostexec_script_works(
    runtime: &str,
    container: &str,
    tag: &str,
    marker_path: &std::path::Path,
) {
    let marker_str = marker_path.to_string_lossy();
    let script_output = exec_in_container(
        runtime,
        container,
        &[
            "workertools",
            "host-exec",
            "--script",
            "/workspace/host-only.sh",
            tag,
            &marker_str,
        ],
    );
    assert_exec_success(
        &script_output,
        &format!("workertools host-exec --script /workspace/host-only.sh {tag} should exit 0"),
    );

    let stdout = String::from_utf8_lossy(&script_output.stdout);
    assert!(
        stdout.contains("host-only: wrote tag"),
        "host-exec script stdout should contain 'host-only: wrote tag'.\nGot: {stdout}"
    );
    assert!(
        stdout.contains(tag),
        "stdout should contain the tag '{tag}'.\nGot: {stdout}"
    );

    assert!(
        marker_path.exists(),
        "marker file should exist at {} after script execution",
        marker_path.display()
    );
    let marker_content = std::fs::read_to_string(marker_path)
        .unwrap_or_else(|e| panic!("failed to read marker file: {e}"));
    assert_eq!(
        marker_content.trim(),
        tag,
        "marker file should contain the tag '{tag}', got: {marker_content:?}"
    );
}

/// Assert that invoking an undeclared script returns a non-zero exit and an
/// error message indicating the script is not allowed.
fn assert_hostexec_script_denied(runtime: &str, container: &str) {
    let denied_output = exec_in_container(
        runtime,
        container,
        &[
            "workertools",
            "host-exec",
            "--script",
            "/workspace/other-undeclared.sh",
        ],
    );
    assert_ne!(
        denied_output.status.code(),
        Some(0),
        "invoking an undeclared script should fail.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&denied_output.stdout),
        String::from_utf8_lossy(&denied_output.stderr),
    );
    let stderr = String::from_utf8_lossy(&denied_output.stderr);
    assert!(
        stderr.contains("not allowed")
            || stderr.contains("SCRIPT_NOT_ALLOWED")
            || stderr.contains("PermissionDenied"),
        "error message should indicate script is not allowed.\nstderr: {stderr}"
    );
}

/// Pool launch: verify that a declared hostexec script runs on the host, the
/// marker file appears with the correct tag, stdout/exit code propagate, and
/// that the original repo script in the pool slot is unmodified.
/// Also verifies that invoking an undeclared script is rejected.
fn scenario_hostexec_script_pool(env: &TestEnv) {
    let ticket_id = "script-pool-test";
    let container_name = env.container_name(ticket_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Launch worker for scriptproj ----
        let launch_output = run_cmd(
            &env.ur,
            &["worker", "launch", "-p", "scriptproj", ticket_id],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch -p scriptproj failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        let launch_stdout = String::from_utf8_lossy(&launch_output.stdout);
        assert!(
            launch_stdout.contains(&container_name),
            "launch output should contain container name '{container_name}'.\nGot: {launch_stdout}"
        );

        wait_for_healthy(&env.runtime, &container_name);

        // ---- Verify pool slot has host-only.sh from the cloned repo ----
        let pool_slot = env
            .config_path
            .join("workspace")
            .join("pool")
            .join("scriptproj")
            .join("0");
        let script_on_host = pool_slot.join("host-only.sh");
        assert!(
            script_on_host.exists(),
            "host-only.sh should be present in pool slot at {}",
            script_on_host.display()
        );

        // Read the original script content for later comparison.
        let original_script_content = std::fs::read_to_string(&script_on_host)
            .expect("failed to read original host-only.sh from pool slot");

        // ---- Create a temp marker dir on the host ----
        let marker_dir = tempfile::tempdir().expect("failed to create marker temp dir");
        let marker_path = marker_dir.path().join("marker.txt");

        // ---- Invoke the declared script from inside the container ----
        assert_hostexec_script_works(
            &env.runtime,
            &container_name,
            "pool-tag-12345",
            &marker_path,
        );

        // ---- Negative case: undeclared script is rejected ----
        assert_hostexec_script_denied(&env.runtime, &container_name);

        // ---- Verify original repo script on host is unmodified ----
        let script_after = std::fs::read_to_string(&script_on_host)
            .expect("failed to read host-only.sh after test");
        assert_eq!(
            original_script_content, script_after,
            "original host-only.sh in pool slot must be unmodified after the test"
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

/// Workspace-mount launch: verify that a declared hostexec script runs on the
/// host when the worker is launched with `-w` (workspace mount). The project
/// key is derived from the ticket ID prefix (`scriptproj`), giving the worker
/// the right allowlist. The host-side `host-only.sh` is present in the
/// workspace directory so the existence check passes.
fn scenario_hostexec_script_workspace(env: &TestEnv, config_path: &std::path::Path) {
    // Ticket ID prefix "scriptproj" matches the configured project key, so the
    // project key is automatically derived from the ID even under -w mode.
    let ticket_id = "scriptproj-ws-test";
    let container_name = env.container_name(ticket_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Prepare workspace directory with the fixture script ----
        let ws_dir = config_path.join("script-workspace");
        std::fs::create_dir_all(&ws_dir).expect("failed to create script workspace dir");

        // Initialize as a git repo (workspace-mount scenario needs git)
        let _ = Command::new("git")
            .args(["init", ws_dir.to_str().unwrap()])
            .output();

        // Copy the fixture script into the workspace so the existence check passes.
        let fixture_script = workspace_root().join("crates/acceptance/tests/fixtures/host-only.sh");
        let script_content = std::fs::read_to_string(&fixture_script)
            .unwrap_or_else(|e| panic!("failed to read fixture: {e}"));
        let script_dest = ws_dir.join("host-only.sh");
        std::fs::write(&script_dest, &script_content)
            .expect("failed to write host-only.sh to workspace");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_dest, std::fs::Permissions::from_mode(0o755))
            .expect("failed to set script permissions");

        // ---- Launch with -w; project key is derived from ticket ID prefix ----
        let ws_str = ws_dir.to_str().unwrap();
        let launch_output = run_cmd(
            &env.ur,
            &["worker", "launch", "-w", ws_str, ticket_id],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch -w failed for {ticket_id}.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        let launch_stdout = String::from_utf8_lossy(&launch_output.stdout);
        assert!(
            launch_stdout.contains(&container_name),
            "launch output should contain container name '{container_name}'.\nGot: {launch_stdout}"
        );

        wait_for_healthy(&env.runtime, &container_name);

        // ---- Create a temp marker dir on the host ----
        let marker_dir = tempfile::tempdir().expect("failed to create marker temp dir");
        let marker_path = marker_dir.path().join("marker.txt");

        // ---- Invoke the declared script from inside the container ----
        assert_hostexec_script_works(
            &env.runtime,
            &container_name,
            "workspace-tag-67890",
            &marker_path,
        );

        // ---- Verify original script in workspace is unmodified ----
        let script_after =
            std::fs::read_to_string(&script_dest).expect("failed to read host-only.sh after test");
        assert_eq!(
            script_content, script_after,
            "original host-only.sh in workspace must be unmodified after the test"
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

/// Global skill injection: verify that a skill declared in `[skills.code]` in ur.toml
/// is bind-mounted into `~/.claude/potential-skills/<name>/` and subsequently copied
/// to `~/.claude/skills/<name>/` by the `workerd init` step.
///
/// This exercises the full path:
///   ur.toml `[skills.code]` → `GlobalSkillsConfig` → `WorkerManager::merge_global_skills`
///   → `RunOptsBuilder::add_extra_skills` → bind mount → `workerd init` copy.
fn scenario_global_skill_injection(env: &TestEnv) {
    let ticket_id = "global-skill-test";
    let container_name = env.container_name(ticket_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Launch a code-mode pool worker ----
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

        // ---- Assert potential-skills bind mount is present ----
        // The bind mount should make SKILL.md visible at the potential-skills path.
        let potential_skill_path = "/home/worker/.claude/potential-skills/test-skill/SKILL.md";
        let ls_potential =
            exec_in_container(&env.runtime, &container_name, &["ls", potential_skill_path]);
        assert_exec_success(
            &ls_potential,
            &format!(
                "potential-skills bind mount should exist at {potential_skill_path} — \
                 check that [skills.code] in ur.toml is plumbed through to add_extra_skills"
            ),
        );

        // ---- Assert workerd init copied the skill to ~/.claude/skills/ ----
        // The `workerd init` step copies each name in UR_WORKER_SKILLS from
        // potential-skills/ → skills/, so the file must appear there too.
        let skills_path = "/home/worker/.claude/skills/test-skill/SKILL.md";
        let ls_skills = exec_in_container(&env.runtime, &container_name, &["ls", skills_path]);
        assert_exec_success(
            &ls_skills,
            &format!(
                "skills copy should exist at {skills_path} — \
                 check that test-skill appears in UR_WORKER_SKILLS so workerd init copies it"
            ),
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

/// Read the tmux `status-left` value from a running worker container.
///
/// Executes `tmux display-message -p -t agent '#{status-left}'` inside the
/// container and returns the trimmed string. Panics if the exec fails.
fn read_tmux_status_left(runtime: &str, container: &str) -> String {
    let output = Command::new(runtime)
        .args([
            "exec",
            container,
            "tmux",
            "display-message",
            "-p",
            "-t",
            "agent",
            "#{status-left}",
        ])
        .output()
        .unwrap_or_else(|e| panic!("failed to exec tmux display-message in {container}: {e}"));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Manual worker launch: verify auto-generated process_id, no branch checkout,
/// worker appears in list with mode "manual", and clean shutdown.
///
/// Flow:
/// 1. Launch with `-m manual -p <project>` (no ticket_id).
/// 2. Verify process_id in launch response matches `{project}-man-{digit}` regex.
/// 3. Verify pool slot git branch is NOT a worker-specific branch (stays on
///    master/main or detached HEAD).
/// 4. Verify worker appears in `ur worker list` with mode "manual".
/// 5. Stop and verify clean shutdown + slot released (stop succeeds).
fn manual_launch_and_verify_process_id(
    ur: &Path,
    project_key: &str,
    env_slice: &[(&str, &str)],
) -> String {
    let launch_output = run_cmd(
        ur,
        &[
            "--output",
            "json",
            "worker",
            "launch",
            "-m",
            "manual",
            "-p",
            project_key,
        ],
        env_slice,
    );
    assert!(
        launch_output.status.success(),
        "ur worker launch -m manual -p {project_key} failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&launch_output.stdout),
        String::from_utf8_lossy(&launch_output.stderr),
    );
    let launch_json: serde_json::Value =
        serde_json::from_slice(&launch_output.stdout).expect("launch output should be valid JSON");
    let process_id = launch_json["data"]["worker_id"]
        .as_str()
        .expect("launch response should have data.worker_id")
        .to_owned();
    let man_prefix = format!("{project_key}-man-");
    let slot_suffix = process_id.strip_prefix(&man_prefix).unwrap_or_else(|| {
        panic!(
            "process_id '{process_id}' should start with '{man_prefix}' \
             (expected pattern: {project_key}-man-{{digit}})"
        )
    });
    assert!(
        slot_suffix.chars().all(|c| c.is_ascii_digit()),
        "process_id suffix '{slot_suffix}' should be all digits in '{process_id}'"
    );
    assert!(
        launch_json["data"]["container_id"]
            .as_str()
            .map(|s| s.contains(&process_id))
            .unwrap_or(false),
        "container_id should contain process_id '{process_id}'.\nJSON: {launch_json}"
    );
    process_id
}

fn manual_verify_no_branch_checkout(
    config_path: &std::path::Path,
    project_key: &str,
    process_id: &str,
) {
    let pool_slot = config_path
        .join("workspace")
        .join("pool")
        .join(project_key)
        .join("0");
    assert!(
        pool_slot.join(".git").exists(),
        "pool slot should be a git repo at {}",
        pool_slot.display()
    );
    let branch_output = Command::new("git")
        .args([
            "-C",
            pool_slot.to_str().unwrap(),
            "symbolic-ref",
            "--short",
            "HEAD",
        ])
        .output()
        .expect("failed to run git symbolic-ref");
    let branch_name = if branch_output.status.success() {
        String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_owned()
    } else {
        "HEAD (detached)".to_owned()
    };
    assert!(
        !branch_name.contains(process_id),
        "pool slot should NOT be on a worker-specific branch after manual launch.\n\
         Expected branch NOT containing '{process_id}', got: '{branch_name}'"
    );
}

fn manual_verify_worker_in_list(ur: &Path, env_slice: &[(&str, &str)], process_id: &str) {
    let list_output = run_cmd(ur, &["--output", "json", "worker", "list"], env_slice);
    assert!(
        list_output.status.success(),
        "ur worker list failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&list_output.stdout),
        String::from_utf8_lossy(&list_output.stderr),
    );
    let list_json: serde_json::Value = serde_json::from_slice(&list_output.stdout)
        .expect("worker list output should be valid JSON");
    let workers = list_json["data"]
        .as_array()
        .expect("worker list data should be an array");
    let mw = workers
        .iter()
        .find(|w| w["worker_id"].as_str() == Some(process_id))
        .unwrap_or_else(|| panic!("worker list should contain '{process_id}'.\nlist: {list_json}"));
    assert_eq!(
        mw["mode"].as_str(),
        Some("manual"),
        "worker '{process_id}' should have mode 'manual', got: {:?}",
        mw["mode"]
    );
}

fn manual_stop_and_verify_gone(ur: &Path, env_slice: &[(&str, &str)], process_id: &str) {
    let stop_output = run_cmd(
        ur,
        &["--output", "json", "worker", "stop", process_id],
        env_slice,
    );
    assert!(
        stop_output.status.success(),
        "ur worker stop {process_id} failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&stop_output.stdout),
        String::from_utf8_lossy(&stop_output.stderr),
    );
    let list_after = run_cmd(ur, &["--output", "json", "worker", "list"], env_slice);
    assert!(
        list_after.status.success(),
        "ur worker list after stop failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&list_after.stdout),
        String::from_utf8_lossy(&list_after.stderr),
    );
    let list_after_json: serde_json::Value = serde_json::from_slice(&list_after.stdout)
        .expect("worker list after stop should be valid JSON");
    let still_running = list_after_json["data"]
        .as_array()
        .map(|workers| {
            workers
                .iter()
                .any(|w| w["worker_id"].as_str() == Some(process_id))
        })
        .unwrap_or(false);
    assert!(
        !still_running,
        "worker '{process_id}' should not appear in list after stop.\nlist: {list_after_json}"
    );
}

fn scenario_manual_worker(env: &TestEnv) {
    let expected_process_id = format!("{}-man-0", env.project_key);
    let container_name = env.container_name(&expected_process_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let process_id = manual_launch_and_verify_process_id(&env.ur, env.project_key, &env_slice);

        wait_for_healthy(&env.runtime, &container_name);

        manual_verify_no_branch_checkout(&env.config_path, env.project_key, &process_id);
        manual_verify_worker_in_list(&env.ur, &env_slice, &process_id);
        manual_stop_and_verify_gone(&env.ur, &env_slice, &process_id);
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// Worker tmux status-left label: verify bare label and PR label cases.
///
/// Flow:
/// 1. Create a ticket and dispatch a worker.
/// 2. Set `pr_number = 42` via `ur ticket set-meta` → spawns label refresh.
/// 3. Poll up to 20s for the label to update to `[<ticket_id> PR-42]`.
/// 4. Delete `pr_number` via `ur ticket delete-meta` → spawns label refresh.
/// 5. Poll up to 20s for the label to revert to `[<ticket_id>]`.
/// 6. Stop worker.
///
/// Both assertions use the set-meta / delete-meta trigger paths
/// (spawn_label_refresh_for_ticket) rather than waiting for the agent to
/// report idle — this avoids timing sensitivity around Claude Code startup.
fn scenario_worker_label_pr_status(env: &TestEnv) {
    let ticket_id = create_test_ticket(env, "Worker label PR status test");
    let container_name = env.container_name(&ticket_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Launch with dispatch so the workflow row is created ----
        launch_dispatched_worker(env, &ticket_id, &container_name);

        // ---- Set pr_number metadata → spawn_label_refresh_for_ticket pushes PR label ----
        let set_meta_output = run_cmd(
            &env.ur,
            &[
                "--output",
                "json",
                "ticket",
                "set-meta",
                &ticket_id,
                "pr_number",
                "42",
            ],
            &env_slice,
        );
        assert!(
            set_meta_output.status.success(),
            "ur ticket set-meta pr_number failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&set_meta_output.stdout),
            String::from_utf8_lossy(&set_meta_output.stderr),
        );

        // ---- Poll up to 20s for the label to include PR-42 ----
        let pr_expected = format!("[{ticket_id} PR-42]");
        let mut pr_ok = false;
        for _ in 0..80 {
            let label = read_tmux_status_left(&env.runtime, &container_name);
            if label == pr_expected {
                pr_ok = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        assert!(
            pr_ok,
            "expected PR status-left '{pr_expected}' within 20s after set-meta; \
             last value: '{}'",
            read_tmux_status_left(&env.runtime, &container_name),
        );

        // ---- Delete pr_number → spawn_label_refresh_for_ticket pushes bare label ----
        let delete_meta_output = run_cmd(
            &env.ur,
            &[
                "--output",
                "json",
                "ticket",
                "delete-meta",
                &ticket_id,
                "pr_number",
            ],
            &env_slice,
        );
        assert!(
            delete_meta_output.status.success(),
            "ur ticket delete-meta pr_number failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&delete_meta_output.stdout),
            String::from_utf8_lossy(&delete_meta_output.stderr),
        );

        // ---- Poll up to 20s for the label to revert to the bare form ----
        let bare_expected = format!("[{ticket_id}]");
        let mut bare_ok = false;
        for _ in 0..80 {
            let label = read_tmux_status_left(&env.runtime, &container_name);
            if label == bare_expected {
                bare_ok = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        assert!(
            bare_ok,
            "expected bare status-left '{bare_expected}' within 20s after delete-meta; \
             last value: '{}'",
            read_tmux_status_left(&env.runtime, &container_name),
        );

        // ---- Stop worker ----
        let stop_output = run_cmd(&env.ur, &["worker", "stop", &ticket_id], &env_slice);
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
