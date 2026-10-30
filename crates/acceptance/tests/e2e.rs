//! End-to-end acceptance tests for the Ur gRPC + workercmd architecture.
//!
//! The suite keeps only paths that need a real worker container, builderd, and
//! the server together — one e2e path per launch mode and agent image. Each
//! launched container is an *anchor* that also carries every cheap in-container
//! check for that configuration; everything else is covered by unit or server
//! integration tests (see `crates/acceptance/CLAUDE.md`).
//!
//! All scenarios share a single `ur start` / `ur stop` cycle to avoid port
//! collisions between concurrent tests and to reduce total test runtime.
//!
//! The `e2e_all` test is the sole `#[test]` entry point. It:
//!   1. Sets up one shared `TestEnv` (config dir, bare repos, `ur start`)
//!   2. Calls each scenario sequentially as plain helper functions
//!   3. Tears down the server once at the end
//!
//! Gated behind `--features acceptance` so they never run in normal `cargo test`.
//! Requires:
//!   - Pre-built `ur` binary in `target/debug/`
//!   - Container images (`ur-server`, `ur-worker-<agent>`) already built (tag via `UR_IMAGE_TAG`, default: `latest`)
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
            let logs_output = Command::new(runtime)
                .args(["logs", "--tail", "100", container])
                .output();
            let logs = logs_output.map_or_else(
                |e| format!("(failed to fetch logs: {e})"),
                |o| {
                    format!(
                        "stdout:\n{}\nstderr:\n{}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    )
                },
            );
            panic!(
                "container '{container}' did not become healthy after 30s \
                 (last status: {status}).\nContainer logs:\n{logs}"
            );
        }
    }
}

