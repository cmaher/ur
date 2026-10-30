use std::path::PathBuf;

use ur_config::AgentType;

/// Env var listing the skills to install (comma-separated).
pub const UR_WORKER_SKILLS_ENV: &str = "UR_WORKER_SKILLS";

/// Per-worker init inputs: what to install for this worker, as opposed to
/// [`WorkerPaths`](crate::WorkerPaths), which says where.
///
/// The container binary fills this from its environment with
/// [`WorkerInitInputs::from_env`]; a host-side runtime hosting many workers in
/// one process builds one per worker with a struct literal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkerInitInputs {
    /// Skill names to install from `potential-skills/`, in order.
    pub skills: Vec<String>,
    pub instructions: InstructionInputs,
}

/// Inputs for composing the agent's instruction file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstructionInputs {
    /// Strategy file name (without `.md`) under `instructions/`. `None` skips
    /// instruction setup entirely.
    pub strategy: Option<String>,
    /// Forge name (`github`, `gitea`); unknown or blank values add nothing.
    pub forge: Option<String>,
    /// Project instruction file to include, if any.
    pub project: Option<ProjectInstruction>,
}

/// A project instruction file and how to include it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectInstruction {
    /// File to read.
    pub path: PathBuf,
    /// Host workspace substituted for `%WORKSPACE%`; `None` leaves the
    /// placeholder as-is.
    pub host_workspace: Option<String>,
    pub mode: ProjectInstructionMode,
}

/// How project instructions reach the agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectInstructionMode {
    /// Write `PROJECT_<filename>` beside the instruction file and reference it
    /// with an `@<path>` import line.
    Import,
    /// Append the content directly to the instruction file, for agents with no
    /// `@` import support (AGY).
    Inline,
}

impl ProjectInstructionMode {
    /// The mode an agent's instruction file supports.
    pub fn for_agent(agent: AgentType) -> Self {
        match agent {
            AgentType::Agy => ProjectInstructionMode::Inline,
            AgentType::Claude | AgentType::Codex => ProjectInstructionMode::Import,
        }
    }
}

impl WorkerInitInputs {
    /// Container inputs from the process environment.
    pub fn from_env(agent: AgentType) -> Self {
        Self::from_lookup(agent, |key| std::env::var(key).ok())
    }

    /// Build inputs from an env-style lookup. Blank values count as unset,
    /// except the forge, which is passed through for `forge_instructions` to
    /// interpret.
    pub fn from_lookup(agent: AgentType, lookup: impl Fn(&str) -> Option<String>) -> Self {
        let non_blank = |key: &str| lookup(key).filter(|v| !v.trim().is_empty());

        let skills = non_blank(UR_WORKER_SKILLS_ENV)
            .map(|list| {
                list.split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        let project =
            non_blank(ur_config::UR_PROJECT_INSTRUCTION_ENV).map(|path| ProjectInstruction {
                path: PathBuf::from(path),
                host_workspace: non_blank(ur_config::UR_HOST_WORKSPACE_ENV),
                mode: ProjectInstructionMode::for_agent(agent),
            });

        WorkerInitInputs {
            skills,
            instructions: InstructionInputs {
                strategy: non_blank(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV),
                forge: lookup(ur_config::UR_WORKER_FORGE_ENV),
                project,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn empty_env_yields_defaults() {
        assert_eq!(
            WorkerInitInputs::from_lookup(AgentType::Claude, lookup(&[])),
            WorkerInitInputs::default()
        );
    }

    #[test]
    fn parses_container_env() {
        let inputs = WorkerInitInputs::from_lookup(
            AgentType::Claude,
            lookup(&[
                (UR_WORKER_SKILLS_ENV, " implement, ,ship "),
                (ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV, "code"),
                (ur_config::UR_WORKER_FORGE_ENV, "github"),
                (ur_config::UR_PROJECT_INSTRUCTION_ENV, "/p/CLAUDE.md"),
                (ur_config::UR_HOST_WORKSPACE_ENV, "/host/ws"),
            ]),
        );
        assert_eq!(
            inputs,
            WorkerInitInputs {
                skills: vec!["implement".into(), "ship".into()],
                instructions: InstructionInputs {
                    strategy: Some("code".into()),
                    forge: Some("github".into()),
                    project: Some(ProjectInstruction {
                        path: "/p/CLAUDE.md".into(),
                        host_workspace: Some("/host/ws".into()),
                        mode: ProjectInstructionMode::Import,
                    }),
                },
            }
        );
    }

    #[test]
    fn blank_values_are_unset() {
        let inputs = WorkerInitInputs::from_lookup(
            AgentType::Claude,
            lookup(&[
                (UR_WORKER_SKILLS_ENV, "  "),
                (ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV, " "),
                (ur_config::UR_PROJECT_INSTRUCTION_ENV, ""),
                (ur_config::UR_HOST_WORKSPACE_ENV, "/host/ws"),
            ]),
        );
        assert!(inputs.skills.is_empty());
        assert_eq!(inputs.instructions.strategy, None);
        assert_eq!(inputs.instructions.project, None);
    }

    #[test]
    fn blank_host_workspace_is_unset() {
        let inputs = WorkerInitInputs::from_lookup(
            AgentType::Claude,
            lookup(&[
                (ur_config::UR_PROJECT_INSTRUCTION_ENV, "/p/CLAUDE.md"),
                (ur_config::UR_HOST_WORKSPACE_ENV, "  "),
            ]),
        );
        assert_eq!(inputs.instructions.project.unwrap().host_workspace, None);
    }

    #[test]
    fn project_instruction_mode_follows_agent() {
        assert_eq!(
            ProjectInstructionMode::for_agent(AgentType::Claude),
            ProjectInstructionMode::Import
        );
        assert_eq!(
            ProjectInstructionMode::for_agent(AgentType::Codex),
            ProjectInstructionMode::Import
        );
        assert_eq!(
            ProjectInstructionMode::for_agent(AgentType::Agy),
            ProjectInstructionMode::Inline
        );
    }
}
