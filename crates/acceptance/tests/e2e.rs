//! End-to-end acceptance tests for the Ur gRPC + workercmd architecture.
//!
//! These tests exercise the full user-facing workflow:
//!   1. `urd` starts and listens on a gRPC socket (`ur-grpc.sock`)
//!   2. `ur process launch` builds the container image and launches a container
//!      with a per-agent gRPC socket mounted at `/var/run/ur/ur.sock`
//!   3. Worker commands inside the container (`ur-ping`, `git`) connect to urd
//!      via tonic gRPC over the per-agent UDS
//!   4. `ur process stop` tears down the container and cleans up
//!
//! Gated behind `--features acceptance` so they never run in normal `cargo test`.
//! Requires:
//!   - Pre-built `urd` and `ur` binaries in `target/debug/`
//!   - Worker commands (`ur-ping`, `git`) baked into the container image
//!   - A container runtime (Apple `container`, Docker, or nerdctl)
#![cfg(feature = "acceptance")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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

/// Start `urd` as a background process with `UR_CONFIG` set to the given dir.
/// Waits for the socket file to appear before returning.
fn start_urd(config_dir: &Path) -> Child {
    let urd = bin("urd");
    assert!(urd.exists(), "urd binary not found at {}", urd.display());

    let child = Command::new(&urd)
        .env("UR_CONFIG", config_dir)
        .env("RUST_LOG", "info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn urd");

    // Wait for the gRPC socket to appear (urd creates it on startup).
    let socket = config_dir.join("ur-grpc.sock");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(
            Instant::now() < deadline,
            "urd did not create socket at {} within 10s",
            socket.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    child
}

/// Run a CLI command from the workspace root, returning its output. Panics on spawn failure.
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

/// Kill a child process and wait for it to exit.
fn kill_and_wait(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Detect the container runtime available on this system.
/// Returns the command name for the runtime (e.g., "container", "docker", or "nerdctl").
fn detect_container_runtime() -> String {
    if let Ok(val) = std::env::var("UR_CONTAINER") {
        // Normalize "containerd" to the actual CLI command.
        return if val == "containerd" {
            "nerdctl".into()
        } else {
            val
        };
    }
    // Check for Apple `container` CLI first, then docker, then nerdctl.
    for cmd in ["container", "docker", "nerdctl"] {
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

#[test]
fn e2e_ping_and_git() {
    // ---- (0) Clean up stale containers from prior failed runs ----
    let runtime = detect_container_runtime();
    let ticket_id = "acceptance-test";
    let container_name = format!("ur-agent-{ticket_id}");
    force_remove_container(&runtime, &container_name);

    // ---- (1) Create temp UR_CONFIG dir ----
    let config_dir = tempfile::tempdir().expect("failed to create temp config dir");
    let config_path = config_dir.path();
    let grpc_socket_path = config_path.join("ur-grpc.sock");

    // ---- (2) Start urd ----
    let urd_child = start_urd(config_path);

    // Use catch_unwind so we always clean up urd even on panic.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let ur = bin("ur");
        assert!(ur.exists(), "ur binary not found at {}", ur.display());

        let socket_str = grpc_socket_path.to_str().unwrap();

        // ---- (3) ur process launch ----
        let launch_output = run_cmd(
            &ur,
            &["--socket", socket_str, "process", "launch", ticket_id],
            &[("UR_CONFIG", config_path.to_str().unwrap())],
        );
        assert!(
            launch_output.status.success(),
            "ur process launch failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&launch_output.stdout),
            String::from_utf8_lossy(&launch_output.stderr),
        );

        let launch_stdout = String::from_utf8_lossy(&launch_output.stdout);
        // Expected output: "Agent ur-agent-<ticket> running (container <id>)"
        assert!(
            launch_stdout.contains(&container_name),
            "launch output should contain container name '{container_name}'.\nGot: {launch_stdout}"
        );

        // ---- (4) exec ur-ping inside container ----
        // ur-ping is at /usr/local/bin/ur-ping inside the container.
        // The socket dir is mounted at /var/run/ur/ (UR_SOCKET set in Dockerfile).
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

        // ---- (5) Test git commands via git proxy ----
        // urd has already created and git-init'd the repo for this process.
        // git status should succeed via the per-agent gRPC socket.
        let git_output = Command::new(&runtime)
            .args(["exec", &container_name, "git", "status"])
            .output()
            .expect("failed to exec git status in container");

        assert_eq!(
            git_output.status.code(),
            Some(0),
            "git status should exit 0.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&git_output.stdout),
            String::from_utf8_lossy(&git_output.stderr),
        );

        let git_stdout = String::from_utf8_lossy(&git_output.stdout);
        assert!(
            git_stdout.contains("branch") || git_stdout.contains("No commits"),
            "git status should show repo info.\nGot: {git_stdout}"
        );

        // ---- (5b) Test blocked flags ----
        // `-C` is a blocked flag that could let the agent escape its repo.
        // The git proxy should relay the gRPC InvalidArgument error and exit non-zero.
        let blocked_output = Command::new(&runtime)
            .args(["exec", &container_name, "git", "-C", "/tmp", "status"])
            .output()
            .expect("failed to exec git -C in container");
        assert_ne!(
            blocked_output.status.code(),
            Some(0),
            "git -C should be blocked.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&blocked_output.stdout),
            String::from_utf8_lossy(&blocked_output.stderr),
        );

        // ---- (6) ur process stop (by ticket_id, not container name) ----
        let stop_output = run_cmd(
            &ur,
            &["--socket", socket_str, "process", "stop", ticket_id],
            &[],
        );
        assert!(
            stop_output.status.success(),
            "ur process stop failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&stop_output.stdout),
            String::from_utf8_lossy(&stop_output.stderr),
        );
    }));

    // ---- (7) Kill urd (always, even if test panicked) ----
    kill_and_wait(urd_child);

    // Re-raise any panic from the test body.
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
