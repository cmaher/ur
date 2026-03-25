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
    let child = std::process::Command::new(&bin)
        .args([
            "--port",
            &config.builderd_port.to_string(),
            "--workspace",
            &config.workspace.display().to_string(),
            "--logs-dir",
            &config.logs_dir.display().to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
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
