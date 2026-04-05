use std::os::unix::process::CommandExt;
use std::process::Stdio;

use anyhow::{Context, Result};
use fs4::fs_std::FileExt;
use tracing::{debug, info, instrument, warn};

use crate::output::OutputManager;

/// Resolve the builderd binary path. Looks next to the current executable first
/// (handles target/debug/ during development), then falls back to PATH lookup.
#[instrument]
fn builderd_bin() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name("builderd");
        if sibling.exists() {
            debug!(path = %sibling.display(), "found builderd next to current executable");
            return sibling;
        }
    }
    debug!("falling back to PATH lookup for builderd");
    std::path::PathBuf::from("builderd")
}

/// Check whether a process with the given PID is alive.
fn is_pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[instrument(skip(config, output), fields(builderd_port = config.builderd_port))]
pub fn start_builderd(config: &ur_config::Config, output: &OutputManager) -> Result<()> {
    let pid_file = config.config_dir.join(ur_config::BUILDERD_PID_FILE);

    // Take an exclusive lock on a lock file to prevent races between concurrent
    // `ur start` invocations. The lock is held only while we check + spawn.
    let lock_path = config.config_dir.join("builderd.lock");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&lock_path)
        .context("failed to open builderd lock file")?;
    lock_file
        .lock_exclusive()
        .context("failed to acquire builderd lock")?;

    // Check for existing process (under lock so no race)
    if pid_file.exists() {
        let pid_str = std::fs::read_to_string(&pid_file)?;
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            if is_pid_alive(pid) {
                info!(pid, "builderd already running");
                output.print_text(&format!("builderd already running (pid {pid})"));
                return Ok(());
            }
            debug!(pid, "removing stale PID file");
            std::fs::remove_file(&pid_file)?;
        }
    }

    let bin = builderd_bin();
    debug!(bin = %bin.display(), "spawning builderd");

    // Redirect stderr to a file so panics are captured but the daemon doesn't
    // hold the parent's stderr pipe open (which would block callers using
    // `Command::output()` to capture ur's output).
    std::fs::create_dir_all(&config.logs_dir).context("failed to create logs directory")?;
    let stderr_path = config.logs_dir.join("builderd.err");
    let stderr_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_path)
        .context("failed to open builderd stderr log")?;

    let args = builderd_args(config);

    let child = std::process::Command::new(&bin)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        // Put builderd in its own process group so signals sent to the ur CLI
        // (e.g. Ctrl-C) don't propagate to the daemon.
        .process_group(0)
        .spawn()
        .context("failed to spawn builderd — is it installed and on PATH?")?;

    let pid = child.id();
    std::fs::write(&pid_file, pid.to_string())?;
    info!(pid, "builderd started");
    output.print_text(&format!("builderd started (pid {pid})"));

    // Lock is released when lock_file is dropped
    Ok(())
}

#[cfg(target_os = "macos")]
fn builderd_args(config: &ur_config::Config) -> Vec<String> {
    vec![
        "--port".to_string(),
        config.builderd_port.to_string(),
        "--workspace".to_string(),
        config.workspace.display().to_string(),
        "--logs-dir".to_string(),
        config.logs_dir.display().to_string(),
    ]
}

#[cfg(not(target_os = "macos"))]
fn builderd_args(config: &ur_config::Config) -> Vec<String> {
    let mut args = vec![
        "--port".to_string(),
        config.builderd_port.to_string(),
        "--workspace".to_string(),
        config.workspace.display().to_string(),
        "--logs-dir".to_string(),
        config.logs_dir.display().to_string(),
    ];

    // Bind builderd to the Docker bridge gateway IP so containers can reach it
    // via host.docker.internal. On macOS, Docker Desktop handles this natively.
    if let Some(gateway_ip) = detect_docker_bridge_ip() {
        info!(gateway_ip, "binding builderd to Docker bridge gateway");
        args.extend(["--bind".to_string(), gateway_ip]);
    } else {
        warn!(
            "could not detect Docker bridge IP; builderd will bind to 127.0.0.1 — containers may not be able to reach it"
        );
    }

    args
}

/// Detect the IP address of the Docker bridge network (`docker0`) by parsing
/// the output of `ip -4 addr show docker0`. Returns `None` if the interface
/// doesn't exist or the IP can't be parsed.
#[cfg(not(target_os = "macos"))]
fn detect_docker_bridge_ip() -> Option<String> {
    let output = std::process::Command::new("ip")
        .args(["-4", "-o", "addr", "show", "docker0"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // Format: "N: docker0    inet 172.17.0.1/16 ..."
    let stdout = String::from_utf8_lossy(&output.stdout);
    let ip = stdout
        .split_whitespace()
        .skip_while(|s| *s != "inet")
        .nth(1)?
        .split('/')
        .next()?
        .to_string();
    debug!(ip, "detected Docker bridge IP");
    Some(ip)
}

#[instrument(skip(config, output))]
pub fn stop_builderd(config: &ur_config::Config, output: &OutputManager) -> Result<()> {
    let pid_file = config.config_dir.join(ur_config::BUILDERD_PID_FILE);

    if !pid_file.exists() {
        debug!("no builderd PID file found, nothing to stop");
        return Ok(());
    }

    let pid_str = std::fs::read_to_string(&pid_file)?;
    if let Ok(pid) = pid_str.trim().parse::<u32>() {
        debug!(pid, "sending SIGTERM to builderd");
        let result = std::process::Command::new("kill")
            .arg(pid.to_string())
            .output();
        if let Err(e) = result {
            warn!(pid, error = %e, "failed to send SIGTERM to builderd");
        }
    }

    std::fs::remove_file(&pid_file)?;
    info!("builderd stopped");
    output.print_text("builderd stopped");

    Ok(())
}
