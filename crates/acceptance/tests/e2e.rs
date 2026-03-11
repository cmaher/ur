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

/// Write a test-specific ur.toml and supporting files.
///
/// `ur start` renders the compose file from its embedded template, replacing
/// network name and container name placeholders with values from the config.
/// Uses unique container names (`ur-server-test`, `ur-squid-test`) so the
/// acceptance test stack never collides with a real running ur stack.
fn write_test_config(config_dir: &Path, daemon_port: u16) {
    let workspace_dir = config_dir.join("workspace");
    std::fs::create_dir_all(&workspace_dir).expect("failed to create workspace dir");

    let squid_dir = config_dir.join("squid");
    std::fs::create_dir_all(&squid_dir).expect("failed to create squid dir");
    std::fs::write(
        squid_dir.join("allowlist.txt"),
        "api.anthropic.com\nplatform.claude.com\nraw.githubusercontent.com\n",
    )
    .expect("failed to write allowlist.txt");

    let compose_file = config_dir.join("docker-compose.yml");
    let toml_content = format!(
        "daemon_port = {daemon_port}\n\
         workspace = \"{workspace}\"\n\
         compose_file = \"{compose}\"\n\
         \n\
         [proxy]\n\
         hostname = \"ur-test-squid\"\n\
         \n\
         [network]\n\
         name = \"ur-test\"\n\
         worker_name = \"ur-test-workers\"\n\
         server_hostname = \"ur-test-server\"\n\
         agent_prefix = \"ur-test-agent-\"\n",
        workspace = workspace_dir.display(),
        compose = compose_file.display(),
    );
    std::fs::write(config_dir.join("ur.toml"), toml_content).expect("failed to write ur.toml");
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
    let ticket_id = "acceptance-test";
    let agent_prefix = "ur-test-agent-";
    let container_name = format!("{agent_prefix}{ticket_id}");
    // Container names match the test config (NOT the defaults, to avoid
    // colliding with a real running ur stack)
    let server_container = "ur-test-server";
    let squid_container = "ur-test-squid";

    // ---- (0) Clean up stale containers from prior failed runs ----
    force_remove_container(&runtime, &container_name);
    force_remove_container(&runtime, server_container);
    force_remove_container(&runtime, squid_container);

    // ---- (1) Create temp UR_CONFIG dir with test-specific config ----
    let config_dir = tempfile::tempdir().expect("failed to create temp config dir");
    let config_path = config_dir.path();
    let daemon_port = 19876u16;

    write_test_config(config_path, daemon_port);

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
        let launch_output = run_cmd(&ur, &["process", "launch", ticket_id], &env);
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

        // ---- (5) Test hostexec: git commands via worker → server → hostd → host ----
        // The git shim calls ur-tools host-exec git, which goes through the full
        // hostexec pipeline. This verifies ur-hostd is running and reachable.
        let git_output = Command::new(&runtime)
            .args(["exec", &container_name, "git", "status"])
            .output()
            .expect("failed to exec git status in container");

        assert_eq!(
            git_output.status.code(),
            Some(0),
            "git status should exit 0 (hostexec pipeline: worker → server → hostd → host).\n\
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
        let stop_output = run_cmd(&ur, &["process", "stop", ticket_id], &env);
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
