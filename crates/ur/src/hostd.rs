use anyhow::{Context, Result};

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
                println!("ur-hostd already running (pid {pid})");
                return Ok(());
            }
            std::fs::remove_file(&pid_file)?;
        }
    }

    let child = std::process::Command::new(hostd_bin())
        .args(["--port", &config.hostd_port.to_string()])
        .stdout(std::fs::File::create(config.config_dir.join("hostd.log"))?)
        .stderr(std::fs::File::create(config.config_dir.join("hostd.err"))?)
        .spawn()
        .context("failed to spawn ur-hostd — is it installed and on PATH?")?;

    std::fs::write(&pid_file, child.id().to_string())?;
    println!("ur-hostd started (pid {})", child.id());

    Ok(())
}

pub fn stop_hostd(config: &ur_config::Config) -> Result<()> {
    let pid_file = config.config_dir.join(ur_config::HOSTD_PID_FILE);

    if !pid_file.exists() {
        return Ok(());
    }

    let pid_str = std::fs::read_to_string(&pid_file)?;
    if let Ok(pid) = pid_str.trim().parse::<u32>() {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .output();
    }

    std::fs::remove_file(&pid_file)?;
    println!("ur-hostd stopped");

    Ok(())
}
