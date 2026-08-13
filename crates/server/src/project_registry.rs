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

    /// True when `key` names a configured local project (`local = true`, no `repo`).
    ///
    /// Returns false for unknown keys: absence is a different failure, reported by
    /// the caller's own lookup with a better message than "not local".
    pub fn is_local(&self, key: &str) -> bool {
        let inner = self.inner.read().expect("ProjectRegistry lock poisoned");
        inner.projects.get(key).is_some_and(ProjectConfig::is_local)
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
        self.apply(config)
    }

    /// Reload from caller-supplied `ur.toml` bytes instead of re-reading the
    /// file. Use this when the caller just wrote the file and is sending the
    /// authoritative contents over RPC — avoids macOS Docker Desktop
    /// bind-mount propagation lag where the server's view of the file can
    /// briefly be stale or partial.
    pub fn reload_from_str(&self, toml: &str, config_dir: &Path) -> Result<ReloadReport> {
        let config = ur_config::Config::from_toml_str(toml, config_dir)?;
        self.apply(config)
    }

    fn apply(&self, config: ur_config::Config) -> Result<ReloadReport> {
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
        let full = format!("node_id = \"n\"\n{content}");
        fs::write(dir.join("ur.toml"), full).unwrap();
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

    fn toml_with_local_project(key: &str) -> String {
        format!(
            r#"
[projects.{key}]
local = true
[projects.{key}.container]
image = "ur-worker"
"#
        )
    }

    fn registry_from_toml(dir: &Path, content: &str) -> ProjectRegistry {
        write_toml(dir, content);
        let config = ur_config::Config::load_from(dir).unwrap();
        let hostexec = HostExecConfigManager::load(dir, &config.hostexec).unwrap();
        ProjectRegistry::new(config.projects, hostexec)
    }

    #[test]
    fn is_local_distinguishes_local_repo_backed_and_unknown() {
        let tmp = TempDir::new().unwrap();
        let content = format!(
            "{}{}",
            toml_with_project("alpha"),
            toml_with_local_project("myapp")
        );
        let registry = registry_from_toml(tmp.path(), &content);

        assert!(registry.is_local("myapp"));
        assert!(!registry.is_local("alpha"));
        // An unknown key is not "local" — absence is a different failure, owned by
        // the caller's own lookup.
        assert!(!registry.is_local("nope"));
        assert!(!registry.is_local(""));
    }

    #[test]
    fn reload_flips_project_between_local_and_repo_backed() {
        let tmp = TempDir::new().unwrap();
        let registry = registry_from_toml(tmp.path(), &toml_with_local_project("myapp"));
        assert!(registry.is_local("myapp"));

        // Give the project a repo: it must stop reading as local.
        write_toml(tmp.path(), &toml_with_project("myapp"));
        registry.reload(tmp.path()).unwrap();
        assert!(!registry.is_local("myapp"));

        // And back again.
        write_toml(tmp.path(), &toml_with_local_project("myapp"));
        registry.reload(tmp.path()).unwrap();
        assert!(registry.is_local("myapp"));
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