fn wait_for_container_file(runtime: &str, container: &str, path: &str) {
    for _ in 0..40 {
        let output = exec_in_container(runtime, container, &["test", "-f", path]);
        if output.status.success() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("timed out waiting for {path} in container {container}");
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

/// Extra project entries to append to ur.toml.
struct ProjectEntry {
    key: String,
    /// Git remote URL. Ignored when `local` is true.
    repo: String,
    /// Emit `local = true` and no `repo` — a repo-less local project.
    local: bool,
    /// Paths to hostexec scripts declared for this project (e.g. `["host-only.sh"]`).
    hostexec_scripts: Vec<String>,
    /// Additional volume mounts for this project (raw mount strings, e.g. `"/host/path:/mnt/test:ro"`).
    mounts: Vec<String>,
    /// Optional `memory_dir` path to include in the project config.
    memory_dir: Option<String>,
    /// Optional `brain_dir` path to include in the project config.
    brain_dir: Option<String>,
    /// Optional explicit forge platform override ("github" or "gitea").
    forge: Option<String>,
}

impl ProjectEntry {
    /// A plain pool-backed GitHub project for `repo`, with no optional settings.
    fn pool(key: &str, repo: &Path) -> Self {
        Self {
            key: key.into(),
            repo: repo.to_string_lossy().into_owned(),
            local: false,
            hostexec_scripts: vec![],
            mounts: vec![],
            memory_dir: None,
            brain_dir: None,
            forge: Some("github".into()),
        }
    }
}

/// Render the `[projects.<key>]` TOML blocks for all project entries.
///
/// Every project's `container.image` is the bare `ur-worker` alias: the server
/// resolves it against the launch's agent and `UR_IMAGE_TAG` (forwarded by
/// `ur start`), so a CI-tagged run launches `ur-worker-<agent>:ci-<hash>`.
/// Optional fields (`hostexec_scripts`, `mounts`, `memory_dir`, `brain_dir`,
/// `forge`) are emitted only when set.
fn render_projects_toml(projects: &[ProjectEntry]) -> String {
    let quoted_list = |values: &[String]| -> String {
        let quoted: Vec<String> = values.iter().map(|v| format!("\"{v}\"")).collect();
        quoted.join(", ")
    };
    let mut projects_toml = String::new();
    for proj in projects {
        // A local project declares `local = true` in place of `repo`.
        let mut body = if proj.local {
            "local = true\n".to_owned()
        } else {
            format!("repo = \"{}\"\n", proj.repo)
        };
        if let Some(ref forge) = proj.forge {
            body.push_str(&format!("forge = \"{forge}\"\n"));
        }
        if !proj.hostexec_scripts.is_empty() {
            body.push_str(&format!(
                "hostexec_scripts = [{}]\n",
                quoted_list(&proj.hostexec_scripts)
            ));
        }
        if let Some(ref memory_dir) = proj.memory_dir {
            body.push_str(&format!("memory_dir = \"{memory_dir}\"\n"));
        }
        if let Some(ref brain_dir) = proj.brain_dir {
            body.push_str(&format!("brain_dir = \"{brain_dir}\"\n"));
        }
        let mounts_line = if proj.mounts.is_empty() {
            String::new()
        } else {
            format!("mounts = [{}]\n", quoted_list(&proj.mounts))
        };
        projects_toml.push_str(&format!(
            "\n[projects.{key}]\n{body}\n[projects.{key}.container]\nimage = \"{alias}\"\n{mounts_line}",
            key = proj.key,
            alias = DEFAULT_IMAGE_ALIAS,
        ));
    }
    projects_toml
}

/// The image alias every test project configures (`ur_config::default_image_alias`).
const DEFAULT_IMAGE_ALIAS: &str = "ur-worker";

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
///
/// The base config also defines `[worker_modes.custommodel]` (a design-based mode
/// with an explicit model, used by `scenario_custom_design_mode`) and seeds a
/// `[worker_models.claude]` override (`manual = { model = "sonnet" }`) that
/// `scenario_manual_worker` asserts reaches the container.
fn write_test_config(
    config_dir: &Path,
    server_port: u16,
    names: &TestNames,
    projects: &[ProjectEntry],
    workspace_brain_dir: &Path,
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

    let projects_toml = render_projects_toml(projects);

    // `workspace_brain_dir` is a top-level key, so it must precede every table header.
    let toml_content = format!(
        "server_port = {server_port}\n\
         workspace = \"{workspace}\"\n\
         compose_file = \"{compose}\"\n\
         workspace_brain_dir = \"{workspace_brain}\"\n\
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
         base = \"design\"\n\
         skills = [\"implement\"]\n\
         model = \"my-custom-model\"\n\
         \n\
         [worker_models.claude]\n\
         manual = {{ model = \"sonnet\" }}\n\
         \n\
         {projects_toml}\n\
         {extra_toml}",
        workspace = workspace_dir.display(),
        compose = compose_file.display(),
        workspace_brain = workspace_brain_dir.display(),
        squid = names.squid_hostname,
        network = names.network,
        worker_network = names.worker_network,
        server = names.server_hostname,
        worker_prefix = names.worker_prefix,
        postgres = names.postgres_hostname,
    );
    std::fs::write(config_dir.join("ur.toml"), toml_content).expect("failed to write ur.toml");
}

/// Run a git command in `dir`, panicking with stderr if it fails.
fn git_in(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("failed to run git {}: {e}", args.join(" ")));
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Create a bare git repository under `parent_dir` with one commit, suitable as a
/// clone source. Returns the path to the bare repo directory.
///
/// The commit holds `README.md` plus each `(relative path, content)` in
/// `executables`, written with mode 0755 (in-repo hooks such as
/// `ur-hooks/git/pre-commit` or `ur-hooks/startup-bg/<name>`).
fn create_bare_repo(parent_dir: &Path, executables: &[(&str, &str)]) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(parent_dir).expect("failed to create repo parent dir");
    let bare_repo = parent_dir.join("test-repo.git");
    let staging = parent_dir.join("staging");

    git_in(parent_dir, &["init", "--bare", bare_repo.to_str().unwrap()]);
    // Allow partial clones (--filter=blob:none) against this bare repo,
    // matching what GitHub serves by default.
    git_in(&bare_repo, &["config", "uploadpack.allowFilter", "true"]);
    git_in(
        parent_dir,
        &[
            "clone",
            bare_repo.to_str().unwrap(),
            staging.to_str().unwrap(),
        ],
    );
    git_in(&staging, &["config", "user.email", "test@test.com"]);
    git_in(&staging, &["config", "user.name", "Test"]);

    std::fs::write(staging.join("README.md"), "# Test repo\n").expect("failed to write README");
    for (rel_path, content) in executables {
        let path = staging.join(rel_path);
        std::fs::create_dir_all(path.parent().unwrap()).expect("failed to create file dir");
        std::fs::write(&path, content).expect("failed to write repo file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("failed to make repo file executable");
    }

    git_in(&staging, &["add", "."]);
    git_in(&staging, &["commit", "-m", "initial commit"]);
    git_in(&staging, &["push", "origin", "HEAD"]);

    bare_repo
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
    /// Bare origin of `project_key`, so pushes from a worker can be verified on the host.
    project_bare_repo: PathBuf,
    /// Host directories bind-mounted into the `mountproj` worker.
    mount_dirs: MountDirs,
    /// Host directory configured as the top-level `workspace_brain_dir`, mounted at
    /// `/brain` for `-w` workers that have no project.
    workspace_brain_dir: PathBuf,
    /// TempDir parent keeping `workspace_brain_dir` alive on disk.
    _workspace_brain_dir_parent: tempfile::TempDir,
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

/// Host-side directories bind-mounted into the `mountproj` worker. All live under
/// the system temp directory — outside `$UR_CONFIG`, so not visible inside the
/// ur-server container — and are pre-created by the test runner (UID 1000, the same
/// UID as the worker user) so the worker can write to the read-write ones.
struct MountDirs {
    /// Parent TempDir keeping every directory below alive for the whole run.
    _parent: tempfile::TempDir,
    /// Project `memory_dir`, seeded with `seed.md`.
    memory: PathBuf,
    /// Project `brain_dir`, seeded with `seed.md`.
    brain: PathBuf,
}

/// Sentinels for the `mountproj` git-hook overlay precedence checks: the in-repo
/// `ur-hooks/git/pre-commit` (A) is shadowed by the host overlay's (B), `post-merge`
/// exists only in the overlay (C), and `my-hook.sh` is an overlay-only skill hook (D).
const IN_REPO_PRE_COMMIT: &str = "sentinel-A\n";

/// Set up the `mountproj` and `badmountproj` project entries.
///
/// `mountproj` is the one claude project carrying every host-side mount feature
/// at once, so a single container proves all of them:
/// - a host-only `mounts` entry (`<tempdir>:/mnt/test:ro`) that only builderd
///   (host-native) can stat and mount — regression test for the prim-usai3 bug
///   class, where the server's container view silently produced an empty dir;
/// - `memory_dir` and `brain_dir`, read-write round trips;
/// - a host hook overlay under `$UR_CONFIG/projects/mountproj/hooks/{git,skills}`;
/// - an in-repo `ur-hooks/git/pre-commit` (shadowed by the overlay) and an in-repo
///   background startup hook `ur-hooks/startup-bg/10-acceptance-marker`.
///
/// `badmountproj` mounts a source that does not exist, so builderd must refuse the
/// launch with `FailedPrecondition` naming the missing path.
fn setup_mount_projects(config_path: &Path) -> (MountDirs, Vec<ProjectEntry>) {
    let parent = tempfile::tempdir().expect("failed to create mount dirs parent");
    let host_only = parent.path().join("host-only");
    let memory = parent.path().join("memory");
    let brain = parent.path().join("brain");
    for dir in [&host_only, &memory, &brain] {
        std::fs::create_dir_all(dir).expect("failed to create mount dir");
    }
    std::fs::write(host_only.join("hello.txt"), "mount-test-content\n")
        .expect("failed to write hello.txt");
    std::fs::write(memory.join("seed.md"), "hello-from-host\n").expect("failed to seed memory");
    std::fs::write(brain.join("seed.md"), "hello-from-host-brain\n").expect("failed to seed brain");

    // Nonexistent path for the missing-mount test. Remove any leftover from a prior run.
    let missing_mount_path = parent
        .path()
        .parent()
        .unwrap_or(parent.path())
        .join("ur-acceptance-nonexistent-mount-source");
    let _ = std::fs::remove_dir_all(&missing_mount_path);

    let bare_repo = create_bare_repo(
        &config_path.join("mount-repos"),
        &[
            ("ur-hooks/git/pre-commit", IN_REPO_PRE_COMMIT),
            (
                "ur-hooks/startup-bg/10-acceptance-marker",
                "#!/bin/sh\ntouch .startup-background-hook-ran\n",
            ),
        ],
    );

    // Host overlay hooks under the convention path. Builderd mounts them as
    // /var/ur/host-hooks/{git,skills}:ro when the worker launches.
    let overlay = config_path.join("projects/mountproj/hooks");
    std::fs::create_dir_all(overlay.join("git")).expect("failed to create git overlay dir");
    std::fs::create_dir_all(overlay.join("skills")).expect("failed to create skills overlay dir");
    std::fs::write(overlay.join("git/pre-commit"), "sentinel-B\n")
        .expect("failed to write overlay pre-commit");
    std::fs::write(overlay.join("git/post-merge"), "sentinel-C\n")
        .expect("failed to write overlay post-merge");
    std::fs::write(overlay.join("skills/my-hook.sh"), "sentinel-D\n")
        .expect("failed to write overlay my-hook.sh");

    let mountproj = ProjectEntry {
        mounts: vec![format!("{}:/mnt/test:ro", host_only.display())],
        memory_dir: Some(memory.to_string_lossy().into_owned()),
        brain_dir: Some(brain.to_string_lossy().into_owned()),
        ..ProjectEntry::pool("mountproj", &bare_repo)
    };
    let badmountproj = ProjectEntry {
        mounts: vec![format!("{}:/mnt/test:ro", missing_mount_path.display())],
        ..ProjectEntry::pool("badmountproj", &bare_repo)
    };

    (
        MountDirs {
            _parent: parent,
            memory,
            brain,
        },
        vec![mountproj, badmountproj],
    )
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

/// Seed a placeholder Codex credentials file so `check_credentials_seeded`
/// (`crates/server/src/grpc.rs`) does not reject a codex-agent launch.
///
/// Acceptance scenarios never drive a real Codex model turn — they only
/// assert on the launch path (agent resolution, in-container layout, dispatch
/// phrasing) — so a placeholder blob is enough. `check_credentials_seeded`
/// only checks that the file exists and is at least 10 bytes; it never reads
/// its content. See `AgentType::host_credentials_path` for why this lands at
/// `$UR_CONFIG/codex/auth.json`, and `crates/acceptance/CLAUDE.md` for the
/// documented decision.
fn seed_dummy_codex_credentials(config_path: &Path) {
    let codex_dir = config_path.join("codex");
    std::fs::create_dir_all(&codex_dir).expect("failed to create codex credentials dir");
    std::fs::write(
        codex_dir.join("auth.json"),
        "{\"acceptance-test\":\"dummy-token\"}",
    )
    .expect("failed to write dummy codex auth.json");
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

/// Holds a brain dir path and its parent TempDir.
struct BrainDirInfo {
    /// The actual brain directory path (pre-created by the test).
    path: PathBuf,
    /// The TempDir parent keeping `path` alive on disk.
    parent: tempfile::TempDir,
}

/// Set up the host directory used as the top-level `workspace_brain_dir`.
///
/// This is the brain mounted for workers launched with `-w` and no project. Like
/// the `mountproj` dirs, it lives under a system temp dir (outside
/// `config_path`, so it is not visible inside the ur-server container) and is
/// pre-created by the test runner (UID 1000, same as the worker user). It is seeded
/// with `ws-seed.md` so the container can verify the host → container direction.
fn setup_workspace_brain() -> BrainDirInfo {
    let parent = tempfile::tempdir().expect("failed to create workspace brain parent");
    let brain_path = parent.path().join("workspace-brain");
    std::fs::create_dir_all(&brain_path).expect("failed to create workspace brain dir");
    std::fs::write(
        brain_path.join("ws-seed.md"),
        "hello-from-workspace-brain\n",
    )
    .expect("failed to write ws-seed.md");

    BrainDirInfo {
        path: brain_path,
        parent,
    }
}

/// Timeout for the entire acceptance test run (10 minutes).
const TEST_TIMEOUT: Duration = Duration::from_secs(600);

/// Key of the repo-less local project (`local = true`) used by
/// `scenario_local_project`. Its directory basename must match so that
/// `ur worker launch -m manual -w .` derives the project from the cwd.
const LOCAL_PROJECT_KEY: &str = "localproj";

/// Bare repositories and project entries for the test environment.
struct ProjectFixtures {
    projects: Vec<ProjectEntry>,
    project_bare_repo: PathBuf,
    skills_extra_toml: String,
    mount_dirs: MountDirs,
    /// Host dir wired up as the top-level `workspace_brain_dir`.
    workspace_brain_dir: BrainDirInfo,
}

/// Project whose startup-hook overlay `scenario_failing_startup_hook` breaks. It is
/// dedicated to that scenario so the failed launch cannot disturb the slots of a
/// project another scenario depends on.
const FAILING_HOOK_PROJECT_KEY: &str = "failhookproj";

/// Write a host overlay skill hook for `project_key`, so `InitSkillHooksManager`
/// actually creates the agent's skill-hooks dir — it is only created when a hooks
/// source exists — and the codex/agy layout checks can assert it.
fn write_skill_hook_overlay(config_path: &Path, project_key: &str) {
    let hooks_dir = config_path.join(format!("projects/{project_key}/hooks/skills"));
    std::fs::create_dir_all(&hooks_dir).expect("failed to create skill hooks overlay dir");
    std::fs::write(hooks_dir.join("my-hook.sh"), "sentinel\n")
        .expect("failed to write skill hook overlay");
}

/// Create every bare git repository, the test skill directory, and the complete
/// project entry list needed by `write_test_config`. All temp directories in the
/// returned struct must stay alive for the duration of the test.
///
/// Every project uses the bare `ur-worker` alias (see `render_projects_toml`);
/// `codexproj` and `agyproj` exist only so their workers can be launched with
/// `--agent codex` / `--agent agy` against their own repos and skill-hook overlays.
fn create_project_fixtures(config_path: &Path, project_key: &str) -> ProjectFixtures {
    let project_bare_repo = create_bare_repo(config_path, &[]);
    let fail_hook_repo = create_bare_repo(&config_path.join("fail-hook-repos"), &[]);
    let codex_repo = create_bare_repo(&config_path.join("codex-repos"), &[]);
    let agy_repo = create_bare_repo(&config_path.join("agy-repos"), &[]);
    write_skill_hook_overlay(config_path, "codexproj");
    write_skill_hook_overlay(config_path, "agyproj");

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

    let (mount_dirs, mount_projects) = setup_mount_projects(config_path);

    let mut projects = vec![
        ProjectEntry::pool(project_key, &project_bare_repo),
        ProjectEntry::pool(FAILING_HOOK_PROJECT_KEY, &fail_hook_repo),
        ProjectEntry::pool("codexproj", &codex_repo),
        ProjectEntry::pool("agyproj", &agy_repo),
        // Relies purely on auto-detection from its repository URL (no explicit
        // `forge`): a local bare path is not github.com, so it resolves to Gitea.
        ProjectEntry {
            forge: None,
            ..ProjectEntry::pool("giteaautoproj", &project_bare_repo)
        },
        // A repo-less local project (`local = true`), declared with the same
        // per-project affordances a pool-backed project gets. Used by
        // `scenario_local_project`.
        ProjectEntry {
            repo: String::new(),
            local: true,
            hostexec_scripts: vec!["host-only.sh".into()],
            forge: None,
            ..ProjectEntry::pool(LOCAL_PROJECT_KEY, &project_bare_repo)
        },
    ];
    projects.extend(mount_projects);

    ProjectFixtures {
        projects,
        project_bare_repo,
        skills_extra_toml,
        mount_dirs,
        workspace_brain_dir: setup_workspace_brain(),
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
        &fixtures.workspace_brain_dir.path,
        &fixtures.skills_extra_toml,
    );
    write_git_hostexec_override(&config_path);

    let ur = bin("ur");
    assert!(ur.exists(), "ur binary not found at {}", ur.display());

    let env = TestEnv {
        _config_dir: config_dir,
        config_path: config_path.clone(),
        ur: ur.clone(),
        runtime,
        names,
        project_key,
        project_bare_repo: fixtures.project_bare_repo,
        mount_dirs: fixtures.mount_dirs,
        workspace_brain_dir: fixtures.workspace_brain_dir.path,
        _workspace_brain_dir_parent: fixtures.workspace_brain_dir.parent,
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

/// Start the shared stack, run every scenario, and always tear down.
///
/// Order matters for one coupling: `scenario_manual_worker` expects to land in
/// `poolproj` slot 0 (`poolproj-man-0`, `pool/poolproj/0`), so it runs right after
/// `scenario_pool_launch` (which releases slot 0 on stop) and before
/// `scenario_dispatch_creates_workflow`, whose `flow cancel` path is the one
/// poolproj teardown that does not go through `ur worker stop`.
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
        scenario_manual_worker(&env);
        scenario_custom_design_mode(&env);
        scenario_failing_startup_hook(&env);
        scenario_local_project(&env);
        scenario_dispatch_creates_workflow(&env);
        scenario_project_gitea_auto_detection(&env);
        scenario_mounts(&env);
        scenario_missing_mount_source(&env);
        scenario_codex_manual_worker(&env);
        scenario_agy_manual_worker(&env);
    }));

    // ---- (4) Always tear down: force-remove leftover worker containers, then stop server ----
    teardown_worker_containers(&env);
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

/// Force-remove every worker container the scenarios may have left running.
///
/// Ticket-ID-named containers are listed explicitly; dispatch scenarios name their
/// containers after generated ticket IDs, so a prefix sweep over this run's worker
/// prefix catches those too.
fn teardown_worker_containers(env: &TestEnv) {
    for ticket in [
        "pool-test",
        "custom-design-test",
        "startup-hook-failure-test",
        "gitea-auto-detect-test",
        "mount-test",
        "agy-manual-worker-test",
    ] {
        force_remove_container(&env.runtime, &env.container_name(ticket));
    }
    // Manual workers use generated process_ids, not ticket IDs:
    // - workspace mount: "{workspace basename}-man-0"
    // - pool manual worker: "{project_key}-man-0"
    // - local project (-p and -w): "{key}-man-{key}"
    force_remove_container(&env.runtime, &env.container_name("workspace-man-0"));
    force_remove_container(
        &env.runtime,
        &env.container_name(&format!("{}-man-0", env.project_key)),
    );
    force_remove_container(
        &env.runtime,
        &env.container_name(&local_manual_process_id(LOCAL_PROJECT_KEY)),
    );
    force_remove_by_prefix(&env.runtime, &env.names.worker_prefix);
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// `{{.Config.Image}}` of a container — the image reference `docker run` received.
fn container_image(runtime: &str, container: &str) -> String {
    let output = Command::new(runtime)
        .args(["inspect", "--format", "{{.Config.Image}}", container])
        .output()
        .expect("failed to inspect container image");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Assert a container runs `ur-worker-<agent>:<UR_IMAGE_TAG>` — the bare
/// `ur-worker` alias resolved by the server against the agent and the image tag.
fn assert_container_image(runtime: &str, container: &str, agent: &str) {
    assert_eq!(
        container_image(runtime, container),
        format!("ur-worker-{agent}:{}", &*IMAGE_TAG),
        "container {container} should run the {agent} image at the stack's image tag"
    );
}

/// The `ur worker list` entry for `worker_id`, panicking if it is absent.
fn worker_list_entry(env: &TestEnv, worker_id: &str) -> serde_json::Value {
    let list_output = run_cmd(&env.ur, &["--output", "json", "worker", "list"], &env.env());
    assert!(
        list_output.status.success(),
        "ur worker list failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&list_output.stdout),
        String::from_utf8_lossy(&list_output.stderr),
    );
    let list_json: serde_json::Value = serde_json::from_slice(&list_output.stdout)
        .expect("worker list output should be valid JSON");
    list_json["data"]
        .as_array()
        .expect("worker list data should be an array")
        .iter()
        .find(|w| w["worker_id"].as_str() == Some(worker_id))
        .cloned()
        .unwrap_or_else(|| panic!("worker list should contain '{worker_id}'.\nlist: {list_json}"))
}

/// Assert `ur worker list` reports `agent_type == expected` for `worker_id`.
fn assert_agent_type(env: &TestEnv, worker_id: &str, expected: &str) {
    let worker = worker_list_entry(env, worker_id);
    assert_eq!(
        worker["agent_type"].as_str(),
        Some(expected),
        "worker '{worker_id}' should have agent_type '{expected}', got: {:?}",
        worker["agent_type"]
    );
}

/// Assert every path in `paths` exists inside the container (`test -e`).
fn assert_container_paths_exist(runtime: &str, container: &str, paths: &[&str]) {
    for path in paths {
        let output = exec_in_container(runtime, container, &["test", "-e", path]);
        assert_exec_success(&output, &format!("{path} should exist in {container}"));
    }
}

/// The base image's agent-agnostic `.agent-shared/` layout, baked into
/// `worker-base` that every per-agent image extends.
const AGENT_SHARED_PATHS: &[&str] = &[
    "/home/worker/.agent-shared/potential-skills",
    "/home/worker/.agent-shared/instructions/code.md",
    "/home/worker/.agent-shared/shared-instructions",
];

/// Run `ur worker stop <id>` and assert it succeeds.
fn stop_worker(env: &TestEnv, worker_id: &str) {
    let stop_output = run_cmd(&env.ur, &["worker", "stop", worker_id], &env.env());
    assert!(
        stop_output.status.success(),
        "ur worker stop {worker_id} failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&stop_output.stdout),
        String::from_utf8_lossy(&stop_output.stderr),
    );
}

/// Run `ur worker launch <args>` and assert it succeeds, returning its stdout.
fn launch_worker(env: &TestEnv, args: &[&str]) -> String {
    let mut full_args = vec!["worker", "launch"];
    full_args.extend_from_slice(args);
    let launch_output = run_cmd(&env.ur, &full_args, &env.env());
    assert!(
        launch_output.status.success(),
        "ur {} failed.\nstdout: {}\nstderr: {}",
        full_args.join(" "),
        String::from_utf8_lossy(&launch_output.stdout),
        String::from_utf8_lossy(&launch_output.stderr),
    );
    String::from_utf8_lossy(&launch_output.stdout).into_owned()
}

/// Run `ur --output json worker launch <args>`, assert it succeeds, and return
/// the parsed response (`data.worker_id` carries the generated process ID).
fn launch_worker_json(env: &TestEnv, args: &[&str]) -> serde_json::Value {
    let mut full_args = vec!["--output", "json", "worker", "launch"];
    full_args.extend_from_slice(args);
    let launch_output = run_cmd(&env.ur, &full_args, &env.env());
    assert!(
        launch_output.status.success(),
        "ur {} failed.\nstdout: {}\nstderr: {}",
        full_args.join(" "),
        String::from_utf8_lossy(&launch_output.stdout),
        String::from_utf8_lossy(&launch_output.stderr),
    );
    serde_json::from_slice(&launch_output.stdout).expect("launch output should be valid JSON")
}

/// Write `content` to `container_path` from inside the container.
fn write_in_container(runtime: &str, container: &str, container_path: &str, content: &str) {
    let output = exec_in_container(
        runtime,
        container,
        &["sh", "-c", &format!("echo '{content}' > {container_path}")],
    );
    assert_exec_success(
        &output,
        &format!("writing {container_path} inside {container} should succeed"),
    );
}

/// Assert `container_path` inside the container holds exactly `expected` (trimmed).
fn assert_container_file(runtime: &str, container: &str, container_path: &str, expected: &str) {
    let output = exec_in_container(runtime, container, &["cat", container_path]);
    assert_exec_success(
        &output,
        &format!("{container_path} must be readable inside {container}"),
    );
    let content = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        content.trim(),
        expected,
        "{container_path} content mismatch in {container}"
    );
}

/// Assert the host file at `path` holds exactly `expected` (trimmed).
fn assert_host_file(path: &Path, expected: &str) {
    let content = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "container-written file not on host at {}: {e}",
            path.display()
        )
    });
    assert_eq!(
        content.trim(),
        expected,
        "{} content mismatch",
        path.display()
    );
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

    // Verify the running claude process was launched with `--model <expected>`.
    // Claude Code clears the screen on startup, so the launch command is not
    // reliably visible in tmux scrollback. Read /proc/*/cmdline instead.
    let needle = format!("--model {expected}");
    let grep_output = exec_in_container(
        runtime,
        container,
        &["sh", "-c", "cat /proc/*/cmdline 2>/dev/null | tr '\\0' ' '"],
    );
    assert_exec_success(
        &grep_output,
        &format!("reading /proc cmdlines should succeed in container {container}"),
    );
    let cmdlines = String::from_utf8_lossy(&grep_output.stdout);
    assert!(
        cmdlines.contains(&needle),
        "container {container} should have a process with '{needle}', got:\n{cmdlines}"
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

    let override_probe =
        exec_in_container(runtime, container, &["git", "acceptance-override-probe"]);
    let override_stderr = String::from_utf8_lossy(&override_probe.stderr);
    assert!(
        override_stderr.contains("discovered git override active"),
        "the filename-discovered git.lua should shadow the built-in transform.\n\
         stderr: {override_stderr}"
    );
}

fn write_git_hostexec_override(config_path: &Path) {
    let hostexec_dir = config_path.join("hostexec");
    std::fs::create_dir_all(&hostexec_dir).expect("failed to create hostexec directory");
    let default_git = include_str!("../../server/src/hostexec/default_scripts/git.lua");
    let override_script = format!(
        r#"{default_git}
local default_transform = transform
function transform(command, args, working_dir, worker_context)
    if args[1] == "acceptance-override-probe" then
        error("discovered git override active", 0)
    end
    return default_transform(command, args, working_dir, worker_context)
end
"#
    );
    std::fs::write(hostexec_dir.join("git.lua"), override_script)
        .expect("failed to write discovered git override");
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

/// Run `git push --no-verify origin HEAD` through the git hostexec shim.
fn git_push_no_verify(runtime: &str, container: &str) -> std::process::Output {
    exec_in_container(
        runtime,
        container,
        &["git", "push", "--no-verify", "origin", "HEAD"],
    )
}

/// Verify the gh shim is generated in bidi mode and that the gh transform runs
/// server-side. Endpoint-by-endpoint `gh api` validation is unit-tested against
/// the Lua transform (`lua_transform.rs`); this pins one blocked call end to end.
///
/// No GitHub credentials are needed: the transform rejects before `gh` is ever
/// spawned on the host, so a block is observable as an "is not allowed" message.
fn assert_gh_hostexec(runtime: &str, container: &str) {
    // The shim must carry --bidi, otherwise workertools never forwards stdin
    // and `gh api --input -` silently receives nothing.
    let shim = exec_in_container(runtime, container, &["cat", "/home/worker/.local/bin/gh"]);
    assert_exec_success(&shim, "reading the gh shim should work");
    let shim_content = String::from_utf8_lossy(&shim.stdout);
    assert!(
        shim_content.contains("--bidi"),
        "gh shim must be generated with --bidi so stdin is forwarded.\nGot: {shim_content}"
    );

    let merge = exec_in_container(
        runtime,
        container,
        &["gh", "api", "repos/owner/repo/pulls/7/merge", "-X", "POST"],
    );
    let merge_err = String::from_utf8_lossy(&merge.stderr);
    assert!(
        merge_err.contains("is not allowed"),
        "the merge endpoint must be blocked by the gh transform.\nstderr: {merge_err}"
    );

    // A GitHub project gets gh by default and no tea shim.
    let tea_shim = exec_in_container(runtime, container, &["ls", "/home/worker/.local/bin/tea"]);
    assert!(
        !tea_shim.status.success(),
        "a GitHub project should NOT receive the tea shim"
    );
}

/// Verify git pushes from a pool worker: `--no-verify` is blocked by default, and a
/// plain `git push origin HEAD` really lands the worker branch on the bare origin.
fn assert_git_push(env: &TestEnv, container: &str) {
    let blocked = git_push_no_verify(&env.runtime, container);
    let blocked_stderr = String::from_utf8_lossy(&blocked.stderr);
    assert!(
        !blocked.status.success() && blocked_stderr.contains("blocked flag: --no-verify"),
        "git push --no-verify should be blocked without allow_no_verify.\nstdout: {}\nstderr: {blocked_stderr}",
        String::from_utf8_lossy(&blocked.stdout),
    );

    let branch_output = exec_in_container(
        &env.runtime,
        container,
        &["git", "rev-parse", "--abbrev-ref", "HEAD"],
    );
    assert_exec_success(&branch_output, "git rev-parse should work in pool slot");
    let branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_owned();

    let push = exec_in_container(&env.runtime, container, &["git", "push", "origin", "HEAD"]);
    assert_exec_success(
        &push,
        "git push origin HEAD should succeed for a pool worker",
    );

    let remote = Command::new("git")
        .args(["ls-remote", "--heads"])
        .arg(&env.project_bare_repo)
        .arg(&branch)
        .output()
        .expect("failed to run git ls-remote");
    assert!(
        String::from_utf8_lossy(&remote.stdout).contains(&format!("refs/heads/{branch}")),
        "pushed branch '{branch}' should exist on the bare origin.\nls-remote: {}",
        String::from_utf8_lossy(&remote.stdout)
    );
}

// ---------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------

/// Workspace mount: `-m manual -w <path>` with no project launches a manual worker
/// named `{basename}-man-0`, on the fallback image resolved at the stack's image
/// tag, with the top-level `workspace_brain_dir` mounted read-write at `/brain`.
fn scenario_workspace_mount(env: &TestEnv) {
    let workspace_dir = env.config_path.join("workspace");
    let expected_process_id = "workspace-man-0";
    let container_name = env.container_name(expected_process_id);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ---- Launch manual worker with workspace mount (no ticket_id) ----
        let launch_json = launch_worker_json(
            env,
            &["-m", "manual", "-w", workspace_dir.to_str().unwrap()],
        );
        assert_eq!(
            launch_json["data"]["worker_id"].as_str(),
            Some(expected_process_id),
            "a -w launch with no project is named after the workspace basename"
        );

        wait_for_healthy(&env.runtime, &container_name);

        // ---- No project configures an image: the claude fallback at the stack's tag ----
        assert_container_image(&env.runtime, &container_name, "claude");

        // ---- workspace_brain_dir: host → container and container → host ----
        assert_container_file(
            &env.runtime,
            &container_name,
            "/brain/ws-seed.md",
            "hello-from-workspace-brain",
        );
        write_in_container(
            &env.runtime,
            &container_name,
            "/brain/from-ws-container.md",
            "written-by-ws-worker",
        );
        assert_host_file(
            &env.workspace_brain_dir.join("from-ws-container.md"),
            "written-by-ws-worker",
        );

        stop_worker(env, expected_process_id);
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// Pool launch: the default claude code-mode anchor. One container covers clone,
/// model, git/gh hostexec and Lua validation, git push, squid filtering, global
/// skill injection, the `.agent-shared/` base layout, `agent_type` in
/// `ur worker list`, and the host pool directory structure.
fn scenario_pool_launch(env: &TestEnv) {
    let ticket_id = "pool-test";
    let container_name = env.container_name(ticket_id);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let launch_stdout = launch_worker(env, &["-p", env.project_key, ticket_id]);
        assert!(
            launch_stdout.contains(&container_name),
            "launch output should contain container name '{container_name}'.\nGot: {launch_stdout}"
        );

        wait_for_healthy(&env.runtime, &container_name);

        // ---- Clone, model, agent identity ----
        assert_container_paths_exist(&env.runtime, &container_name, &["/workspace/README.md"]);
        assert_worker_model(&env.runtime, &container_name, "sonnet");
        assert_agent_type(env, ticket_id, "claude");

        // ---- Global [skills.code] skill: bind-mounted, then copied by workerd init ----
        // ---- plus the base image's .agent-shared/ layout ----
        let mut paths = vec![
            "/home/worker/.agent-shared/potential-skills/test-skill/SKILL.md",
            "/home/worker/.claude/skills/test-skill/SKILL.md",
        ];
        paths.extend_from_slice(AGENT_SHARED_PATHS);
        assert_container_paths_exist(&env.runtime, &container_name, &paths);

        // ---- Hostexec: git and gh transforms, pushes ----
        assert_git_hostexec(&env.runtime, &container_name);
        assert_gh_hostexec(&env.runtime, &container_name);
        assert_git_push(env, &container_name);

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
            pool_slot.join(".git").exists(),
            "pool slot should be a git repo at {}",
            pool_slot.display()
        );
        assert!(
            pool_slot.join("README.md").exists(),
            "pool slot should contain README.md from clone"
        );

        stop_worker(env, ticket_id);
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// Design-based custom mode: `[worker_modes.custommodel]` has `base = "design"` and an
/// explicit `model`, so one launch proves both the design strategy (the worker
/// gets the project's shared slot — not a numbered one — with the cloned content)
/// and that a custom mode's `model` reaches the container.
fn scenario_custom_design_mode(env: &TestEnv) {
    let ticket_id = "custom-design-test";
    let container_name = env.container_name(ticket_id);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let launch_stdout = launch_worker(
            env,
            &["-p", env.project_key, "-m", "custommodel", ticket_id],
        );
        assert!(
            launch_stdout.contains(&container_name),
            "launch output should contain container name '{container_name}'.\nGot: {launch_stdout}"
        );

        wait_for_healthy(&env.runtime, &container_name);

        assert_worker_model(&env.runtime, &container_name, "my-custom-model");
        assert_container_paths_exist(&env.runtime, &container_name, &["/workspace/README.md"]);

        stop_worker(env, ticket_id);
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// Manual process_id for a local project launched as `-p <key> -w <dir>` where the
/// directory basename is the key.
///
/// `generate_manual_process_id` builds `{project_key}-man-{slot_name}`, and for a
/// launch that supplies both `-p` and `-w` the slot name is the workspace basename
/// — so this is `{key}-man-{key}`, not `{key}-man-0`.
fn local_manual_process_id(key: &str) -> String {
    format!("{key}-man-{key}")
}

/// Create the local project's host directory: a plain directory that is
/// deliberately *not* a git repo, holding the hostexec fixture script.
///
/// The basename matches the project key so `-w .` can derive the project from cwd.
fn setup_local_project_dir(env: &TestEnv, key: &str) -> PathBuf {
    let project_dir = env.config_path.join(key);
    std::fs::create_dir_all(&project_dir).expect("failed to create local project dir");
    let fixture_script = workspace_root().join("crates/acceptance/tests/fixtures/host-only.sh");
    let script_content = std::fs::read_to_string(&fixture_script)
        .unwrap_or_else(|e| panic!("failed to read fixture: {e}"));
    let script_dest = project_dir.join("host-only.sh");
    std::fs::write(&script_dest, &script_content).expect("failed to write host-only.sh");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script_dest, std::fs::Permissions::from_mode(0o755))
        .expect("failed to set script permissions");
    assert!(
        !project_dir.join(".git").exists(),
        "local project dir must not be a git repo — that is the case under test"
    );
    project_dir
}

/// Launch the one supported worker shape for a local project — `-m manual` with
/// `-w <dir>` — and verify it comes up on the project's configured image with the
/// project's hostexec scripts allowed (and only those), exactly as a pool-backed
/// project would.
fn launch_and_verify_local_worker(
    env: &TestEnv,
    key: &str,
    project_dir: &Path,
    container_name: &str,
) {
    // `--output json` so the generated process_id can be asserted from
    // `data.worker_id`. The human-readable "Launching worker ..." line is built from
    // the ticket_id, which is empty in manual mode, so it never carries the real name.
    let launch_json = launch_worker_json(
        env,
        &[
            "-m",
            "manual",
            "-p",
            key,
            "-w",
            project_dir.to_str().unwrap(),
        ],
    );
    assert_eq!(
        launch_json["data"]["worker_id"].as_str(),
        Some(local_manual_process_id(key).as_str()),
        "a -p + -w manual launch derives its process_id as \
         {{project_key}}-man-{{workspace basename}}"
    );

    wait_for_healthy(&env.runtime, container_name);

    // The project's configured image must apply, exactly as for a pool project.
    assert_container_image(&env.runtime, container_name, "claude");

    // The project's declared hostexec scripts must be allowed, and only those.
    let marker_dir = tempfile::tempdir().expect("failed to create marker temp dir");
    let marker_path = marker_dir.path().join("marker.txt");
    assert_hostexec_script_works(
        &env.runtime,
        container_name,
        "local-project-tag-13579",
        &marker_path,
    );
    assert_hostexec_script_denied(&env.runtime, container_name);
}

/// Repo-less local projects (`local = true`).
///
/// Covers the container side of a project with no git remote: tickets can be
/// created against it, `-m manual -w <dir> -p <key>` launches a healthy worker on
/// the configured image with the project's hostexec scripts allowed, and no
/// `pool/<key>` directory is ever created. Launch shapes that need a pool are
/// refused before any RPC by the CLI (`reject_unsupported_local_launch` tests in
/// `crates/ur`) and server (`local_launch_rejection_reason` tests).
fn scenario_local_project(env: &TestEnv) {
    let key = LOCAL_PROJECT_KEY;
    // With both -p and -w, the manual process_id is `{project_key}-man-{workspace
    // basename}` (see generate_manual_process_id) — not `-man-0`. The project dir
    // basename is the key here, so both halves are the key.
    let process_id = local_manual_process_id(key);
    let container_name = env.container_name(&process_id);
    let env_pairs = env.env();
    let env_slice = env_pairs.to_vec();
    let pool_dir = env.config_path.join("workspace").join("pool").join(key);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let project_dir = setup_local_project_dir(env, key);

        // ---- Tickets can be created against a local project ----
        // `--output json` is required: parse_ticket_id_from_create reads data.id.
        let ticket_output = run_cmd(
            &env.ur,
            &[
                "--output",
                "json",
                "ticket",
                "create",
                "Local project ticket",
                "--project",
                key,
            ],
            &env_slice,
        );
        assert!(
            ticket_output.status.success(),
            "ticket create for local project failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&ticket_output.stdout),
            String::from_utf8_lossy(&ticket_output.stderr),
        );
        parse_ticket_id_from_create(&ticket_output.stdout);

        // ---- Supported: manual mode with a workspace mount ----
        launch_and_verify_local_worker(env, key, &project_dir, &container_name);

        // ---- A local project gets no pool directory, even after a real launch ----
        assert!(
            !pool_dir.exists(),
            "a local project must never get a pool dir; found {}",
            pool_dir.display()
        );

        stop_worker(env, &process_id);
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// Poll the worker's tmux `status-left` until it equals `expected` (up to 20s).
fn wait_for_status_left(runtime: &str, container: &str, expected: &str, trigger: &str) {
    for _ in 0..80 {
        if read_tmux_status_left(runtime, container) == expected {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!(
        "expected status-left '{expected}' within 20s after {trigger}; last value: '{}'",
        read_tmux_status_left(runtime, container),
    );
}

/// Run `ur ticket <subcommand> <args>` and assert it succeeds.
fn ticket_cmd(env: &TestEnv, args: &[&str]) {
    let mut full_args = vec!["--output", "json", "ticket"];
    full_args.extend_from_slice(args);
    let output = run_cmd(&env.ur, &full_args, &env.env());
    assert!(
        output.status.success(),
        "ur {} failed.\nstdout: {}\nstderr: {}",
        full_args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Wait up to 20s for a container to no longer exist.
fn wait_for_container_gone(runtime: &str, container: &str) {
    for _ in 0..80 {
        let inspect = Command::new(runtime)
            .args(["inspect", container])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("failed to inspect container");
        if !inspect.success() {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("container {container} still exists 20s after its workflow was cancelled");
}

/// Dispatch end to end against a real worker: `-d` creates a workflow (visible in
/// `ur flow show` / `ur flow list`) without closing the ticket; ticket metadata
/// changes refresh the worker's tmux label (`[<ticket> PR-42]`, then back to
/// `[<ticket>]`); and `ur flow cancel` marks the workflow cancelled and kills the
/// worker container.
///
/// Duplicate/closed-ticket dispatch refusal and the list/cancel bookkeeping are
/// unit-tested in `grpc_ticket.rs`.
fn scenario_dispatch_creates_workflow(env: &TestEnv) {
    let ticket_id = create_test_ticket(env, "Dispatch workflow test");
    let container_name = env.container_name(&ticket_id);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // The -d flag calls CreateWorkflow before launching; if workflow creation
        // fails, the launch command itself fails.
        launch_worker(env, &["-p", env.project_key, "-d", &ticket_id]);
        wait_for_healthy(&env.runtime, &container_name);

        // ---- Dispatch does not close the ticket; the workflow is visible ----
        assert_eq!(
            get_ticket_status(&env.ur, &env.env(), &ticket_id).as_deref(),
            Some("open"),
            "ticket should still be open after dispatch"
        );
        let flow_json =
            flow_show(env, &ticket_id).expect("ur flow show should succeed after dispatch");
        assert_eq!(
            flow_json["data"]["workflow"]["ticket_id"].as_str(),
            Some(ticket_id.as_str()),
            "flow show should reference the dispatched ticket.\nJSON: {flow_json}"
        );
        let list_json = flow_list(env);
        assert!(
            flow_list_contains(&list_json, &ticket_id),
            "flow list should include the dispatched workflow.\nJSON: {list_json}"
        );

        // ---- Metadata changes refresh the worker label (spawn_label_refresh_for_ticket) ----
        // Driven by set-meta/delete-meta rather than the agent reporting idle, which
        // avoids timing sensitivity around Claude Code startup.
        ticket_cmd(env, &["set-meta", &ticket_id, "pr_number", "42"]);
        wait_for_status_left(
            &env.runtime,
            &container_name,
            &format!("[{ticket_id} PR-42]"),
            "set-meta",
        );
        ticket_cmd(env, &["delete-meta", &ticket_id, "pr_number"]);
        wait_for_status_left(
            &env.runtime,
            &container_name,
            &format!("[{ticket_id}]"),
            "delete-meta",
        );

        // ---- Cancelling the workflow marks it cancelled and kills its worker ----
        flow_cancel(env, &ticket_id);
        let show =
            flow_show(env, &ticket_id).expect("ur flow show should return cancelled workflow");
        assert_eq!(
            show["data"]["workflow"]["status"].as_str(),
            Some("cancelled"),
            "cancelled workflow should have status 'cancelled'.\nJSON: {show}"
        );
        wait_for_container_gone(&env.runtime, &container_name);
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// Automatic Gitea forge detection:
/// - Project has no explicit `forge`, `skills`, `hostexec`, or `hostexec_deny` settings.
/// - Repository URL does not contain "github.com" (defaults to Gitea).
/// - Container has the tea shim, no gh shim, and the gitea skill installed.
/// - The tea transform runs server-side: `tea login` is rejected as blocked.
/// - Container's CLAUDE.md contains `# Forge Collaboration` referencing Gitea and tea.
fn scenario_project_gitea_auto_detection(env: &TestEnv) {
    let ticket_id = "gitea-auto-detect-test";
    let container_name = env.container_name(ticket_id);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        launch_worker(env, &["-p", "giteaautoproj", ticket_id]);
        wait_for_healthy(&env.runtime, &container_name);

        // ---- tea shim and gitea skill present, gh shim absent ----
        assert_container_paths_exist(
            &env.runtime,
            &container_name,
            &[
                "/home/worker/.local/bin/tea",
                "/home/worker/.claude/skills/gitea/SKILL.md",
            ],
        );
        let gh_shim = exec_in_container(
            &env.runtime,
            &container_name,
            &["ls", "/home/worker/.local/bin/gh"],
        );
        assert!(
            !gh_shim.status.success(),
            "gh shim should NOT exist in auto-detected Gitea project"
        );

        // ---- Blocked tea command is rejected by the Lua transform ----
        let blocked_tea = exec_in_container(&env.runtime, &container_name, &["tea", "login"]);
        let tea_stderr = String::from_utf8_lossy(&blocked_tea.stderr);
        assert!(
            !blocked_tea.status.success() && tea_stderr.contains("blocked"),
            "tea login should be rejected as blocked by the tea transform, got: {tea_stderr}"
        );

        // ---- CLAUDE.md contains the Forge Collaboration section ----
        let claude_md = exec_in_container(
            &env.runtime,
            &container_name,
            &["cat", "/home/worker/.claude/CLAUDE.md"],
        );
        assert_exec_success(&claude_md, "reading CLAUDE.md should succeed");
        let content = String::from_utf8_lossy(&claude_md.stdout);
        assert!(
            content.contains("# Forge Collaboration"),
            "CLAUDE.md should contain '# Forge Collaboration', got:\n{content}"
        );
        assert!(
            content.contains("Gitea") && content.contains("tea"),
            "CLAUDE.md should reference Gitea and tea, got:\n{content}"
        );

        stop_worker(env, ticket_id);
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// A synchronous startup-hook failure must fail the launch and include the hook error.
fn scenario_failing_startup_hook(env: &TestEnv) {
    use std::os::unix::fs::PermissionsExt;

    let ticket_id = "startup-hook-failure-test";
    let container_name = env.container_name(ticket_id);
    let hook_dir = env
        .config_path
        .join(format!("projects/{FAILING_HOOK_PROJECT_KEY}/hooks/startup"));
    std::fs::create_dir_all(&hook_dir).expect("failed to create startup hook overlay");
    let hook = hook_dir.join("05-fail");
    std::fs::write(
        &hook,
        "#!/bin/sh\necho acceptance-startup-hook-failure >&2\nexit 7\n",
    )
    .expect("failed to write failing startup hook");
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
        .expect("failed to make startup hook executable");

    let output = run_cmd(
        &env.ur,
        &[
            "worker",
            "launch",
            "-p",
            FAILING_HOOK_PROJECT_KEY,
            ticket_id,
        ],
        &env.env(),
    );
    std::fs::remove_file(&hook).expect("failed to remove failing startup hook");
    assert!(
        !output.status.success(),
        "launch with a failing startup hook unexpectedly succeeded"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("acceptance-startup-hook-failure"),
        "{combined}"
    );

    let inspect = Command::new(&env.runtime)
        .args(["inspect", &container_name])
        .output()
        .expect("failed to inspect failed startup container");
    assert!(
        !inspect.status.success(),
        "failed startup container was not removed"
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

/// Manual pool worker: `-m manual -p <project>` (no ticket) gets an auto-generated
/// `{project}-man-{slot}` process ID, stays off any worker-specific branch, may
/// `git checkout`, shows as mode `manual` in `ur worker list`, picks up the
/// `[worker_models.claude] manual = { model = "sonnet" }` override from the shared
/// config (the built-in manual default is "opus"), and stops cleanly.
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
        assert_worker_model(&env.runtime, &container_name, "sonnet");
        manual_verify_git_checkout_allowed(&env.runtime, &container_name);
        manual_stop_and_verify_gone(&env.ur, &env_slice, &process_id);
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// Host-side mounts anchor: one `mountproj` worker (see `setup_mount_projects`)
/// proves every per-project bind mount and hook overlay together:
///
/// - the host-only `mounts` source is visible at `/mnt/test` (prim-usai3
///   regression: only builderd, running natively on the host, can see it; the
///   pre-builderd server-side path made Docker silently create an empty dir);
/// - `memory_dir` round-trips host ↔ container at the agent's memory path;
/// - `brain_dir` round-trips at `/brain`, and the project's brain wins over the
///   top-level `workspace_brain_dir` (whose `ws-seed.md` must not appear);
/// - the host git-hook overlay shadows the in-repo hook, adds overlay-only hooks
///   (both 0755), and overlay skill hooks are installed;
/// - the in-repo background startup hook ran.
fn scenario_mounts(env: &TestEnv) {
    let ticket_id = "mount-test";
    let container_name = env.container_name(ticket_id);
    let memory_container_path = "/home/worker/.claude/projects/-workspace/memory";

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        launch_worker(env, &["-p", "mountproj", ticket_id]);
        wait_for_healthy(&env.runtime, &container_name);

        // ---- Host-only bind mount ----
        assert_container_file(
            &env.runtime,
            &container_name,
            "/mnt/test/hello.txt",
            "mount-test-content",
        );

        // ---- memory_dir: host → container, then container → host ----
        assert_container_file(
            &env.runtime,
            &container_name,
            &format!("{memory_container_path}/seed.md"),
            "hello-from-host",
        );
        write_in_container(
            &env.runtime,
            &container_name,
            &format!("{memory_container_path}/from-container.md"),
            "hello-from-container",
        );

        // ---- brain_dir: project brain beats workspace_brain_dir, read-write ----
        assert_container_file(
            &env.runtime,
            &container_name,
            "/brain/seed.md",
            "hello-from-host-brain",
        );
        let ws_seed = exec_in_container(
            &env.runtime,
            &container_name,
            &["test", "-e", "/brain/ws-seed.md"],
        );
        assert!(
            !ws_seed.status.success(),
            "a project brain_dir must win over workspace_brain_dir"
        );
        write_in_container(
            &env.runtime,
            &container_name,
            "/brain/from-container.md",
            "hello-from-container-brain",
        );

        // ---- Hook overlay precedence and in-repo startup hook ----
        verify_hook_overlay_assertions(&env.runtime, &container_name);
        wait_for_container_file(
            &env.runtime,
            &container_name,
            "/workspace/.startup-background-hook-ran",
        );

        stop_worker(env, ticket_id);

        assert_host_file(
            &env.mount_dirs.memory.join("from-container.md"),
            "hello-from-container",
        );
        assert_host_file(
            &env.mount_dirs.brain.join("from-container.md"),
            "hello-from-container-brain",
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

fn launch_agy_worker(env: &TestEnv, project: &str, ticket_id: &str, dispatch: bool) {
    let env_pairs = env.env();
    let mut args = vec!["worker", "launch", "-p", project, "--agent", "agy"];
    if dispatch {
        args.push("-d");
    }
    args.push(ticket_id);
    let output = run_cmd(&env.ur, &args, &env_pairs);
    assert!(
        output.status.success(),
        "AGY worker launch failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    wait_for_healthy(&env.runtime, &env.container_name(ticket_id));
}

fn stop_agy_worker(env: &TestEnv, ticket_id: &str) {
    let output = run_cmd(&env.ur, &["worker", "stop", ticket_id], &env.env());
    assert!(
        output.status.success(),
        "stopping AGY worker failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Codex-agent anchor: a dispatched (`-d`) `--agent codex` worker on `codexproj`.
///
/// - `agent_type=codex` in `ur worker list`, and the bare `ur-worker` alias resolves
///   to `ur-worker-codex:<UR_IMAGE_TAG>`;
/// - the in-container `~/.codex/` layout mirrors `~/.claude/`'s (`config.toml`,
///   `AGENTS.md`, `skills/<name>/SKILL.md`, `skill-hooks/`), with `auth.json`
///   mounted as a single file (see `RunOptsBuilder::add_credentials`), plus the
///   base `.agent-shared/` layout;
/// - dispatch is accepted without closing the ticket.
///
/// The dummy credential cannot get codex to an idle prompt (see
/// `crates/acceptance/CLAUDE.md`), so the agent-phrased dispatch text is covered by
/// the `implement_commands_*` unit tests in `crates/workerd/src/grpc_service.rs`.
fn scenario_codex_manual_worker(env: &TestEnv) {
    seed_dummy_codex_credentials(&env.config_path);
    let create_output = run_cmd(
        &env.ur,
        &[
            "--output",
            "json",
            "ticket",
            "create",
            "Codex dispatch test",
            "-p",
            "codexproj",
        ],
        &env.env(),
    );
    assert!(
        create_output.status.success(),
        "ur ticket create -p codexproj failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&create_output.stdout),
        String::from_utf8_lossy(&create_output.stderr),
    );
    let ticket_id = parse_ticket_id_from_create(&create_output.stdout);
    let container_name = env.container_name(&ticket_id);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        launch_worker(
            env,
            &["-p", "codexproj", "--agent", "codex", "-d", &ticket_id],
        );
        wait_for_healthy(&env.runtime, &container_name);

        assert_agent_type(env, &ticket_id, "codex");
        assert_container_image(&env.runtime, &container_name, "codex");

        let mut paths = vec![
            "/home/worker/.codex/config.toml",
            "/home/worker/.codex/AGENTS.md",
            "/home/worker/.codex/skills/test-skill/SKILL.md",
            "/home/worker/.codex/skill-hooks",
        ];
        paths.extend_from_slice(AGENT_SHARED_PATHS);
        assert_container_paths_exist(&env.runtime, &container_name, &paths);
        let is_file_output = exec_in_container(
            &env.runtime,
            &container_name,
            &["test", "-f", "/home/worker/.codex/auth.json"],
        );
        assert_exec_success(
            &is_file_output,
            "/home/worker/.codex/auth.json should be a regular file, not a directory",
        );

        assert_eq!(
            get_ticket_status(&env.ur, &env.env(), &ticket_id).as_deref(),
            Some("open"),
            "ticket should still be open after dispatch"
        );

        stop_worker(env, &ticket_id);
    }));

    if let Err(e) = result {
        force_remove_container(&env.runtime, &container_name);
        std::panic::resume_unwind(e);
    }
}

/// AGY-agent anchor: an `--agent agy` worker on `agyproj`, launched with no
/// pre-seeded token.
///
/// - model, `UR_AGENT_TYPE`, `agent_type=agy`, and the bare alias resolving to
///   `ur-worker-agy:<UR_IMAGE_TAG>`;
/// - the split customization (`~/.gemini/config`) / runtime
///   (`~/.gemini/antigravity-cli`) layout;
/// - credentials: launch creates the host token file and mounts exactly that one
///   file — regular, writable by the worker, never the wider `.gemini` tree;
/// - the baked global Stop hook points at `workertools notify-idle --json`, which
///   reaches NotifyIdle and prints AGY's required `{}`.
///
/// Not dispatched: on a dispatched worker, notify-idle can push `/clear` +
/// `/implement`. Dispatch phrasing is unit-tested in `crates/workerd/src/grpc_service.rs`.
fn scenario_agy_manual_worker(env: &TestEnv) {
    let ticket_id = "agy-manual-worker-test";
    let container = env.container_name(ticket_id);
    let host_token = env.config_path.join("agy/antigravity-oauth-token");
    if host_token.exists() {
        std::fs::remove_file(&host_token).expect("failed to reset AGY token fixture");
    }
    let token_path = "/home/worker/.gemini/antigravity-cli/antigravity-oauth-token";

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        launch_agy_worker(env, "agyproj", ticket_id, false);

        // ---- Agent identity, model, image ----
        assert_eq!(
            container_env_var(&env.runtime, &container, "UR_AGENT_TYPE"),
            "agy"
        );
        assert_worker_model(&env.runtime, &container, "gemini-3.8-flash");
        assert_agent_type(env, ticket_id, "agy");
        assert_container_image(&env.runtime, &container, "agy");

        // ---- Split customization/runtime layout ----
        assert_container_paths_exist(
            &env.runtime,
            &container,
            &[
                "/home/worker/.gemini/config/AGENTS.md",
                "/home/worker/.gemini/config/skills/test-skill/SKILL.md",
                "/home/worker/.gemini/config/skill-hooks/my-hook.sh",
                "/home/worker/.gemini/antigravity-cli/settings.json",
            ],
        );

        // ---- An absent token is allowed: launch creates and mounts just that file ----
        assert!(
            host_token.is_file(),
            "launch should create the AGY token file"
        );
        let output = exec_in_container(&env.runtime, &container, &["test", "-f", token_path]);
        assert_exec_success(&output, "AGY token mount must be a regular file");
        let output = exec_in_container(&env.runtime, &container, &["test", "-w", token_path]);
        assert_exec_success(
            &output,
            "AGY token mount must be writable by the worker user",
        );
        let mounts = Command::new(&env.runtime)
            .args(["inspect", "--format", "{{json .Mounts}}", &container])
            .output()
            .expect("failed to inspect AGY mounts");
        let text = String::from_utf8_lossy(&mounts.stdout);
        assert_eq!(text.matches("antigravity-oauth-token").count(), 2, "{text}");
        assert!(!text.contains("/home/worker/.gemini\""), "{text}");

        // ---- Global Stop hook reaches NotifyIdle with AGY's JSON stdout contract ----
        let hooks = exec_in_container(
            &env.runtime,
            &container,
            &["cat", "/home/worker/.gemini/config/hooks.json"],
        );
        assert_exec_success(&hooks, "AGY global hooks file should exist");
        assert!(String::from_utf8_lossy(&hooks.stdout).contains("workertools notify-idle --json"));
        let notify = exec_in_container(
            &env.runtime,
            &container,
            &["workertools", "notify-idle", "--json"],
        );
        assert_exec_success(&notify, "AGY Stop hook command should reach NotifyIdle");
        assert_eq!(String::from_utf8_lossy(&notify.stdout), "{}\n");

        stop_agy_worker(env, ticket_id);
    }));
    if let Err(error) = result {
        force_remove_container(&env.runtime, &container);
        std::panic::resume_unwind(error);
    }
}
