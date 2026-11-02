use std::path::{Path, PathBuf};

/// Plugin name ur skills are published under for host-side (local) workers.
pub const UR_PLUGIN_NAME: &str = "ur";

/// How selected ur skills are laid out on disk and how they are named when invoked.
///
/// Injected into [`InitSkillsManager`](crate::InitSkillsManager),
/// [`InitInstructionsManager`](crate::InitInstructionsManager) and
/// [`WorkerDaemonServiceImpl`](crate::WorkerDaemonServiceImpl) so install
/// layout, instructions and dispatch commands always agree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillLayout {
    /// Skills live directly in the agent's own skill directory
    /// (`<skills_dest>/<name>/`) and are invoked by bare name. Used in containers,
    /// where ur owns the agent home.
    AgentHome,
    /// Skills are packaged as a Claude Code plugin rooted at `skills_dest`
    /// (`.claude-plugin/plugin.json` + `skills/<name>/`) and invoked as
    /// `<name>:<skill>`, so they never collide with the user's own skills.
    NamespacedPlugin { name: String },
}

impl SkillLayout {
    /// The `ur` plugin layout used by host-side workers.
    pub fn ur_plugin() -> Self {
        SkillLayout::NamespacedPlugin {
            name: UR_PLUGIN_NAME.to_owned(),
        }
    }

    /// Plugin namespace, or `None` for [`SkillLayout::AgentHome`].
    pub fn namespace(&self) -> Option<&str> {
        match self {
            SkillLayout::AgentHome => None,
            SkillLayout::NamespacedPlugin { name } => Some(name),
        }
    }

    /// The name `skill` is invoked by under this layout (`implement` or `ur:implement`).
    pub fn qualify(&self, skill: &str) -> String {
        match self.namespace() {
            None => skill.to_owned(),
            Some(namespace) => format!("{namespace}:{skill}"),
        }
    }

    /// Directory that holds one subdirectory per installed skill.
    pub fn skills_root(&self, skills_dest: &Path) -> PathBuf {
        match self {
            SkillLayout::AgentHome => skills_dest.to_path_buf(),
            SkillLayout::NamespacedPlugin { .. } => skills_dest.join("skills"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_home_uses_bare_names_and_dest_root() {
        let layout = SkillLayout::AgentHome;
        assert_eq!(layout.namespace(), None);
        assert_eq!(layout.qualify("implement"), "implement");
        assert_eq!(
            layout.skills_root(Path::new("/h/.claude/skills")),
            PathBuf::from("/h/.claude/skills")
        );
    }

    #[test]
    fn ur_plugin_qualifies_and_nests_skills() {
        let layout = SkillLayout::ur_plugin();
        assert_eq!(layout.namespace(), Some("ur"));
        assert_eq!(layout.qualify("implement"), "ur:implement");
        assert_eq!(layout.qualify("brain:init"), "ur:brain:init");
        assert_eq!(
            layout.skills_root(Path::new("/d/.ur/plugin")),
            PathBuf::from("/d/.ur/plugin/skills")
        );
    }
}
