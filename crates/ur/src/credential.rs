use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use container::{ContainerId, ContainerRuntime, ExecOpts};

/// macOS Keychain service name where Claude Code stores OAuth credentials.
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
/// host and bind-mounted into all agent containers. The app config
/// (`.claude.json`) is baked into the container image.
#[derive(Clone)]
pub struct CredentialManager;

impl CredentialManager {
    /// Ensure credentials exist on disk for container mounting.
    ///
    /// Seeds OAuth credentials from the macOS Keychain if no credentials file
    /// exists yet. After seeding, containers own their session independently —
    /// token refreshes in containers write back to the shared mount without
    /// touching the host Keychain.
    pub fn ensure_credentials(&self) -> Result<()> {
        let creds_path = Self::host_credentials_path()?;

        // Seed credentials from Keychain only if missing — containers manage
        // their own token lifecycle after the initial copy.
        // If Keychain is unavailable (e.g. Linux CI), skip silently —
        // the server creates an empty stub for the Docker mount.
        if !creds_path.exists()
            && let Ok(creds_json) = read_keychain_credentials()
        {
            write_file(&creds_path, &creds_json)?;
        }

        Ok(())
    }

    /// Save credentials and config extracted from a running container to the host config dir.
    ///
    /// Reads both `.credentials.json` and `.claude.json` from the container and
    /// writes them to `$UR_CONFIG/claude/`.
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

        Ok(saved)
    }

    /// Read a file from the container and write it to the host path.
    fn save_file_from_container(
        &self,
        runtime: &impl ContainerRuntime,
        container_id: &ContainerId,
        container_path: &str,
        host_path: &Path,
    ) -> Result<PathBuf> {
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

/// Read Claude Code OAuth credentials from the macOS Keychain.
fn read_keychain_credentials() -> Result<String> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
        .context("failed to run `security` command")?;
    if !output.status.success() {
        anyhow::bail!(
            "no credentials in macOS Keychain for service {KEYCHAIN_SERVICE:?} — \
             log in to Claude Code on this machine first"
        );
    }
    let json =
        String::from_utf8(output.stdout).context("keychain credentials are not valid UTF-8")?;
    let trimmed = json.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("keychain credentials are empty");
    }
    Ok(trimmed)
}

/// Write content to a file, creating parent directories as needed.
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
