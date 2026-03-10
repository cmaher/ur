use anyhow::{Context, Result};
use tracing::{debug, info, instrument, warn};

/// Resolve the ur-hostd binary path. Looks next to the current executable first
/// (handles target/debug/ during development), then falls back to PATH lookup.
fn hostd_bin() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name("ur-hostd");
        if sibling.exists() {
            return sibling;
        }
    }
    std::path::PathBuf::from("ur-hostd")
}

#[instrument(skip(config), fields(hostd_port = config.hostd_port))]
pub fn start_hostd(config: &ur_config::Config) -> Result<()> {
    let pid_file = config.config_dir.join(ur_config::HOSTD_PID_FILE);

    // Check for stale PID
    if pid_file.exists() {
        let pid_str = std::fs::read_to_string(&pid_file)?;
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if alive {
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
        .spawn()
        .context("failed to spawn ur-hostd — is it installed and on PATH?")?;

    let pid = child.id();
    std::fs::write(&pid_file, pid.to_string())?;
    info!(pid, "ur-hostd started");
    println!("ur-hostd started (pid {pid})");

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
