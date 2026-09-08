use std::path::{Path, PathBuf};

use crate::RepoPoolManager;

/// Worker strategy enum governing mode-specific behavior: skill selection,
/// slot acquisition, and slot release. Three variants exist: `Code`
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
    /// - `Code`: acquires an exclusive slot via `pool.acquire_slot`, returning
    ///   `(host_path, Some(claim))` for DB linking via worker_slot.
    /// - `Design`: acquires a shared slot via `pool.acquire_shared_slot`, returning
    ///   `(host_path, None)` — no DB ownership tracking.
    /// - `Manual`: acquires an exclusive slot via `pool.acquire_slot` (same as
    ///   `Code`), returning `(host_path, Some(claim))`.
    ///
    /// The returned `SlotClaim` must be held until the slot is linked to a worker; see
    /// `SlotClaim` for why ownership rather than a bare slot ID is handed back.
    pub async fn acquire_slot(
        &self,
        pool: &RepoPoolManager,
        project_key: &str,
    ) -> Result<(PathBuf, Option<crate::pool::SlotClaim>), String> {
        match self {
            Self::Code | Self::Manual => {
                let (path, claim) = pool.acquire_slot(project_key).await?;
                Ok((path, Some(claim)))
            }
            Self::Design => {
                let path = pool.acquire_shared_slot(project_key).await?;
                Ok((path, None))
            }
        }
    }

    /// Release a pool slot for this worker strategy.
    ///
    /// - `Code`: releases the exclusive slot via `pool.release_slot`.
    /// - `Design`: no-op — shared slots have no DB ownership tracking.
    /// - `Manual`: releases the exclusive slot via `pool.release_slot` (same as
    ///   `Code`).
    pub async fn release_slot(
        &self,
        pool: &RepoPoolManager,
        worker_id: &str,
        slot_path: &Path,
    ) -> Result<(), String> {
        match self {
            Self::Code | Self::Manual => pool.release_slot(worker_id, slot_path).await,
            Self::Design => Ok(()),
        }
    }

    /// Returns the instruction-strategy name (without extension) for this
    /// strategy. Used to set `UR_WORKER_INSTRUCTION_STRATEGY` env var so
    /// workerd can copy the right file from `.agent-shared/instructions/` to
    /// `~/{agent.home_subdir()}/{agent.instruction_filename()}` (e.g. `~/.claude/CLAUDE.md`).
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
