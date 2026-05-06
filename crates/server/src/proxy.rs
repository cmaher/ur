use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use tracing::info;
use ur_rpc::proto::builder_container::ExecContainerRequest;

use crate::builder_container_client::BuilderContainerClient;

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
    /// Container name used to exec `squid -k reconfigure` (e.g., "ur-squid").
    container_name: String,
    allowlist: Arc<RwLock<Vec<String>>>,
    /// gRPC client used to exec commands in the Squid container via builderd.
    builder_container_client: BuilderContainerClient,
}

impl SquidManager {
    pub fn new(
        config_dir: PathBuf,
        container_name: String,
        allowlist: Vec<String>,
        builder_container_client: BuilderContainerClient,
    ) -> Self {
        Self {
            config_dir,
            container_name,
            allowlist: Arc::new(RwLock::new(allowlist)),
            builder_container_client,
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
    /// Sends `squid -k reconfigure` via builderd exec. Active connections are
    /// preserved; new connections use the updated allowlist.
    pub async fn signal_reconfigure(&self) -> Result<()> {
        let request = ExecContainerRequest {
            container_id: self.container_name.clone(),
            command: "squid".into(),
            args: vec!["-k".into(), "reconfigure".into()],
            workdir: String::new(),
        };
        let output = self
            .builder_container_client
            .exec_container(request)
            .await
            .context("signal squid reconfigure")?;
        if output.exit_code != 0 {
            anyhow::bail!("squid reconfigure failed: {}", output.stderr);
        }
        info!(
            container = %self.container_name,
            "squid reconfigure signaled"
        );
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

    fn dummy_client() -> BuilderContainerClient {
        // Channel is lazy — it won't connect until a call is made.
        // Tests here only exercise file I/O, not exec, so this is safe.
        let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();
        BuilderContainerClient::new(channel)
    }

    #[tokio::test]
    async fn writes_allowlist_file() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(
            tmp.path().to_path_buf(),
            "ur-squid".into(),
            test_allowlist(),
            dummy_client(),
        );
        manager.update_allowlist(test_allowlist()).unwrap();

        let allowlist = std::fs::read_to_string(tmp.path().join("allowlist.txt")).unwrap();
        assert!(allowlist.contains("api.anthropic.com"));
        assert!(allowlist.contains("example.com"));
    }

    #[tokio::test]
    async fn update_allowlist_rewrites_file() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(
            tmp.path().to_path_buf(),
            "ur-squid".into(),
            test_allowlist(),
            dummy_client(),
        );
        manager.update_allowlist(test_allowlist()).unwrap();

        manager
            .update_allowlist(vec!["new.example.com".into()])
            .unwrap();

        let content = std::fs::read_to_string(tmp.path().join("allowlist.txt")).unwrap();
        assert!(content.contains("new.example.com"));
        assert!(!content.contains("api.anthropic.com"));
    }

    #[tokio::test]
    async fn add_domain_appends() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(
            tmp.path().to_path_buf(),
            "ur-squid".into(),
            test_allowlist(),
            dummy_client(),
        );
        manager.update_allowlist(test_allowlist()).unwrap();

        manager.add_domain("new.example.com").unwrap();

        let domains = manager.list_domains();
        assert!(domains.contains(&"new.example.com".to_string()));
        assert!(domains.contains(&"api.anthropic.com".to_string()));
    }

    #[tokio::test]
    async fn add_domain_deduplicates() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(
            tmp.path().to_path_buf(),
            "ur-squid".into(),
            test_allowlist(),
            dummy_client(),
        );
        manager.update_allowlist(test_allowlist()).unwrap();

        manager.add_domain("api.anthropic.com").unwrap();

        assert_eq!(manager.list_domains().len(), 2);
    }

    #[tokio::test]
    async fn remove_domain() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(
            tmp.path().to_path_buf(),
            "ur-squid".into(),
            test_allowlist(),
            dummy_client(),
        );
        manager.update_allowlist(test_allowlist()).unwrap();

        manager.remove_domain("example.com").unwrap();

        let domains = manager.list_domains();
        assert!(!domains.contains(&"example.com".to_string()));
        assert!(domains.contains(&"api.anthropic.com".to_string()));
    }

    #[tokio::test]
    async fn list_domains_returns_current() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(
            tmp.path().to_path_buf(),
            "ur-squid".into(),
            test_allowlist(),
            dummy_client(),
        );
        assert_eq!(manager.list_domains().len(), 2);
    }
}
