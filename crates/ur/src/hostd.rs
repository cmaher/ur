use std::os::unix::process::CommandExt;

use anyhow::{Context, Result};
use fs4::fs_std::FileExt;
use tracing::{debug, info, instrument, warn};

/// Resolve the ur-hostd binary path. Looks next to the current executable first
/// (handles target/debug/ during development), then falls back to PATH lookup.
#[instrument]
fn hostd_bin() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name("ur-hostd");
        if sibling.exists() {
            debug!(path = %sibling.display(), "found ur-hostd next to current executable");
            return sibling;
        }
    }
    debug!("falling back to PATH lookup for ur-hostd");
    std::path::PathBuf::from("ur-hostd")
}

/// Check whether a process with the given PID is alive.
fn is_pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[instrument(skip(config), fields(hostd_port = config.hostd_port))]
pub fn start_hostd(config: &ur_config::Config) -> Result<()> {
    let pid_file = config.config_dir.join(ur_config::HOSTD_PID_FILE);

    // Take an exclusive lock on a lock file to prevent races between concurrent
    // `ur start` invocations. The lock is held only while we check + spawn.
    let lock_path = config.config_dir.join("hostd.lock");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&lock_path)
        .context("failed to open hostd lock file")?;
    lock_file
        .lock_exclusive()
        .context("failed to acquire hostd lock")?;

    // Check for existing process (under lock so no race)
    if pid_file.exists() {
        let pid_str = std::fs::read_to_string(&pid_file)?;
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            if is_pid_alive(pid) {
                info!(pid, "ur-hostd already running");
                println!("ur-hostd already running (pid {pid})");
                return Ok(());
            }
            debug!(pid, "removing stale PID file");
            std::fs::remove_file(&pid_file)?;
        }
    }

    let bin = hostd_bin();
    debug!(bin = %bin.display(), "spawning ur-hostd");
    let child = std::process::Command::new(&bin)
        .args(["--port", &config.hostd_port.to_string()])
        .stdout(std::fs::File::create(config.config_dir.join("hostd.log"))?)
        .stderr(std::fs::File::create(config.config_dir.join("hostd.err"))?)
        // Put hostd in its own process group so signals sent to the ur CLI
        // (e.g. Ctrl-C) don't propagate to the daemon.
        .process_group(0)
        .spawn()
        .context("failed to spawn ur-hostd — is it installed and on PATH?")?;

    let pid = child.id();
    std::fs::write(&pid_file, pid.to_string())?;
    info!(pid, "ur-hostd started");
    println!("ur-hostd started (pid {pid})");

    // Lock is released when lock_file is dropped
    Ok(())
}

#[instrument(skip(config))]
pub fn stop_hostd(config: &ur_config::Config) -> Result<()> {
    let pid_file = config.config_dir.join(ur_config::HOSTD_PID_FILE);

    if !pid_file.exists() {
        debug!("no hostd PID file found, nothing to stop");
        return Ok(());
    }

    let pid_str = std::fs::read_to_string(&pid_file)?;
    if let Ok(pid) = pid_str.trim().parse::<u32>() {
        debug!(pid, "sending SIGTERM to ur-hostd");
        let result = std::process::Command::new("kill")
            .arg(pid.to_string())
            .output();
        if let Err(e) = result {
            warn!(pid, error = %e, "failed to send SIGTERM to ur-hostd");
        }
    }

    std::fs::remove_file(&pid_file)?;
    info!("ur-hostd stopped");
    println!("ur-hostd stopped");

    Ok(())
}
