use std::path::{Path, PathBuf};

use crate::RepoPoolManager;

/// Worker strategy enum governing mode-specific behavior: skill selection
/// and slot acquisition. Three variants exist: `Code`
/// (exclusive numbered pool slots), `Design` (shared named slot), and
/// `Manual` (exclusive numbered pool slots, no branch checkout, opus model,
/// all skills).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerStrategy {
    Code,
    Design,
    Manual,
}

impl WorkerStrategy {
    /// All strategy variants, so callers can seed per-strategy maps or assert
    /// per-strategy invariants without re-listing the variants at each site.
    pub const ALL: &'static [WorkerStrategy] = &[Self::Code, Self::Design, Self::Manual];

    /// Parse a strategy name into a variant.
    /// Valid values: `"code"`, `"design"`, `"manual"`.
    pub fn from_name(name: &str) -> Result<Self, String> {
        match name {
            "code" => Ok(Self::Code),
            "design" => Ok(Self::Design),
            "manual" => Ok(Self::Manual),
            other => Err(format!("unknown worker strategy: {other}")),
        }
    }

    /// Returns the name string for this strategy variant.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Design => "design",
            Self::Manual => "manual",
        }
    }

    /// Acquire a pool slot for this worker strategy.
    ///
    /// - `Code` / `Manual`: an exclusive slot via `pool.acquire_slot`, claimed
    ///   for the `provisioning` row `owner` builds (returned as `Some`). The
    ///   claim lasts until the worker's stop deletes its link.
    /// - `Design`: the shared slot via `pool.acquire_shared_slot` — no claim,
    ///   no DB ownership.
    pub async fn acquire_slot<F>(
        &self,
        pool: &RepoPoolManager,
        project_key: &str,
        owner: F,
    ) -> Result<(PathBuf, Option<workflow_db::model::Worker>), String>
    where
        F: Fn(&Path) -> workflow_db::model::Worker,
    {
        match self {
            Self::Code | Self::Manual => {
                let (path, row) = pool.acquire_slot(project_key, owner).await?;
                Ok((path, Some(row)))
            }
            Self::Design => {
                let path = pool.acquire_shared_slot(project_key).await?;
                Ok((path, None))
            }
        }
    }

    /// Returns the instruction-strategy name (without extension) for this
    /// strategy. Used to set `UR_WORKER_INSTRUCTION_STRATEGY` env var so
    /// workerd can copy the right file from `.agent-shared/instructions/` to
    /// `~/{agent.customization_root()}/{agent.instruction_filename()}`
    /// (e.g. `~/.claude/CLAUDE.md` or `~/.gemini/config/AGENTS.md`).
    pub fn instruction_strategy_name(&self) -> &'static str {
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
                skills.extend(["design".into(), "dispatch".into()]);
            }
            Self::Manual => {
                skills.extend([
                    "implement".into(),
                    "implement-agents".into(),
                    "ship".into(),
                    "bacon".into(),
                    "systematic-debugging".into(),
                    "test-driven-development".into(),
                    "design".into(),
                    "dispatch".into(),
                    // Brain consolidation: a person drives it, so manual workers only.
                    "brain:dream".into(),
                    "brain:dream-init".into(),
                ]);
            }
        }
        skills
    }
}

/// Skills shared by all worker strategies.
fn common_skills() -> Vec<String> {
    vec![
        "green".into(),
        "cli-design".into(),
        "reclaude".into(),
        "writing-skills".into(),
        "rag-docs".into(),
        "address-feedback".into(),
        "code-review".into(),
        "brain".into(),
        "brain:init".into(),
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
        for strategy in [
            WorkerStrategy::Code,
            WorkerStrategy::Design,
            WorkerStrategy::Manual,
        ] {
            let skills = strategy.skills();
            assert!(skills.contains(&"green".to_string()));
            assert!(skills.contains(&"cli-design".to_string()));
            assert!(skills.contains(&"reclaude".to_string()));
            assert!(skills.contains(&"writing-skills".to_string()));
            assert!(skills.contains(&"rag-docs".to_string()));
            assert!(skills.contains(&"address-feedback".to_string()));
            assert!(skills.contains(&"code-review".to_string()));
            assert!(skills.contains(&"brain".to_string()));
            assert!(skills.contains(&"brain:init".to_string()));
        }
    }

    #[test]
    fn brain_dream_skills_are_manual_only() {
        for strategy in [WorkerStrategy::Code, WorkerStrategy::Design] {
            let skills = strategy.skills();
            assert!(!skills.contains(&"brain:dream".to_string()));
            assert!(!skills.contains(&"brain:dream-init".to_string()));
        }
    }

    #[test]
    fn manual_skills_include_all_categories() {
        let skills = WorkerStrategy::Manual.skills();
        // Code-specific skills
        assert!(skills.contains(&"implement".to_string()));
        assert!(skills.contains(&"ship".to_string()));
        assert!(skills.contains(&"bacon".to_string()));
        assert!(skills.contains(&"systematic-debugging".to_string()));
        assert!(skills.contains(&"test-driven-development".to_string()));
        // Subagent-dispatch variant is available to manual workers (opt-in)
        assert!(skills.contains(&"implement-agents".to_string()));
        // Design-specific skills
        assert!(skills.contains(&"design".to_string()));
        assert!(skills.contains(&"dispatch".to_string()));
        // Brain consolidation
        assert!(skills.contains(&"brain:dream".to_string()));
        assert!(skills.contains(&"brain:dream-init".to_string()));
        // Common skills
        assert!(skills.contains(&"green".to_string()));
        assert!(skills.contains(&"cli-design".to_string()));
        assert!(skills.contains(&"reclaude".to_string()));
        assert!(skills.contains(&"brain".to_string()));
        assert!(skills.contains(&"brain:init".to_string()));
    }

    #[test]
    fn code_skills_exclude_implement_agents() {
        // The automated code flow stays agent-free by default; the
        // subagent-dispatch variant is opt-in (manual workers only).
        let skills = WorkerStrategy::Code.skills();
        assert!(skills.contains(&"implement".to_string()));
        assert!(!skills.contains(&"implement-agents".to_string()));
    }

    #[test]
    fn manual_name_roundtrip() {
        assert_eq!(WorkerStrategy::Manual.name(), "manual");
        assert_eq!(
            WorkerStrategy::from_name("manual").unwrap(),
            WorkerStrategy::Manual
        );
    }

    #[test]
    fn manual_instruction_strategy_name() {
        assert_eq!(WorkerStrategy::Manual.instruction_strategy_name(), "manual");
    }

    #[test]
    fn instruction_strategy_name_matches_strategy_name() {
        assert_eq!(WorkerStrategy::Code.instruction_strategy_name(), "code");
        assert_eq!(WorkerStrategy::Design.instruction_strategy_name(), "design");
        assert_eq!(WorkerStrategy::Manual.instruction_strategy_name(), "manual");
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
        assert_eq!(
            WorkerStrategy::from_name("manual").unwrap(),
            WorkerStrategy::Manual
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
        assert_eq!(
            WorkerStrategy::from_name(WorkerStrategy::Manual.name()).unwrap(),
            WorkerStrategy::Manual
        );
    }
}
