use std::path::PathBuf;

use ur_config::AgentType;

/// Container workspace mount point.
const CONTAINER_WORKSPACE: &str = "/workspace";
/// Container mount point for host-side hook overlays (`~/.ur/projects/<key>/hooks`).
const CONTAINER_HOST_HOOKS: &str = "/var/ur/host-hooks";
/// Baked, agent-agnostic asset directory under the worker home.
const AGENT_SHARED_DIR: &str = ".agent-shared";
/// Hostexec shim directory under the worker home.
const SHIM_DIR: &str = ".local/bin";
/// Baked settings template filename, next to the agent's live settings file.
const POTENTIAL_SETTINGS_FILENAME: &str = "potential-settings.json";

/// Every filesystem location workerd reads from or writes to.
///
/// Injected into the gRPC service's collaborators and every init manager so
/// none of them hardcodes container paths. The container binary builds these
/// with [`WorkerPaths::container`]; a host-side (local) worker builds them
/// with a struct literal rooted at its drive/slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerPaths {
    /// Repository checkout the agent works in (in-repo `ur-hooks/` lives here).
    pub workspace: PathBuf,
    /// Destination for skill hooks (in-repo `ur-hooks/skills` + host overlay).
    pub skill_hooks: PathBuf,
    /// Destination directory for selected skills.
    pub skills_dest: PathBuf,
    /// Destination for the composed instruction file (e.g. `CLAUDE.md`).
    pub instructions_dest: PathBuf,
    /// Template copied into `settings_dest`. `None` disables settings init.
    pub settings_template: Option<PathBuf>,
    /// Destination for the agent's settings file. `None` for agents with no
    /// settings-file concept.
    pub settings_dest: Option<PathBuf>,
    /// Host-side hook overlay root (`git/`, `skills/`, `startup/`, `startup-bg/`).
    pub host_hooks: PathBuf,
    /// Baked asset root (`potential-skills/`, `instructions/`, `shared-instructions/`).
    pub agent_shared: PathBuf,
    /// Directory hostexec shims are written into.
    pub shim_dir: PathBuf,
}

impl WorkerPaths {
    /// The paths used inside an agent container for `agent`.
    ///
    /// Home-relative paths follow `$HOME` (falling back to the container's
    /// known home); `workspace` and `host_hooks` are fixed mount points; and
    /// `skill_hooks` is always under the container's known home in the agent's
    /// customization root. Skill sources reference hooks
    /// via `%UR_SKILL_HOOKS%`, rendered from `skill_hooks` at install time.
    pub fn container(agent: AgentType) -> Self {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(ur_config::WORKER_HOME));
        let customization_root = home.join(agent.customization_root());
        let settings_dir = home.join(agent.home_subdir());

        WorkerPaths {
            workspace: PathBuf::from(CONTAINER_WORKSPACE),
            skill_hooks: PathBuf::from(ur_config::WORKER_HOME)
                .join(agent.customization_root())
                .join(agent.skill_hooks_subdir()),
            skills_dest: customization_root.join(agent.skill_subdir()),
            instructions_dest: customization_root.join(agent.instruction_filename()),
            settings_template: agent
                .settings_filename()
                .map(|_| settings_dir.join(POTENTIAL_SETTINGS_FILENAME)),
            settings_dest: agent
                .settings_filename()
                .map(|filename| settings_dir.join(filename)),
            host_hooks: PathBuf::from(CONTAINER_HOST_HOOKS),
            agent_shared: home.join(AGENT_SHARED_DIR),
            shim_dir: home.join(SHIM_DIR),
        }
    }

    /// In-repo hook root (`<workspace>/ur-hooks`).
    pub fn in_repo_hooks(&self) -> PathBuf {
        self.workspace.join("ur-hooks")
    }

    /// Git hooks directory of the workspace checkout.
    pub fn workspace_git_hooks(&self) -> PathBuf {
        self.workspace.join(".git").join("hooks")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::container_paths_with_home;

    #[tokio::test]
    async fn container_paths_for_claude() {
        let paths = container_paths_with_home(AgentType::Claude, "/home/worker").await;
        assert_eq!(
            paths,
            WorkerPaths {
                workspace: "/workspace".into(),
                skill_hooks: "/home/worker/.claude/skill-hooks".into(),
                skills_dest: "/home/worker/.claude/skills".into(),
                instructions_dest: "/home/worker/.claude/CLAUDE.md".into(),
                settings_template: Some("/home/worker/.claude/potential-settings.json".into()),
                settings_dest: Some("/home/worker/.claude/settings.json".into()),
                host_hooks: "/var/ur/host-hooks".into(),
                agent_shared: "/home/worker/.agent-shared".into(),
                shim_dir: "/home/worker/.local/bin".into(),
            }
        );
        assert_eq!(paths.in_repo_hooks(), PathBuf::from("/workspace/ur-hooks"));
        assert_eq!(
            paths.workspace_git_hooks(),
            PathBuf::from("/workspace/.git/hooks")
        );
    }

    #[tokio::test]
    async fn container_paths_for_codex() {
        let paths = container_paths_with_home(AgentType::Codex, "/home/worker").await;
        assert_eq!(
            paths.skills_dest,
            PathBuf::from("/home/worker/.codex/skills")
        );
        assert_eq!(
            paths.skill_hooks,
            PathBuf::from("/home/worker/.codex/skill-hooks")
        );
    }

    #[tokio::test]
    async fn container_paths_for_agy() {
        let paths = container_paths_with_home(AgentType::Agy, "/home/worker").await;
        assert_eq!(
            paths.skills_dest,
            PathBuf::from("/home/worker/.gemini/config/skills")
        );
        assert_eq!(
            paths.skill_hooks,
            PathBuf::from("/home/worker/.gemini/config/skill-hooks")
        );
        assert_eq!(
            paths.instructions_dest,
            PathBuf::from("/home/worker/.gemini/config/AGENTS.md")
        );
        assert_eq!(
            paths.settings_dest,
            Some(PathBuf::from(
                "/home/worker/.gemini/antigravity-cli/settings.json"
            ))
        );
    }

    #[tokio::test]
    async fn container_skill_hooks_ignore_home_override() {
        let paths = container_paths_with_home(AgentType::Claude, "/tmp/elsewhere").await;
        assert_eq!(
            paths.skills_dest,
            PathBuf::from("/tmp/elsewhere/.claude/skills")
        );
        assert_eq!(
            paths.skill_hooks,
            PathBuf::from("/home/worker/.claude/skill-hooks")
        );
    }
}
