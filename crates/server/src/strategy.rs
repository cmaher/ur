/// Worker strategy enum governing mode-specific behavior: skill selection,
/// slot acquisition, and slot release. Two variants ship initially: `Code`
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

    /// Returns the default skill list for this strategy.
    pub fn skills(&self) -> Vec<String> {
        let mut skills = common_skills();
        match self {
            Self::Code => {
                skills.extend([
                    "tk:agents".into(),
                    "tk:start".into(),
                    "bacon".into(),
                    "systematic-debugging".into(),
                    "test-driven-development".into(),
                ]);
            }
            Self::Design => {
                skills.extend(["brainstorming".into()]);
            }
        }
        skills
    }
}

/// Skills shared by all worker strategies.
fn common_skills() -> Vec<String> {
    vec![
        "tk".into(),
        "ship".into(),
        "green".into(),
        "cli-design".into(),
        "reclaude".into(),
        "writing-skills".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_skills_include_code_specific() {
        let skills = WorkerStrategy::Code.skills();
        assert!(skills.contains(&"tk:agents".to_string()));
        assert!(skills.contains(&"tk:start".to_string()));
        assert!(skills.contains(&"bacon".to_string()));
        assert!(skills.contains(&"systematic-debugging".to_string()));
        assert!(skills.contains(&"test-driven-development".to_string()));
    }

    #[test]
    fn design_skills_include_brainstorming() {
        let skills = WorkerStrategy::Design.skills();
        assert!(skills.contains(&"brainstorming".to_string()));
        // Design should NOT have code-specific skills
        assert!(!skills.contains(&"tk:agents".to_string()));
        assert!(!skills.contains(&"bacon".to_string()));
    }

    #[test]
    fn both_strategies_include_common_skills() {
        for strategy in [WorkerStrategy::Code, WorkerStrategy::Design] {
            let skills = strategy.skills();
            assert!(skills.contains(&"tk".to_string()));
            assert!(skills.contains(&"ship".to_string()));
            assert!(skills.contains(&"green".to_string()));
            assert!(skills.contains(&"cli-design".to_string()));
            assert!(skills.contains(&"reclaude".to_string()));
            assert!(skills.contains(&"writing-skills".to_string()));
        }
    }

    #[test]
    fn from_name_valid() {
        assert_eq!(WorkerStrategy::from_name("code").unwrap(), WorkerStrategy::Code);
        assert_eq!(WorkerStrategy::from_name("design").unwrap(), WorkerStrategy::Design);
    }

    #[test]
    fn from_name_invalid() {
        assert!(WorkerStrategy::from_name("unknown").is_err());
    }

    #[test]
    fn name_roundtrip() {
        assert_eq!(WorkerStrategy::from_name(WorkerStrategy::Code.name()).unwrap(), WorkerStrategy::Code);
        assert_eq!(WorkerStrategy::from_name(WorkerStrategy::Design.name()).unwrap(), WorkerStrategy::Design);
    }
}
