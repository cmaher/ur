use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

/// In-memory map of process_id -> repo directory path.
/// TEMPORARY: will be replaced by ur_db.
pub struct RepoRegistry {
    workspace: PathBuf,
    /// process_id -> absolute repo path
    repos: RwLock<HashMap<String, PathBuf>>,
}

impl RepoRegistry {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            repos: RwLock::new(HashMap::new()),
        }
    }

    /// Register a process with its repo subdirectory within the workspace.
    pub fn register(&self, process_id: &str, repo_name: &str) {
        self.repos
            .write()
            .expect("repo registry lock poisoned")
            .insert(process_id.to_string(), self.workspace.join(repo_name));
    }

    /// Register a process with an absolute path (e.g., a pre-existing workspace directory).
    pub fn register_absolute(&self, process_id: &str, path: PathBuf) {
        self.repos
            .write()
            .expect("repo registry lock poisoned")
            .insert(process_id.to_string(), path);
    }

    /// Remove a process from the registry.
    pub fn unregister(&self, process_id: &str) {
        self.repos
            .write()
            .expect("repo registry lock poisoned")
            .remove(process_id);
    }

    /// Resolve a process_id to its full repo path.
    #[cfg(test)]
    pub(crate) fn resolve(&self, process_id: &str) -> Result<PathBuf, String> {
        let repos = self.repos.read().expect("repo registry lock poisoned");
        repos
            .get(process_id)
            .cloned()
            .ok_or_else(|| format!("unknown process_id: {process_id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_resolve_unknown_process() {
        let reg = RepoRegistry::new(PathBuf::from("/workspace"));
        let err = reg.resolve("unknown").unwrap_err();
        assert!(err.contains("unknown process_id"));
    }

    #[test]
    fn registry_resolve_known_process() {
        let reg = RepoRegistry::new(PathBuf::from("/workspace"));
        reg.register("p1", "my-repo");
        let path = reg.resolve("p1").unwrap();
        assert_eq!(path, PathBuf::from("/workspace/my-repo"));
    }

    #[test]
    fn registry_unregister() {
        let reg = RepoRegistry::new(PathBuf::from("/workspace"));
        reg.register("p1", "my-repo");
        reg.unregister("p1");
        assert!(reg.resolve("p1").is_err());
    }

    #[test]
    fn registry_register_absolute() {
        let reg = RepoRegistry::new(PathBuf::from("/workspace"));
        let abs_path = PathBuf::from("/home/user/my-project");
        reg.register_absolute("p1", abs_path.clone());
        let path = reg.resolve("p1").unwrap();
        assert_eq!(path, abs_path);
    }

    #[test]
    fn registry_absolute_does_not_join_workspace() {
        let reg = RepoRegistry::new(PathBuf::from("/workspace"));
        let abs_path = PathBuf::from("/other/dir");
        reg.register_absolute("p1", abs_path.clone());
        let resolved = reg.resolve("p1").unwrap();
        // Should NOT be /workspace/other/dir — should be the absolute path directly
        assert_eq!(resolved, abs_path);
        assert!(!resolved.starts_with("/workspace"));
    }
}
