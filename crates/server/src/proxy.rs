use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use container::ContainerRuntime;
use tracing::info;

/// Squid proxy container name on the Docker network.
pub const SQUID_CONTAINER_NAME: &str = "ur-squid";

/// Manages Squid proxy config files and runtime allowlist.
///
/// Config files live in a host directory (`$UR_CONFIG/squid/`) mounted into the
/// Squid container at `/etc/squid/`. The Squid container itself is managed by
/// Docker Compose — this manager only handles config and reconfigure signals.
///
/// Allowlist changes: rewrite `allowlist.txt`, then `signal_reconfigure()` to
/// tell Squid to re-read its config without restarting.
#[derive(Clone)]
pub struct SquidManager {
    config_dir: PathBuf,
    allowlist: Arc<RwLock<Vec<String>>>,
}

impl SquidManager {
    pub fn new(config_dir: PathBuf, allowlist: Vec<String>) -> Self {
        Self {
            config_dir,
            allowlist: Arc::new(RwLock::new(allowlist)),
        }
    }

    /// Replace the entire allowlist and write to disk.
    pub fn update_allowlist(&self, domains: Vec<String>) -> Result<()> {
        *self.allowlist.write().expect("allowlist lock poisoned") = domains;
        self.write_allowlist_file()
    }

    /// Add a domain to the allowlist and write to disk. No-op if already present.
    pub fn add_domain(&self, domain: &str) -> Result<()> {
        {
            let mut list = self.allowlist.write().expect("allowlist lock poisoned");
            if !list.iter().any(|d| d == domain) {
                list.push(domain.to_string());
            }
        }
        self.write_allowlist_file()
    }

    /// Remove a domain from the allowlist and write to disk.
    pub fn remove_domain(&self, domain: &str) -> Result<()> {
        self.allowlist
            .write()
            .expect("allowlist lock poisoned")
            .retain(|d| d != domain);
        self.write_allowlist_file()
    }

    /// Return a snapshot of the current allowlist.
    pub fn list_domains(&self) -> Vec<String> {
        self.allowlist
            .read()
            .expect("allowlist lock poisoned")
            .clone()
    }

    /// Signal the Squid container to re-read config files.
    /// Sends `squid -k reconfigure` via `docker exec`. Active connections are
    /// preserved; new connections use the updated allowlist.
    pub fn signal_reconfigure(&self) -> Result<()> {
        let rt = container::runtime_from_env();
        let cid = container::ContainerId(SQUID_CONTAINER_NAME.to_string());
        let opts = container::ExecOpts {
            command: vec!["squid".into(), "-k".into(), "reconfigure".into()],
            workdir: None,
        };
        let output = rt.exec(&cid, &opts).context("signal squid reconfigure")?;
        if output.exit_code != 0 {
            anyhow::bail!("squid reconfigure failed: {}", output.stderr);
        }
        info!(container = SQUID_CONTAINER_NAME, "squid reconfigure signaled");
        Ok(())
    }

    fn write_allowlist_file(&self) -> Result<()> {
        let list = self.allowlist.read().expect("allowlist lock poisoned");
        let content = list.join("\n") + "\n";
        std::fs::write(self.config_dir.join("allowlist.txt"), content)
            .context("write allowlist.txt")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_allowlist() -> Vec<String> {
        vec!["api.anthropic.com".into(), "example.com".into()]
    }

    #[test]
    fn writes_allowlist_file() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(tmp.path().to_path_buf(), test_allowlist());
        manager.update_allowlist(test_allowlist()).unwrap();

        let allowlist = std::fs::read_to_string(tmp.path().join("allowlist.txt")).unwrap();
        assert!(allowlist.contains("api.anthropic.com"));
        assert!(allowlist.contains("example.com"));
    }

    #[test]
    fn update_allowlist_rewrites_file() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(tmp.path().to_path_buf(), test_allowlist());
        manager.update_allowlist(test_allowlist()).unwrap();

        manager
            .update_allowlist(vec!["new.example.com".into()])
            .unwrap();

        let content = std::fs::read_to_string(tmp.path().join("allowlist.txt")).unwrap();
        assert!(content.contains("new.example.com"));
        assert!(!content.contains("api.anthropic.com"));
    }

    #[test]
    fn add_domain_appends() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(tmp.path().to_path_buf(), test_allowlist());
        manager.update_allowlist(test_allowlist()).unwrap();

        manager.add_domain("new.example.com").unwrap();

        let domains = manager.list_domains();
        assert!(domains.contains(&"new.example.com".to_string()));
        assert!(domains.contains(&"api.anthropic.com".to_string()));
    }

    #[test]
    fn add_domain_deduplicates() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(tmp.path().to_path_buf(), test_allowlist());
        manager.update_allowlist(test_allowlist()).unwrap();

        manager.add_domain("api.anthropic.com").unwrap();

        assert_eq!(manager.list_domains().len(), 2);
    }

    #[test]
    fn remove_domain() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(tmp.path().to_path_buf(), test_allowlist());
        manager.update_allowlist(test_allowlist()).unwrap();

        manager.remove_domain("example.com").unwrap();

        let domains = manager.list_domains();
        assert!(!domains.contains(&"example.com".to_string()));
        assert!(domains.contains(&"api.anthropic.com".to_string()));
    }

    #[test]
    fn list_domains_returns_current() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(tmp.path().to_path_buf(), test_allowlist());
        assert_eq!(manager.list_domains().len(), 2);
    }
}
