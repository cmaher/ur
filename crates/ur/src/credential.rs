use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use container::{ContainerId, ContainerRuntime, ExecOpts};
use tracing::{debug, info, instrument, warn};

/// macOS Keychain service name where Claude Code stores OAuth credentials.
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

fn worker_home() -> &'static Path {
    Path::new(ur_config::WORKER_HOME)
}

/// Path inside the container where Claude Code stores credentials.
fn container_credentials_path() -> PathBuf {
    worker_home()
        .join(".claude")
        .join(ur_config::CLAUDE_CREDENTIALS_FILENAME)
}

/// Path inside the container where Claude Code stores app config.
fn container_config_path() -> PathBuf {
    worker_home().join(ur_config::CLAUDE_CONFIG_FILENAME)
}

/// Manages Claude Code credentials for container workers.
///
/// Credentials (`.credentials.json`) are stored at `$UR_CONFIG/claude/` on the
/// host and bind-mounted into all worker containers. The app config
/// (`.claude.json`) is baked into the container image.
#[derive(Clone)]
pub struct CredentialManager;

impl CredentialManager {
    /// Ensure credentials exist on disk for container mounting.
    ///
    /// Seeds OAuth credentials from the host Claude Code installation if no
    /// credentials file exists yet. On macOS, reads from the Keychain; on Linux,
    /// copies from `~/.claude/.credentials.json`. After seeding, containers own
    /// their session independently -- token refreshes in containers write back
    /// to the shared mount without touching the host credentials.
    #[instrument(skip(self))]
    pub fn ensure_credentials(&self) -> Result<()> {
        let creds_path = Self::host_credentials_path()?;

        // Seed credentials if missing or empty — containers manage their own
        // token lifecycle after the initial copy. An empty/stub file can be left
        // behind by the server's Docker bind-mount setup, so treat it as missing.
        let needs_seed = !creds_path.exists()
            || std::fs::metadata(&creds_path)
                .map(|m| m.len() < 10)
                .unwrap_or(true);
        if needs_seed {
            if let Ok(creds_json) = read_host_credentials() {
                info!(path = %creds_path.display(), "seeding credentials from host Claude Code");
                write_file(&creds_path, &creds_json)?;
            } else {
                debug!(path = %creds_path.display(), "no host credentials found to seed");
            }
        } else {
            debug!(path = %creds_path.display(), "credentials already exist");
        }

        Ok(())
    }

    /// Save credentials and config extracted from a running container to the host config dir.
    ///
    /// Reads both `.credentials.json` and `.claude.json` from the container and
    /// writes them to `$UR_CONFIG/claude/`.
    #[instrument(skip(self, runtime), fields(container = %container_id.0))]
    pub fn save_from_container(
        &self,
        runtime: &impl ContainerRuntime,
        container_id: &ContainerId,
    ) -> Result<Vec<PathBuf>> {
        let mut saved = Vec::new();

        let creds_container_path = container_credentials_path();
        let creds_path = self.save_file_from_container(
            runtime,
            container_id,
            &creds_container_path.to_string_lossy(),
            &Self::host_credentials_path()?,
        )?;
        saved.push(creds_path);

        let config_container_path = container_config_path();
        let config_path = self.save_file_from_container(
            runtime,
            container_id,
            &config_container_path.to_string_lossy(),
            &Self::host_config_path()?,
        )?;
        saved.push(config_path);

        info!(count = saved.len(), "credentials saved from container");
        Ok(saved)
    }

    /// Read a file from the container and write it to the host path.
    #[instrument(skip(self, runtime), fields(container = %container_id.0))]
    fn save_file_from_container(
        &self,
        runtime: &impl ContainerRuntime,
        container_id: &ContainerId,
        container_path: &str,
        host_path: &Path,
    ) -> Result<PathBuf> {
        debug!(container_path, host_path = %host_path.display(), "reading file from container");
        let opts = ExecOpts {
            command: vec!["cat".into(), container_path.into()],
            workdir: None,
        };
        let output = runtime
            .exec(container_id, &opts)
            .with_context(|| format!("failed to read {container_path} from container"))?;
        if output.exit_code != 0 {
            anyhow::bail!(
                "container has no file at {container_path} — \
                 login to Claude Code in the container first"
            );
        }
        let contents = output.stdout.trim();
        if contents.is_empty() {
            anyhow::bail!("{container_path} in container is empty");
        }
        write_file(host_path, contents)?;
        info!(host_path = %host_path.display(), "saved file from container");
        Ok(host_path.to_path_buf())
    }

    /// Resolve the host-side credentials file path.
    pub fn host_credentials_path() -> Result<PathBuf> {
        let config_dir = ur_config::resolve_config_dir()?;
        Ok(config_dir
            .join(ur_config::CLAUDE_DIR)
            .join(ur_config::CLAUDE_CREDENTIALS_FILENAME))
    }

    /// Resolve the host-side Claude config file path.
    pub fn host_config_path() -> Result<PathBuf> {
        let config_dir = ur_config::resolve_config_dir()?;
        Ok(config_dir
            .join(ur_config::CLAUDE_DIR)
            .join(ur_config::CLAUDE_CONFIG_FILENAME))
    }
}

/// Read Claude Code OAuth credentials from the host system.
///
/// On macOS, reads from the Keychain. On Linux, reads directly from the
/// Claude Code credentials file at `~/.claude/.credentials.json`.
#[instrument]
fn read_host_credentials() -> Result<String> {
    read_platform_credentials()
}

#[cfg(target_os = "macos")]
fn read_platform_credentials() -> Result<String> {
    use std::process::Command;
    debug!("reading credentials from macOS Keychain");
    let output = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
        .context("failed to run `security` command")?;
    if !output.status.success() {
        warn!("no credentials found in macOS Keychain");
        anyhow::bail!(
            "no credentials in macOS Keychain for service {KEYCHAIN_SERVICE:?} — \
             log in to Claude Code on this machine first"
        );
    }
    let json =
        String::from_utf8(output.stdout).context("keychain credentials are not valid UTF-8")?;
    let trimmed = json.trim().to_string();
    if trimmed.is_empty() {
        warn!("keychain credentials are empty");
        anyhow::bail!("keychain credentials are empty");
    }
    info!("credentials read from macOS Keychain");
    Ok(trimmed)
}

#[cfg(not(target_os = "macos"))]
fn read_platform_credentials() -> Result<String> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let path = PathBuf::from(home)
        .join(".claude")
        .join(ur_config::CLAUDE_CREDENTIALS_FILENAME);
    debug!(path = %path.display(), "reading credentials from Claude Code config");
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let trimmed = contents.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("{} is empty", path.display());
    }
    info!(path = %path.display(), "credentials read from Claude Code config");
    Ok(trimmed)
}

/// Write content to a file, creating parent directories as needed.
#[instrument(skip(contents), fields(path = %path.display()))]
fn write_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, contents)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_credentials_path_is_under_config_dir() {
        if let Ok(path) = CredentialManager::host_credentials_path() {
            assert!(path.ends_with(ur_config::CLAUDE_CREDENTIALS_FILENAME));
        }
    }

    #[test]
    fn host_config_path_is_under_config_dir() {
        if let Ok(path) = CredentialManager::host_config_path() {
            assert!(path.ends_with(ur_config::CLAUDE_CONFIG_FILENAME));
        }
    }
}
