use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use ur_config::ProjectConfig;

use crate::hostexec::HostExecConfigManager;

/// Report of what changed during a reload.
pub struct ReloadReport {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

struct ProjectRegistryInner {
    projects: HashMap<String, ProjectConfig>,
    hostexec_config: HostExecConfigManager,
}

/// Centralizes project config access behind `Arc<RwLock>`, allowing live reload
/// of `ur.toml` projects and hostexec configuration without restarting the server.
#[derive(Clone)]
pub struct ProjectRegistry {
    inner: Arc<RwLock<ProjectRegistryInner>>,
}

impl ProjectRegistry {
    pub fn new(
        projects: HashMap<String, ProjectConfig>,
        hostexec_config: HostExecConfigManager,
    ) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ProjectRegistryInner {
                projects,
                hostexec_config,
            })),
        }
    }

    /// Clone a single project config by key.
    pub fn get(&self, key: &str) -> Option<ProjectConfig> {
        let inner = self.inner.read().expect("ProjectRegistry lock poisoned");
        inner.projects.get(key).cloned()
    }

    /// Clone the full project map.
    pub fn projects(&self) -> HashMap<String, ProjectConfig> {
        let inner = self.inner.read().expect("ProjectRegistry lock poisoned");
        inner.projects.clone()
    }

    /// Clone the hostexec config manager.
    pub fn hostexec_config(&self) -> HostExecConfigManager {
        let inner = self.inner.read().expect("ProjectRegistry lock poisoned");
        inner.hostexec_config.clone()
    }

    /// Return the set of valid project keys.
    pub fn valid_project_keys(&self) -> HashSet<String> {
        let inner = self.inner.read().expect("ProjectRegistry lock poisoned");
        inner.projects.keys().cloned().collect()
    }

    /// Re-read `ur.toml` from `config_dir`, rebuild projects and hostexec config,
    /// swap state under write lock, and return what changed.
    ///
    /// The write lock is held only for the final swap — all I/O happens before acquiring it.
    pub fn reload(&self, config_dir: &Path) -> Result<ReloadReport> {
        let config = ur_config::Config::load_from(config_dir)?;
        let new_hostexec = HostExecConfigManager::load(&config.config_dir, &config.hostexec)?;
        let new_projects = config.projects;

        let mut inner = self.inner.write().expect("ProjectRegistry lock poisoned");

        let old_keys: HashSet<String> = inner.projects.keys().cloned().collect();
        let new_keys: HashSet<String> = new_projects.keys().cloned().collect();

        let mut added: Vec<String> = new_keys.difference(&old_keys).cloned().collect();
        let mut removed: Vec<String> = old_keys.difference(&new_keys).cloned().collect();
        added.sort();
        removed.sort();

        inner.projects = new_projects;
        inner.hostexec_config = new_hostexec;

        Ok(ReloadReport { added, removed })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_toml(dir: &Path, content: &str) {
        fs::write(dir.join("ur.toml"), content).unwrap();
    }

    fn toml_with_project(key: &str) -> String {
        format!(
            r#"
[projects.{key}]
repo = "git@github.com:test/{key}.git"
[projects.{key}.container]
image = "ur-worker"
"#
        )
    }

    #[test]
    fn reload_detects_added_and_removed_projects() {
        let tmp = TempDir::new().unwrap();
        write_toml(tmp.path(), &toml_with_project("alpha"));

        let config = ur_config::Config::load_from(tmp.path()).unwrap();
        let hostexec = HostExecConfigManager::load(tmp.path(), &config.hostexec).unwrap();
        let registry = ProjectRegistry::new(config.projects, hostexec);

        assert!(registry.get("alpha").is_some());
        assert!(registry.get("beta").is_none());

        // Rewrite ur.toml: remove alpha, add beta
        write_toml(tmp.path(), &toml_with_project("beta"));

        let report = registry.reload(tmp.path()).unwrap();
        assert_eq!(report.added, vec!["beta"]);
        assert_eq!(report.removed, vec!["alpha"]);

        // New state is visible
        assert!(registry.get("alpha").is_none());
        assert!(registry.get("beta").is_some());
        assert_eq!(
            registry.valid_project_keys(),
            HashSet::from(["beta".into()])
        );
    }

    #[test]
    fn reload_with_invalid_toml_preserves_old_state() {
        let tmp = TempDir::new().unwrap();
        write_toml(tmp.path(), &toml_with_project("alpha"));

        let config = ur_config::Config::load_from(tmp.path()).unwrap();
        let hostexec = HostExecConfigManager::load(tmp.path(), &config.hostexec).unwrap();
        let registry = ProjectRegistry::new(config.projects, hostexec);

        // Write invalid TOML
        write_toml(tmp.path(), "not valid [[[ toml");

        let result = registry.reload(tmp.path());
        assert!(result.is_err());

        // Old state preserved
        assert!(registry.get("alpha").is_some());
        assert_eq!(
            registry.valid_project_keys(),
            HashSet::from(["alpha".into()])
        );
    }

    #[test]
    fn projects_returns_full_map() {
        let tmp = TempDir::new().unwrap();
        let toml = format!(
            "{}\n{}",
            toml_with_project("alpha"),
            toml_with_project("beta")
        );
        write_toml(tmp.path(), &toml);

        let config = ur_config::Config::load_from(tmp.path()).unwrap();
        let hostexec = HostExecConfigManager::load(tmp.path(), &config.hostexec).unwrap();
        let registry = ProjectRegistry::new(config.projects, hostexec);

        let map = registry.projects();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("alpha"));
        assert!(map.contains_key("beta"));
    }

    #[test]
    fn hostexec_config_is_accessible() {
        let tmp = TempDir::new().unwrap();
        write_toml(tmp.path(), &toml_with_project("alpha"));

        let config = ur_config::Config::load_from(tmp.path()).unwrap();
        let hostexec = HostExecConfigManager::load(tmp.path(), &config.hostexec).unwrap();
        let registry = ProjectRegistry::new(config.projects, hostexec);

        let hec = registry.hostexec_config();
        // Default commands should be present
        assert!(hec.is_allowed("git"));
    }
}
