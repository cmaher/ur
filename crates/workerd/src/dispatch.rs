use std::collections::VecDeque;

use ur_config::AgentType;

use crate::skill_layout::SkillLayout;

/// Build the two-command dispatch buffer (context reset + skill invocation) for
/// a server-driven dispatch (`Implement` / `Design` / `AddressFeedbackTickets`).
///
/// The skill is qualified for `layout` (`implement` → `ur:implement` under the
/// `ur` plugin) so the agent runs the ur copy even when the user has a
/// same-named skill. Kept out of the gRPC handlers so the exact commands sent
/// to the agent are unit-testable without a tmux session.
pub fn dispatch_commands(
    agent: AgentType,
    layout: &SkillLayout,
    skill: &str,
    args: &[&str],
) -> VecDeque<String> {
    VecDeque::from(vec![
        agent.clear_command().to_string(),
        agent.skill_invocation(&layout.qualify(skill), args),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: SkillLayout = SkillLayout::AgentHome;

    fn cmds(agent: AgentType, layout: &SkillLayout, skill: &str, args: &[&str]) -> Vec<String> {
        Vec::from(dispatch_commands(agent, layout, skill, args))
    }

    #[test]
    fn claude_agent_home() {
        assert_eq!(
            cmds(AgentType::Claude, &HOME, "implement", &["ur-x"]),
            ["/clear", "/implement ur-x"]
        );
        assert_eq!(
            cmds(AgentType::Claude, &HOME, "design", &["ur-x"]),
            ["/clear", "/design ur-x"]
        );
        assert_eq!(
            cmds(
                AgentType::Claude,
                &HOME,
                "address-feedback",
                &["ur-x", "123"]
            ),
            ["/clear", "/address-feedback ur-x 123"]
        );
    }

    #[test]
    fn claude_plugin() {
        let plugin = SkillLayout::ur_plugin();
        assert_eq!(
            cmds(AgentType::Claude, &plugin, "implement", &["ur-x"]),
            ["/clear", "/ur:implement ur-x"]
        );
        assert_eq!(
            cmds(AgentType::Claude, &plugin, "design", &["ur-x"]),
            ["/clear", "/ur:design ur-x"]
        );
        assert_eq!(
            cmds(
                AgentType::Claude,
                &plugin,
                "address-feedback",
                &["ur-x", "123"]
            ),
            ["/clear", "/ur:address-feedback ur-x 123"]
        );
    }

    #[test]
    fn codex_agent_home() {
        assert_eq!(
            cmds(AgentType::Codex, &HOME, "implement", &["ur-x"]),
            [
                "/new",
                "Run the `implement` skill. DO NOT USE DESIGN SKILL. Arguments: ur-x"
            ]
        );
        assert_eq!(
            cmds(AgentType::Codex, &HOME, "design", &["ur-x"]),
            ["/new", "Run the `design` skill. Arguments: ur-x"]
        );
        assert_eq!(
            cmds(
                AgentType::Codex,
                &HOME,
                "address-feedback",
                &["ur-x", "123"]
            ),
            [
                "/new",
                "Run the `address-feedback` skill. Arguments: ur-x, 123"
            ]
        );
    }

    #[test]
    fn codex_plugin() {
        let plugin = SkillLayout::ur_plugin();
        assert_eq!(
            cmds(AgentType::Codex, &plugin, "implement", &["ur-x"]),
            [
                "/new",
                "Run the `ur:implement` skill. DO NOT USE DESIGN SKILL. Arguments: ur-x"
            ]
        );
        assert_eq!(
            cmds(AgentType::Codex, &plugin, "design", &["ur-x"]),
            ["/new", "Run the `ur:design` skill. Arguments: ur-x"]
        );
        assert_eq!(
            cmds(
                AgentType::Codex,
                &plugin,
                "address-feedback",
                &["ur-x", "123"]
            ),
            [
                "/new",
                "Run the `ur:address-feedback` skill. Arguments: ur-x, 123"
            ]
        );
    }

    #[test]
    fn agy_agent_home() {
        assert_eq!(
            cmds(AgentType::Agy, &HOME, "implement", &["ur-x"]),
            ["/clear", "/implement ur-x"]
        );
        assert_eq!(
            cmds(AgentType::Agy, &HOME, "design", &["ur-x"]),
            ["/clear", "/design ur-x"]
        );
        assert_eq!(
            cmds(AgentType::Agy, &HOME, "address-feedback", &["ur-x", "123"]),
            ["/clear", "/address-feedback ur-x 123"]
        );
    }

    #[test]
    fn agy_plugin() {
        let plugin = SkillLayout::ur_plugin();
        assert_eq!(
            cmds(AgentType::Agy, &plugin, "implement", &["ur-x"]),
            ["/clear", "/ur:implement ur-x"]
        );
        assert_eq!(
            cmds(AgentType::Agy, &plugin, "design", &["ur-x"]),
            ["/clear", "/ur:design ur-x"]
        );
        assert_eq!(
            cmds(
                AgentType::Agy,
                &plugin,
                "address-feedback",
                &["ur-x", "123"]
            ),
            ["/clear", "/ur:address-feedback ur-x 123"]
        );
    }
}
