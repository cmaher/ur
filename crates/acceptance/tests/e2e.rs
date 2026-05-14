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
    /// Additional volume mounts for this project (raw mount strings, e.g. `"/host/path:/mnt/test:ro"`).
    mounts: Vec<String>,
    /// Optional `memory_dir` path to include in the project config.
    memory_dir: Option<String>,
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
        let mounts_line = if proj.mounts.is_empty() {
            String::new()
        } else {
            let quoted: Vec<String> = proj.mounts.iter().map(|s| format!("\"{}\"", s)).collect();
            format!("mounts = [{}]\n", quoted.join(", "))
        };
        let memory_dir_line = if let Some(ref md) = proj.memory_dir {
            format!("memory_dir = \"{md}\"\n")
        } else {
            String::new()
        };
        projects_toml.push_str(&format!(
            "\n[projects.{key}]\nrepo = \"{repo}\"\n{scripts}{memory_dir}\n[projects.{key}.container]\nimage = \"{image}\"\n{mounts}",
            key = proj.key,
            repo = proj.repo,
            scripts = scripts_line,
            memory_dir = memory_dir_line,
            image = image_ref,
            mounts = mounts_line,
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
    /// Host-only temp directory for mount regression tests (bind-mounted into workers).
    /// Kept alive so the directory exists for the duration of the test run.
    _host_mount_dir: tempfile::TempDir,
    /// Host directory used as memory dir for `memproj` memory mount tests.
    /// Kept alive so it exists for the duration of the test run.
    memory_dir: PathBuf,
    /// TempDir parent keeping `memory_dir` alive on disk.
    _memory_dir_parent: tempfile::TempDir,
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

/// Set up project fixtures for mount regression tests.
///
/// Creates two project entries:
/// - `mountproj`: has a host-only bind mount (`<tempdir>:/mnt/test:ro`). The source
///   directory is outside `$UR_CONFIG`/`$UR_WORKSPACE`/`$UR_LOGS_DIR`, so it is not
///   visible inside ur-server. Builderd (host-native) stat-checks and mounts it
///   directly. Regression test for the prim-usai3 bug class.
/// - `badmountproj`: has a mount source that does not exist. Verifies that builderd
///   returns a `FailedPrecondition` error naming the missing path.
///
/// Returns the `TempDir` for the host-only mount (must stay alive for the test) and
/// the two `ProjectEntry` values to include in `write_test_config`.
fn setup_mount_projects(config_path: &Path) -> (tempfile::TempDir, Vec<ProjectEntry>) {
    // Host-only mount dir — lives in the system temp directory, NOT under config_path,
    // so it is NOT mounted inside the ur-server container.
    let host_mount_dir = tempfile::tempdir().expect("failed to create host-only mount dir");
    let host_mount_path = host_mount_dir.path().to_path_buf();
    std::fs::write(host_mount_path.join("hello.txt"), "mount-test-content\n")
        .expect("failed to write hello.txt in host mount dir");

    // Nonexistent path for the missing-mount test. Remove any leftover from a prior run.
    let missing_mount_path = host_mount_path
        .parent()
        .unwrap_or(&host_mount_path)
        .join("ur-acceptance-nonexistent-mount-source");
    let _ = std::fs::remove_dir_all(&missing_mount_path);

    let mount_repos_dir = config_path.join("mount-repos");
    std::fs::create_dir_all(&mount_repos_dir).expect("failed to create mount-repos dir");
    let bare_repo_mount = create_bare_repo(&mount_repos_dir);
    let repo = bare_repo_mount.to_string_lossy().into_owned();

    let projects = vec![
        ProjectEntry {
            key: "mountproj".into(),
            repo: repo.clone(),
            image: "ur-worker".into(),
            hostexec_scripts: vec![],
            mounts: vec![format!("{}:/mnt/test:ro", host_mount_path.display())],
            memory_dir: None,
        },
        ProjectEntry {
            key: "badmountproj".into(),
            repo,
            image: "ur-worker".into(),
            hostexec_scripts: vec![],
            mounts: vec![format!("{}:/mnt/test:ro", missing_mount_path.display())],
            memory_dir: None,
        },
    ];
    (host_mount_dir, projects)
}

/// Holds the memory dir path and its parent TempDir for memory mount tests.
struct MemoryDirInfo {
    /// The actual memory directory path (pre-created by the test).
    path: PathBuf,
    /// The TempDir parent keeping `path` alive on disk.
    parent: tempfile::TempDir,
}

/// Set up the project entry and host memory dir for per-project memory bind mount tests.
///
/// Creates a `memproj` project entry with `memory_dir` pointing to a pre-created host
/// directory under a system temp dir. Seeds the directory with a `seed.md` file so
/// container-side reads can verify the host → container direction.
///
/// Returns:
/// - `MemoryDirInfo`: the memory dir path + its TempDir parent (must stay alive)
/// - `Vec<ProjectEntry>`: the `memproj` project entry to include in `write_test_config`
fn setup_memory_projects(config_path: &Path) -> (MemoryDirInfo, Vec<ProjectEntry>) {
    // Create the memory dir under a system temp dir (outside config_path so it is
    // NOT automatically mounted inside the ur-server container). The test pre-creates
    // it with UID 1000 (the test runner's UID), so the worker user (also UID 1000)
    // can write to it when Docker bind-mounts it.
    let parent = tempfile::tempdir().expect("failed to create memory dir parent");
    let memory_path = parent.path().join("memory");
    std::fs::create_dir_all(&memory_path).expect("failed to create memory dir");

    // Seed a file so the container can verify host → container direction.
    std::fs::write(memory_path.join("seed.md"), "hello-from-host\n")
        .expect("failed to write seed.md");

    // Create a bare repo for the memory project.
    let mem_repos_dir = config_path.join("mem-repos");
    std::fs::create_dir_all(&mem_repos_dir).expect("failed to create mem-repos dir");
    let bare_repo = create_bare_repo(&mem_repos_dir);
    let repo = bare_repo.to_string_lossy().into_owned();

    let projects = vec![ProjectEntry {
        key: "memproj".into(),
        repo,
        image: "ur-worker".into(),
        hostexec_scripts: vec![],
        mounts: vec![],
        memory_dir: Some(memory_path.to_string_lossy().into_owned()),
    }];

    (
        MemoryDirInfo {
            path: memory_path,
            parent,
        },
        projects,
    )
}

/// Timeout for the entire acceptance test run (10 minutes).
const TEST_TIMEOUT: Duration = Duration::from_secs(600);

/// Bare repositories and project entries for the test environment.
struct ProjectFixtures {
    projects: Vec<ProjectEntry>,
    skills_extra_toml: String,
    host_mount_dir: tempfile::TempDir,
    memory_dir: MemoryDirInfo,
}

/// Create a bare repo whose HEAD commit includes `ur-hooks/git/pre-commit`
/// containing the given sentinel string.
///
/// This exercises the in-repo hooks convention (`/workspace/ur-hooks/git/`).
fn create_bare_repo_with_git_hooks(parent_dir: &Path, pre_commit_sentinel: &str) -> PathBuf {
    let bare_repo = parent_dir.join("hook-repo.git");
    let staging = parent_dir.join("hook-staging");

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

    std::fs::write(staging.join("README.md"), "# Hook test repo\n")
        .expect("failed to write README");

    // Write in-repo git hook: ur-hooks/git/pre-commit
    let hooks_dir = staging.join("ur-hooks").join("git");
    std::fs::create_dir_all(&hooks_dir).expect("failed to create ur-hooks/git dir");
    let hook_path = hooks_dir.join("pre-commit");
    std::fs::write(&hook_path, pre_commit_sentinel).expect("failed to write pre-commit hook");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755))
        .expect("failed to set hook permissions");

    let output = Command::new("git")
        .args(["add", "."])
        .current_dir(&staging)
        .output()
        .expect("failed to git add");
    assert!(output.status.success(), "git add failed");

    let output = Command::new("git")
        .args(["commit", "-m", "initial commit with git hooks"])
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

/// Set up the `hookproj` project entry and host overlay hook directories.
///
/// Creates:
/// - A bare repo with an in-repo `ur-hooks/git/pre-commit` (sentinel A).
/// - `<config_path>/projects/hookproj/hooks/git/pre-commit` (sentinel B — overlay wins).
/// - `<config_path>/projects/hookproj/hooks/git/post-merge` (sentinel C — overlay-only file).
/// - `<config_path>/projects/hookproj/hooks/skills/my-hook.sh` (sentinel D — skill overlay).
///
/// The `projects/hookproj/hooks/` directory lives under `config_path`, which is the
/// test's `UR_CONFIG` directory. Builderd (host-native) stat-checks and mounts it directly
/// as `/var/ur/host-hooks/git:ro` in the worker container.
///
/// Returns the `ProjectEntry` for `hookproj`.
fn setup_hook_overlay_projects(config_path: &Path) -> Vec<ProjectEntry> {
    let hook_repos_dir = config_path.join("hook-repos");
    std::fs::create_dir_all(&hook_repos_dir).expect("failed to create hook-repos dir");
    let bare_repo = create_bare_repo_with_git_hooks(&hook_repos_dir, "sentinel-A\n");
    let repo = bare_repo.to_string_lossy().into_owned();

    // Create host overlay git hooks under the convention path.
    // Builderd will mount this as /var/ur/host-hooks/git:ro when the worker launches.
    let git_overlay_dir = config_path
        .join("projects")
        .join("hookproj")
        .join("hooks")
        .join("git");
    std::fs::create_dir_all(&git_overlay_dir).expect("failed to create git overlay hooks dir");
    std::fs::write(git_overlay_dir.join("pre-commit"), "sentinel-B\n")
        .expect("failed to write overlay pre-commit");
    std::fs::write(git_overlay_dir.join("post-merge"), "sentinel-C\n")
        .expect("failed to write overlay post-merge");

    // Create host overlay skill hooks under the convention path.
    // Builderd will mount this as /var/ur/host-hooks/skills:ro when the worker launches.
    let skills_overlay_dir = config_path
        .join("projects")
        .join("hookproj")
        .join("hooks")
        .join("skills");
    std::fs::create_dir_all(&skills_overlay_dir)
        .expect("failed to create skills overlay hooks dir");
    std::fs::write(skills_overlay_dir.join("my-hook.sh"), "sentinel-D\n")
        .expect("failed to write overlay my-hook.sh");

    vec![ProjectEntry {
        key: "hookproj".into(),
        repo,
        image: "ur-worker".into(),
        hostexec_scripts: vec![],
        mounts: vec![],
        memory_dir: None,
    }]
}

/// Create all bare git repositories, the test skill directory, and the complete
/// project entry list needed by `write_test_config`. All temp directories in the
/// returned struct must stay alive for the duration of the test.
fn create_project_fixtures(config_path: &Path, project_key: &str) -> ProjectFixtures {
    let bare_repo = create_bare_repo(config_path);

    let rust_repos_dir = config_path.join("rust-repos");
    std::fs::create_dir_all(&rust_repos_dir).expect("failed to create rust-repos dir");
    let bare_repo_rust = create_bare_repo(&rust_repos_dir);

    let script_repos_dir = config_path.join("script-repos");
    std::fs::create_dir_all(&script_repos_dir).expect("failed to create script-repos dir");
    let bare_repo_script = create_bare_repo_with_script(&script_repos_dir);

    // Absolute path so server (host-path resolution) and Docker both see the real path
    // without any %URCONFIG% → /config remapping.
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

    let (host_mount_dir, mount_projects) = setup_mount_projects(config_path);
    let (memory_dir, memory_projects) = setup_memory_projects(config_path);
    let hook_projects = setup_hook_overlay_projects(config_path);

    let mut projects = vec![
        ProjectEntry {
            key: project_key.into(),
            repo: bare_repo.to_string_lossy().into_owned(),
            image: "ur-worker".into(),
            hostexec_scripts: vec![],
            mounts: vec![],
            memory_dir: None,
        },
        ProjectEntry {
            key: "rustproj".into(),
            repo: bare_repo_rust.to_string_lossy().into_owned(),
            image: "ur-worker-rust".into(),
            hostexec_scripts: vec![],
            mounts: vec![],
            memory_dir: None,
        },
        ProjectEntry {
            key: "scriptproj".into(),
            repo: bare_repo_script.to_string_lossy().into_owned(),
            image: "ur-worker".into(),
            hostexec_scripts: vec!["host-only.sh".into()],
            mounts: vec![],
            memory_dir: None,
        },
    ];
    projects.extend(mount_projects);
    projects.extend(memory_projects);
    projects.extend(hook_projects);

    ProjectFixtures {
        projects,
        skills_extra_toml,
        host_mount_dir,
        memory_dir,
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

    // ---- (1a) Initialize file logging ----
    let logs_dir = config_path.join("logs");
    let _log_guard = init_test_logging(&logs_dir);

    let fixtures = create_project_fixtures(&config_path, project_key);

    write_test_config(
        &config_path,
        server_port,
        &names,
        &fixtures.projects,
        &fixtures.skills_extra_toml,
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
        _host_mount_dir: fixtures.host_mount_dir,
        memory_dir: fixtures.memory_dir.path,
        _memory_dir_parent: fixtures.memory_dir.parent,
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
        scenario_host_only_mount(&env);
        scenario_missing_mount_source(&env);
        scenario_memory_pool(&env);
        scenario_memory_workspace_with_project(&env, &config_path);
        scenario_memory_workspace_no_project(&env, &config_path);
        scenario_hook_overlay_precedence(&env);
    }));

    // ---- (4) Always tear down: force-remove leftover worker containers, then stop server ----
    for ticket in [
        "pool-test",
        "design-test-1",
        "design-test-2",
        "custom-model-test",
        "rust-image-test",
        "hotreload-test",
        "script-pool-test",
        "global-skill-test",
        "mount-test",
        "memory-pool-test",
        "memproj-ws-test",
        "nomem-ws-test",
        "hook-overlay-test",
    ] {
        force_remove_container(&env.runtime, &env.container_name(ticket));
    }
    // Manual workers use generated process_ids, not ticket IDs
    // workspace-mount scenario uses "workspace-man-0" (workspace dir basename + -man-0)
    force_remove_container(&env.runtime, &env.container_name("workspace-man-0"));
    // hostexec-script-workspace scenario uses "script-workspace-man-0"
    force_remove_container(&env.runtime, &env.container_name("script-workspace-man-0"));
    // Pool-based manual worker uses "{project_key}-man-0"
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

/// Workspace mount: verify `-m manual -w <path>` launches a manual worker, mounts the host
/// directory, and generates a process_id matching the `{basename}-man-0` pattern.
fn scenario_workspace_mount(env: &TestEnv) {
    let workspace_dir = env.config_path.join("workspace");
    let workspace_basename = workspace_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");
    let expected_process_id = format!("{workspace_basename}-man-0");
    let container_name = env.container_name(&expected_process_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Launch manual worker with workspace mount (no ticket_id) ----
        let workspace_str = workspace_dir.to_str().unwrap();
        let launch_output = run_cmd(
            &env.ur,
            &[
                "--output",
                "json",
                "worker",
                "launch",
                "-m",
                "manual",
                "-w",
                workspace_str,
            ],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch -m manual -w failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        // ---- Parse JSON and verify process_id pattern ----
        let launch_json: serde_json::Value = serde_json::from_slice(&launch_output.stdout)
            .expect("launch output should be valid JSON");
        let process_id = launch_json["data"]["worker_id"]
            .as_str()
            .expect("launch response should have data.worker_id")
            .to_owned();
        let man_prefix = format!("{workspace_basename}-man-");
        let slot_suffix = process_id.strip_prefix(&man_prefix).unwrap_or_else(|| {
            panic!(
                "process_id '{process_id}' should start with '{man_prefix}' \
                 (expected pattern: {workspace_basename}-man-{{digit}})"
            )
        });
        assert!(
            slot_suffix.chars().all(|c| c.is_ascii_digit()),
            "process_id suffix '{slot_suffix}' should be all digits in '{process_id}'"
        );
        assert_eq!(
            process_id, expected_process_id,
            "process_id should be '{expected_process_id}', got '{process_id}'"
        );

        wait_for_healthy(&env.runtime, &container_name);

        // ---- exec ur-ping inside container ----
        assert_ping_pong(&env.runtime, &container_name);

        // ---- Stop worker using auto-generated process_id ----
        let stop_output = run_cmd(&env.ur, &["worker", "stop", &process_id], &env_slice);
        assert!(
            stop_output.status.success(),
            "ur worker stop {process_id} failed.\nstdout: {}\nstderr: {}",
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
    // Manual-mode workspace workers have no project key, so the script registry
    // must deny all script invocations — even scripts that are declared for a
    // project that happens to share the workspace basename prefix.
    let process_id = "script-workspace-man-0";
    let container_name = env.container_name(process_id);
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

        // ---- Launch in manual mode with -w (no project key) ----
        let ws_str = ws_dir.to_str().unwrap();
        let launch_output = run_cmd(
            &env.ur,
            &["worker", "launch", "-m", "manual", "-w", ws_str],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch -m manual -w failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        wait_for_healthy(&env.runtime, &container_name);

        // ---- Scripts must be denied: no project key in manual-mode -w workers ----
        let marker_dir = tempfile::tempdir().expect("failed to create marker temp dir");
        let marker_path = marker_dir.path().join("marker.txt");
        let marker_str = marker_path.to_string_lossy().to_string();
        let denied_output = exec_in_container(
            &env.runtime,
            &container_name,
            &[
                "workertools",
                "host-exec",
                "--script",
                "/workspace/host-only.sh",
                "workspace-tag-67890",
                &marker_str,
            ],
        );
        assert_ne!(
            denied_output.status.code(),
            Some(0),
            "script should be denied for manual-mode worker (no project key).\nstdout: {}\nstderr: {}",
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

        // ---- Stop worker ----
        let stop_output = run_cmd(&env.ur, &["worker", "stop", process_id], &env_slice);
        assert!(
            stop_output.status.success(),
            "ur worker stop {process_id} failed.\nstdout: {}\nstderr: {}",
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
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "launch response should have a non-empty container_id.\nJSON: {launch_json}"
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

/// Verify that `git checkout -b <branch>` from inside a manual worker container
/// succeeds via the host-exec path.
///
/// Manual workers have `branch == ""` in their WorkerContext, so git.lua's
/// `branch_locked` gate must pass for `checkout`/`switch`. If a future change
/// accidentally reinstates the block, this function panics with a message that
/// explicitly names the lua transform block so the failure is unambiguous.
fn manual_verify_git_checkout_allowed(runtime: &str, container: &str) {
    let checkout_output = exec_in_container(
        runtime,
        container,
        &["git", "checkout", "-b", "acceptance-scratch"],
    );
    let stderr = String::from_utf8_lossy(&checkout_output.stderr);
    assert_eq!(
        checkout_output.status.code(),
        Some(0),
        "git checkout -b acceptance-scratch should succeed for a manual worker \
         (lua transform must not block checkout when branch == \"\").\n\
         If this fails with 'blocked git subcommand: checkout', the lua transform \
         is incorrectly treating the manual worker as branch-locked.\n\
         stdout: {}\nstderr: {stderr}",
        String::from_utf8_lossy(&checkout_output.stdout),
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
        manual_verify_git_checkout_allowed(&env.runtime, &container_name);
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

/// Host-only mount regression test (prim-usai3 bug class).
///
/// Launches a pool worker for "mountproj", which has a host-only bind mount configured
/// (`<tempdir>:/mnt/test:ro`). The mount source lives outside `$UR_CONFIG`,
/// `$UR_WORKSPACE`, and `$UR_LOGS_DIR` — so it is NOT visible inside the ur-server
/// container. Builderd (running natively on the host) stat-checks and bind-mounts the
/// path directly, so it must be reachable from the host without going through the server.
///
/// If this test were run against the pre-builderd code path (where the server called the
/// Docker socket from inside its container), the mount source would not exist at the
/// path the server sees (the server only sees `$UR_CONFIG`, `$UR_WORKSPACE`, `$UR_LOGS_DIR`),
/// and Docker would silently create an empty directory there instead of mounting the host path.
fn scenario_host_only_mount(env: &TestEnv) {
    let ticket_id = "mount-test";
    let container_name = env.container_name(ticket_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Launch pool worker for mountproj ----
        let launch_output = run_cmd(
            &env.ur,
            &["worker", "launch", "-p", "mountproj", ticket_id],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch -p mountproj failed (host-only mount should work via builderd).\n\
             stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        wait_for_healthy(&env.runtime, &container_name);

        // ---- Verify the bind mount is visible inside the container ----
        // hello.txt was written to the host-only tempdir before ur start.
        let ls_output = exec_in_container(
            &env.runtime,
            &container_name,
            &["ls", "/mnt/test/hello.txt"],
        );
        assert_exec_success(
            &ls_output,
            "bind-mounted file /mnt/test/hello.txt must be visible inside the worker container — \
             if this fails, the host-only mount was silently dropped (prim-usai3 regression)",
        );

        // ---- Stop worker ----
        let stop_output = run_cmd(&env.ur, &["worker", "stop", ticket_id], &env_slice);
        assert!(
            stop_output.status.success(),
            "ur worker stop (mount-test) failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&stop_output.stdout),
            String::from_utf8_lossy(&stop_output.stderr),
        );
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// Missing mount source test.
///
/// Attempts to launch a pool worker for "badmountproj", which has a mount source that
/// does not exist anywhere on the host. Builderd stat-checks each volume source before
/// calling `docker run`, so it must return `FailedPrecondition` with the missing path
/// in the error message, and the launch must fail.
///
/// This locks in the builderd-side validation behavior introduced to make prim-usai3
/// visible as an error rather than a silent empty mount.
fn scenario_missing_mount_source(env: &TestEnv) {
    let ticket_id = "missing-mount-test";
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    // ---- Launch should fail because the mount source does not exist ----
    let launch_output = run_cmd(
        &env.ur,
        &["worker", "launch", "-p", "badmountproj", ticket_id],
        &env_slice,
    );
    assert!(
        !launch_output.status.success(),
        "ur worker launch -p badmountproj should have FAILED due to missing mount source, \
         but it succeeded.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&launch_output.stdout),
        String::from_utf8_lossy(&launch_output.stderr),
    );

    // ---- Error output must name the missing path ----
    let stderr = String::from_utf8_lossy(&launch_output.stderr);
    assert!(
        stderr.contains("ur-acceptance-nonexistent-mount-source"),
        "error message should contain the missing path 'ur-acceptance-nonexistent-mount-source'.\n\
         Got stderr: {stderr}"
    );
}

/// Memory bind mount — pool launch axis.
///
/// Verifies the host → container and container → host directions for the per-project
/// memory directory feature when launched as a pool slot (`-p memproj`).
///
/// Flow:
/// 1. Host pre-seeds `seed.md` in the memory dir (done by `setup_memory_projects`).
/// 2. Pool worker is launched for `memproj`.
/// 3. Inside the container, `seed.md` is readable at the expected mount point.
/// 4. Inside the container, a new `from-container.md` file is written.
/// 5. Worker is stopped.
/// 6. On the host, `from-container.md` is readable with the expected content.
fn scenario_memory_pool(env: &TestEnv) {
    let ticket_id = "memory-pool-test";
    let container_name = env.container_name(ticket_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Launch pool worker for memproj ----
        let launch_output = run_cmd(
            &env.ur,
            &["worker", "launch", "-p", "memproj", ticket_id],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch -p memproj failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        wait_for_healthy(&env.runtime, &container_name);

        // ---- Axis 1: host → container: seed.md seeded by test must be readable ----
        let memory_container_path = "/home/worker/.claude/projects/-workspace/memory";
        let seed_path = format!("{memory_container_path}/seed.md");
        let cat_output = exec_in_container(&env.runtime, &container_name, &["cat", &seed_path]);
        assert_exec_success(
            &cat_output,
            &format!(
                "seed.md must be readable inside the container at {seed_path} — \
                 memory dir bind mount was not established"
            ),
        );
        let seed_content = String::from_utf8_lossy(&cat_output.stdout);
        assert_eq!(
            seed_content.trim(),
            "hello-from-host",
            "seed.md content mismatch — expected 'hello-from-host', got: {seed_content:?}"
        );

        // ---- Axis 2: container → host: write a file inside the container ----
        let from_container_path = format!("{memory_container_path}/from-container.md");
        let write_output = exec_in_container(
            &env.runtime,
            &container_name,
            &[
                "sh",
                "-c",
                &format!("echo 'hello-from-container' > {from_container_path}"),
            ],
        );
        assert_exec_success(
            &write_output,
            "writing from-container.md inside the container should succeed",
        );

        // ---- Stop worker ----
        let stop_output = run_cmd(&env.ur, &["worker", "stop", ticket_id], &env_slice);
        assert!(
            stop_output.status.success(),
            "ur worker stop failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&stop_output.stdout),
            String::from_utf8_lossy(&stop_output.stderr),
        );

        // ---- Verify container-written file is visible on the host ----
        let host_from_container = env.memory_dir.join("from-container.md");
        assert!(
            host_from_container.exists(),
            "from-container.md must exist on the host at {} after worker stop — \
             container → host direction of memory bind mount did not work",
            host_from_container.display()
        );
        let host_content = std::fs::read_to_string(&host_from_container)
            .expect("failed to read from-container.md");
        assert_eq!(
            host_content.trim(),
            "hello-from-container",
            "from-container.md host content mismatch: {host_content:?}"
        );
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// Memory bind mount — workspace-with-project axis.
///
/// Verifies the per-project memory directory feature when a workspace-mount worker
/// is launched with a ticket ID whose prefix matches a configured project key
/// (`memproj`). The project key is derived automatically from the ticket ID prefix,
/// so the project config (including `memory_dir`) applies.
///
/// Exercises the same host ↔ container round-trip as `scenario_memory_pool`.
fn scenario_memory_workspace_with_project(env: &TestEnv, config_path: &std::path::Path) {
    // "memproj-ws-test" prefix "memproj" matches the configured project key.
    let ticket_id = "memproj-ws-test";
    let container_name = env.container_name(ticket_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Prepare a workspace directory ----
        let ws_dir = config_path.join("memory-workspace");
        std::fs::create_dir_all(&ws_dir).expect("failed to create memory workspace dir");
        let _ = Command::new("git")
            .args(["init", ws_dir.to_str().unwrap()])
            .output();

        // ---- Launch with -w; project key derived from "memproj" prefix ----
        let launch_output = run_cmd(
            &env.ur,
            &[
                "worker",
                "launch",
                "-w",
                ws_dir.to_str().unwrap(),
                ticket_id,
            ],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch -w for {ticket_id} failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        wait_for_healthy(&env.runtime, &container_name);

        // ---- Verify seed.md is readable inside the container ----
        let memory_container_path = "/home/worker/.claude/projects/-workspace/memory";
        let seed_path = format!("{memory_container_path}/seed.md");
        let cat_output = exec_in_container(&env.runtime, &container_name, &["cat", &seed_path]);
        assert_exec_success(
            &cat_output,
            &format!(
                "seed.md must be readable at {seed_path} in workspace-with-project launch — \
                 memory dir bind mount was not established for derived project key"
            ),
        );
        let seed_content = String::from_utf8_lossy(&cat_output.stdout);
        assert_eq!(
            seed_content.trim(),
            "hello-from-host",
            "seed.md content mismatch in workspace launch: {seed_content:?}"
        );

        // ---- Write a file from inside the container ----
        let ws_from_container = format!("{memory_container_path}/from-ws-container.md");
        let write_output = exec_in_container(
            &env.runtime,
            &container_name,
            &[
                "sh",
                "-c",
                &format!("echo 'hello-from-ws-container' > {ws_from_container}"),
            ],
        );
        assert_exec_success(
            &write_output,
            "writing from-ws-container.md inside the container should succeed",
        );

        // ---- Stop worker ----
        let stop_output = run_cmd(&env.ur, &["worker", "stop", ticket_id], &env_slice);
        assert!(
            stop_output.status.success(),
            "ur worker stop failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&stop_output.stdout),
            String::from_utf8_lossy(&stop_output.stderr),
        );

        // ---- Verify container-written file is visible on the host ----
        let host_ws_file = env.memory_dir.join("from-ws-container.md");
        assert!(
            host_ws_file.exists(),
            "from-ws-container.md must exist on the host at {} after worker stop — \
             container → host direction of memory mount did not work in workspace launch",
            host_ws_file.display()
        );
        let host_content =
            std::fs::read_to_string(&host_ws_file).expect("failed to read from-ws-container.md");
        assert_eq!(
            host_content.trim(),
            "hello-from-ws-container",
            "from-ws-container.md content mismatch: {host_content:?}"
        );
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// Memory bind mount — workspace-only (no project) axis.
///
/// Verifies that when a workspace-mount worker is launched with a ticket ID whose
/// prefix does NOT match any configured project key, NO memory bind mount is
/// created. The container's memory directory should not exist or should be empty
/// (the memory dir is project-scoped; without a project there is no mount).
fn scenario_memory_workspace_no_project(env: &TestEnv, config_path: &std::path::Path) {
    // "nomem-ws-test" prefix "nomem" does not match any configured project key.
    let ticket_id = "nomem-ws-test";
    let container_name = env.container_name(ticket_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Prepare a workspace directory ----
        let ws_dir = config_path.join("nomem-workspace");
        std::fs::create_dir_all(&ws_dir).expect("failed to create nomem workspace dir");
        let _ = Command::new("git")
            .args(["init", ws_dir.to_str().unwrap()])
            .output();

        // ---- Launch with -w only; no project key derived ----
        let launch_output = run_cmd(
            &env.ur,
            &[
                "worker",
                "launch",
                "-w",
                ws_dir.to_str().unwrap(),
                ticket_id,
            ],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch -w for {ticket_id} (no project) failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        wait_for_healthy(&env.runtime, &container_name);

        // ---- Verify that the memory directory does NOT exist inside the container ----
        // When no project is associated with the launch, `memory_dir` is never resolved,
        // so the bind mount is absent. The container path should not exist.
        let memory_container_path = "/home/worker/.claude/projects/-workspace/memory";
        let ls_output = exec_in_container(
            &env.runtime,
            &container_name,
            &["ls", memory_container_path],
        );
        assert_ne!(
            ls_output.status.code(),
            Some(0),
            "memory dir {memory_container_path} must NOT exist in a no-project workspace launch — \
             a mount was unexpectedly established.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&ls_output.stdout),
            String::from_utf8_lossy(&ls_output.stderr),
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

/// Hook overlay precedence test.
///
/// Verifies that the two-layer merge (in-repo + host overlay) for git hooks and skill
/// hooks produces the correct result inside a live worker:
///
/// - `pre-commit` is present in both the in-repo (`ur-hooks/git/pre-commit`, sentinel A)
///   and the host overlay (`<config>/projects/hookproj/hooks/git/pre-commit`, sentinel B).
///   The overlay must win — sentinel B is expected in `/workspace/.git/hooks/pre-commit`.
///
/// - `post-merge` exists only in the host overlay (sentinel C). It must appear in
///   `/workspace/.git/hooks/post-merge`.
///
/// - Both installed hooks must be executable (`0o755`).
///
/// - `my-hook.sh` exists only in the host overlay for skills (sentinel D). It must appear
///   in `/home/worker/.claude/skill-hooks/my-hook.sh`.
///
/// The overlay directories are created by `setup_hook_overlay_projects` under the
/// convention path `<config_path>/projects/hookproj/hooks/{git,skills}/` before the
/// server starts. Builderd auto-discovers and mounts them as
/// `/var/ur/host-hooks/{git,skills}:ro` at worker launch time.
fn verify_hook_overlay_assertions(runtime: &str, container_name: &str) {
    // Axis 1: overlay wins on conflict (pre-commit)
    // In-repo sentinel A should be overwritten by overlay sentinel B.
    let pre_commit_output = exec_in_container(
        runtime,
        container_name,
        &["cat", "/workspace/.git/hooks/pre-commit"],
    );
    assert_exec_success(
        &pre_commit_output,
        "/workspace/.git/hooks/pre-commit must exist — hook overlay or in-repo copy failed",
    );
    let pre_commit_content = String::from_utf8_lossy(&pre_commit_output.stdout)
        .trim()
        .to_owned();
    assert_eq!(
        pre_commit_content, "sentinel-B",
        "pre-commit must contain overlay sentinel-B (overlay wins over in-repo sentinel-A), \
         got: {pre_commit_content:?}"
    );

    // Axis 2: overlay-only file appears (post-merge)
    let post_merge_output = exec_in_container(
        runtime,
        container_name,
        &["cat", "/workspace/.git/hooks/post-merge"],
    );
    assert_exec_success(
        &post_merge_output,
        "/workspace/.git/hooks/post-merge must exist — overlay-only file was not copied",
    );
    let post_merge_content = String::from_utf8_lossy(&post_merge_output.stdout)
        .trim()
        .to_owned();
    assert_eq!(
        post_merge_content, "sentinel-C",
        "post-merge must contain overlay sentinel-C (overlay-only file), got: {post_merge_content:?}"
    );

    // Axis 3: hook permissions are 0o755 — use `stat -c %a` (GNU/Linux) inside the container.
    let pre_commit_mode = exec_in_container(
        runtime,
        container_name,
        &["stat", "-c", "%a", "/workspace/.git/hooks/pre-commit"],
    );
    assert_exec_success(&pre_commit_mode, "stat of pre-commit hook must succeed");
    let mode_str = String::from_utf8_lossy(&pre_commit_mode.stdout)
        .trim()
        .to_owned();
    assert_eq!(
        mode_str, "755",
        "pre-commit hook must have 0o755 permissions, got: {mode_str:?}"
    );

    let post_merge_mode = exec_in_container(
        runtime,
        container_name,
        &["stat", "-c", "%a", "/workspace/.git/hooks/post-merge"],
    );
    assert_exec_success(&post_merge_mode, "stat of post-merge hook must succeed");
    let mode_str = String::from_utf8_lossy(&post_merge_mode.stdout)
        .trim()
        .to_owned();
    assert_eq!(
        mode_str, "755",
        "post-merge hook must have 0o755 permissions, got: {mode_str:?}"
    );

    // Axis 4: skill overlay-only file appears
    let skill_hook_output = exec_in_container(
        runtime,
        container_name,
        &["cat", "/home/worker/.claude/skill-hooks/my-hook.sh"],
    );
    assert_exec_success(
        &skill_hook_output,
        "/home/worker/.claude/skill-hooks/my-hook.sh must exist — \
         skill hook overlay-only file was not copied",
    );
    let skill_hook_content = String::from_utf8_lossy(&skill_hook_output.stdout)
        .trim()
        .to_owned();
    assert_eq!(
        skill_hook_content, "sentinel-D",
        "my-hook.sh must contain overlay sentinel-D (skill hook overlay-only file), \
         got: {skill_hook_content:?}"
    );
}

fn scenario_hook_overlay_precedence(env: &TestEnv) {
    let ticket_id = "hook-overlay-test";
    let container_name = env.container_name(ticket_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Launch pool worker for hookproj ----
        let launch_output = run_cmd(
            &env.ur,
            &["worker", "launch", "-p", "hookproj", ticket_id],
            &env_slice,
        );
        assert!(
            launch_output.status.success(),
            "ur worker launch -p hookproj failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        wait_for_healthy(&env.runtime, &container_name);

        verify_hook_overlay_assertions(&env.runtime, &container_name);

        // ---- Stop worker ----
        let stop_output = run_cmd(&env.ur, &["worker", "stop", ticket_id], &env_slice);
        assert!(
            stop_output.status.success(),
            "ur worker stop (hook-overlay-test) failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&stop_output.stdout),
            String::from_utf8_lossy(&stop_output.stderr),
        );
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}
