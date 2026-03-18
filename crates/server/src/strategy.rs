use std::path::{Path, PathBuf};

use crate::RepoPoolManager;

/// Worker strategy enum governing mode-specific behavior: skill selection,
/// slot acquisition, and slot release. Two variants exist initially: `Code`
/// (exclusive numbered pool slots) and `Design` (shared named slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerStrategy {
    Code,
    Design,
}

impl WorkerStrategy {
    /// Parse a strategy name into a variant.
    /// Valid values: `"code"`, `"design"`.
    pub fn from_name(name: &str) -> Result<Self, String> {
        match name {
            "code" => Ok(Self::Code),
            "design" => Ok(Self::Design),
            other => Err(format!("unknown worker strategy: {other}")),
        }
    }

    /// Returns the name string for this strategy variant.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Design => "design",
        }
    }

    /// Acquire a pool slot for this worker strategy.
    ///
    /// All strategies acquire an exclusive slot via `pool.acquire_slot`.
    ///
    /// Returns (host_path, slot_id) — the host-side path for Docker mounts and the
    /// slot ID for linking via worker_slot.
    pub async fn acquire_slot(
        &self,
        pool: &RepoPoolManager,
        project_key: &str,
    ) -> Result<(PathBuf, String), String> {
        pool.acquire_slot(project_key).await
    }

    /// Release a pool slot for this worker strategy.
    ///
    /// All strategies release the slot via `pool.release_slot`.
    pub async fn release_slot(
        &self,
        pool: &RepoPoolManager,
        worker_id: &str,
        slot_path: &Path,
    ) -> Result<(), String> {
        pool.release_slot(worker_id, slot_path).await
    }

    /// Returns the CLAUDE.md filename (without extension) for this strategy.
    /// Used to set `UR_WORKER_CLAUDE` env var so workerd can copy the right
    /// file from `potential-claudes/` to `~/.claude/CLAUDE.md`.
    pub fn claude_md_name(&self) -> &'static str {
        self.name()
    }

    /// Returns the default skill list for this strategy.
    pub fn skills(&self) -> Vec<String> {
        let mut skills = common_skills();
        match self {
            Self::Code => {
                skills.extend([
                    "implement".into(),
                    "ship".into(),
                    "bacon".into(),
                    "systematic-debugging".into(),
                    "test-driven-development".into(),
                ]);
            }
            Self::Design => {
                skills.extend(["design".into()]);
            }
        }
        skills
    }
}

/// Skills shared by all worker strategies.
fn common_skills() -> Vec<String> {
    vec![
        "push".into(),
        "green".into(),
        "cli-design".into(),
        "reclaude".into(),
        "writing-skills".into(),
        "rag-docs".into(),
        "create-feedback".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_skills_include_code_specific() {
        let skills = WorkerStrategy::Code.skills();
        assert!(skills.contains(&"implement".to_string()));
        assert!(skills.contains(&"ship".to_string()));
        assert!(skills.contains(&"bacon".to_string()));
        assert!(skills.contains(&"systematic-debugging".to_string()));
        assert!(skills.contains(&"test-driven-development".to_string()));
    }

    #[test]
    fn design_skills_include_design() {
        let skills = WorkerStrategy::Design.skills();
        assert!(skills.contains(&"design".to_string()));
        // Design should NOT have code-specific skills
        assert!(!skills.contains(&"implement".to_string()));
        assert!(!skills.contains(&"bacon".to_string()));
    }

    #[test]
    fn both_strategies_include_common_skills() {
        for strategy in [WorkerStrategy::Code, WorkerStrategy::Design] {
            let skills = strategy.skills();
            assert!(skills.contains(&"push".to_string()));
            assert!(skills.contains(&"green".to_string()));
            assert!(skills.contains(&"cli-design".to_string()));
            assert!(skills.contains(&"reclaude".to_string()));
            assert!(skills.contains(&"writing-skills".to_string()));
            assert!(skills.contains(&"rag-docs".to_string()));
            assert!(skills.contains(&"create-feedback".to_string()));
        }
    }

    #[test]
    fn claude_md_name_matches_strategy_name() {
        assert_eq!(WorkerStrategy::Code.claude_md_name(), "code");
        assert_eq!(WorkerStrategy::Design.claude_md_name(), "design");
    }

    #[test]
    fn from_name_valid() {
        assert_eq!(
            WorkerStrategy::from_name("code").unwrap(),
            WorkerStrategy::Code
        );
        assert_eq!(
            WorkerStrategy::from_name("design").unwrap(),
            WorkerStrategy::Design
        );
    }

    #[test]
    fn from_name_invalid() {
        assert!(WorkerStrategy::from_name("unknown").is_err());
    }

    #[test]
    fn name_roundtrip() {
        assert_eq!(
            WorkerStrategy::from_name(WorkerStrategy::Code.name()).unwrap(),
            WorkerStrategy::Code
        );
        assert_eq!(
            WorkerStrategy::from_name(WorkerStrategy::Design.name()).unwrap(),
            WorkerStrategy::Design
        );
    }
}
