use std::collections::{HashMap, HashSet};

use ur_config::ProjectConfig;

/// Per-project allow-list for hostexec scripts.
///
/// Built from the project map at server start and refreshed when `ur.toml`
/// reloads. Answers: "is `rel_path` an allowed hostexec script for
/// `project_key`?"
///
/// Stored paths are already in canonical form (no leading `./`) because
/// `ProjectConfig::hostexec_scripts` is normalized at parse time. Incoming
/// paths are normalized the same way before lookup.
#[derive(Clone, Default)]
pub struct ScriptRegistry {
    /// Map from project key → set of allowed script paths (canonical form).
    allowed: HashMap<String, HashSet<String>>,
}

impl ScriptRegistry {
    /// Build a `ScriptRegistry` from the project map used by `ProjectRegistry`.
    pub fn from_projects(projects: &HashMap<String, ProjectConfig>) -> Self {
        let allowed = projects
            .iter()
            .map(|(key, cfg)| {
                let paths: HashSet<String> = cfg.hostexec_scripts.iter().cloned().collect();
                (key.clone(), paths)
            })
            .collect();
        Self { allowed }
    }

    /// Return `true` if `rel_path` is an allowed hostexec script for
    /// `project_key`.
    ///
    /// `rel_path` is normalized (leading `./` stripped) before the lookup so
    /// callers do not need to canonicalize first.
    pub fn allows(&self, project_key: &str, rel_path: &str) -> bool {
        let normalized = rel_path.strip_prefix("./").unwrap_or(rel_path);
        self.allowed
            .get(project_key)
            .map(|scripts| scripts.contains(normalized))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ur_config::{ContainerConfig, ProjectConfig};

    use super::*;

    fn make_project(key: &str, scripts: Vec<&str>) -> ProjectConfig {
        ProjectConfig {
            key: key.to_string(),
            repo: format!("git@github.com:test/{key}.git"),
            name: key.to_string(),
            pool_limit: 10,
            hostexec: vec![],
            git_hooks_dir: None,
            skill_hooks_dir: None,
            claude_md: None,
            container: ContainerConfig {
                image: "ur-worker".to_string(),
                mounts: vec![],
                ports: vec![],
            },
            workflow_hooks_dir: None,
            max_fix_attempts: 5,
            protected_branches: vec!["main".to_string(), "master".to_string()],
            tui: None,
            ignored_workflow_checks: vec![],
            hostexec_scripts: scripts.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    fn registry_with_scripts(project_key: &str, scripts: Vec<&str>) -> ScriptRegistry {
        let mut projects = HashMap::new();
        projects.insert(project_key.to_string(), make_project(project_key, scripts));
        ScriptRegistry::from_projects(&projects)
    }

    #[test]
    fn present_project_present_path_returns_true() {
        let registry = registry_with_scripts("myproject", vec!["scripts/deploy.sh"]);
        assert!(registry.allows("myproject", "scripts/deploy.sh"));
    }

    #[test]
    fn present_project_absent_path_returns_false() {
        let registry = registry_with_scripts("myproject", vec!["scripts/deploy.sh"]);
        assert!(!registry.allows("myproject", "scripts/other.sh"));
    }

    #[test]
    fn absent_project_returns_false() {
        let registry = registry_with_scripts("myproject", vec!["scripts/deploy.sh"]);
        assert!(!registry.allows("unknown", "scripts/deploy.sh"));
    }

    #[test]
    fn dotslash_prefix_normalized_at_lookup() {
        // Stored without ./ (canonical form), looked up with ./
        let registry = registry_with_scripts("myproject", vec!["scripts/deploy.sh"]);
        assert!(registry.allows("myproject", "./scripts/deploy.sh"));
    }

    #[test]
    fn stored_without_prefix_and_looked_up_without_prefix() {
        let registry = registry_with_scripts("myproject", vec!["scripts/deploy.sh"]);
        assert!(registry.allows("myproject", "scripts/deploy.sh"));
    }

    #[test]
    fn multiple_projects_independent() {
        let mut projects = HashMap::new();
        projects.insert("alpha".to_string(), make_project("alpha", vec!["run.sh"]));
        projects.insert("beta".to_string(), make_project("beta", vec!["other.sh"]));
        let registry = ScriptRegistry::from_projects(&projects);

        assert!(registry.allows("alpha", "run.sh"));
        assert!(!registry.allows("alpha", "other.sh"));
        assert!(registry.allows("beta", "other.sh"));
        assert!(!registry.allows("beta", "run.sh"));
    }

    #[test]
    fn project_with_empty_scripts_denies_all() {
        let registry = registry_with_scripts("myproject", vec![]);
        assert!(!registry.allows("myproject", "anything.sh"));
    }

    #[test]
    fn multiple_scripts_per_project() {
        let registry = registry_with_scripts("myproject", vec!["a.sh", "b.sh", "subdir/c.sh"]);
        assert!(registry.allows("myproject", "a.sh"));
        assert!(registry.allows("myproject", "b.sh"));
        assert!(registry.allows("myproject", "subdir/c.sh"));
        assert!(!registry.allows("myproject", "d.sh"));
    }
}
