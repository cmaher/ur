use std::collections::VecDeque;

use ur_config::AgentType;

/// Build the two-command dispatch buffer (context reset + skill invocation) for
/// a server-driven dispatch (`Implement` / `Design` / `AddressFeedbackTickets`).
///
/// Kept out of the gRPC handlers so the exact commands sent to the agent are
/// unit-testable without a tmux session.
pub fn dispatch_commands(agent: AgentType, skill: &str, args: &[&str]) -> VecDeque<String> {
    VecDeque::from(vec![
        agent.clear_command().to_string(),
        agent.skill_invocation(skill, args),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implement_commands_claude() {
        let commands = dispatch_commands(AgentType::Claude, "implement", &["ur-x"]);
        assert_eq!(
            Vec::from(commands),
            vec!["/clear".to_string(), "/implement ur-x".to_string()]
        );
    }

    #[test]
    fn implement_commands_codex() {
        let commands = dispatch_commands(AgentType::Codex, "implement", &["ur-x"]);
        assert_eq!(
            Vec::from(commands),
            vec![
                "/new".to_string(),
                "Run the `implement` skill. DO NOT USE DESIGN SKILL. Arguments: ur-x".to_string()
            ]
        );
    }

    #[test]
    fn design_commands_claude() {
        let commands = dispatch_commands(AgentType::Claude, "design", &["ur-x"]);
        assert_eq!(
            Vec::from(commands),
            vec!["/clear".to_string(), "/design ur-x".to_string()]
        );
    }

    #[test]
    fn design_commands_codex() {
        let commands = dispatch_commands(AgentType::Codex, "design", &["ur-x"]);
        assert_eq!(
            Vec::from(commands),
            vec![
                "/new".to_string(),
                "Run the `design` skill. Arguments: ur-x".to_string()
            ]
        );
    }

    #[test]
    fn address_feedback_commands_claude() {
        let commands = dispatch_commands(AgentType::Claude, "address-feedback", &["ur-x", "123"]);
        assert_eq!(
            Vec::from(commands),
            vec![
                "/clear".to_string(),
                "/address-feedback ur-x 123".to_string()
            ]
        );
    }

    #[test]
    fn address_feedback_commands_codex() {
        let commands = dispatch_commands(AgentType::Codex, "address-feedback", &["ur-x", "123"]);
        assert_eq!(
            Vec::from(commands),
            vec![
                "/new".to_string(),
                "Run the `address-feedback` skill. Arguments: ur-x, 123".to_string()
            ]
        );
    }

    #[test]
    fn implement_commands_agy() {
        let commands = dispatch_commands(AgentType::Agy, "implement", &["ur-x"]);
        assert_eq!(
            Vec::from(commands),
            vec!["/clear".to_string(), "/implement ur-x".to_string()]
        );
    }

    #[test]
    fn design_commands_agy() {
        let commands = dispatch_commands(AgentType::Agy, "design", &["ur-x"]);
        assert_eq!(
            Vec::from(commands),
            vec!["/clear".to_string(), "/design ur-x".to_string()]
        );
    }

    #[test]
    fn address_feedback_commands_agy() {
        let commands = dispatch_commands(AgentType::Agy, "address-feedback", &["ur-x", "123"]);
        assert_eq!(
            Vec::from(commands),
            vec![
                "/clear".to_string(),
                "/address-feedback ur-x 123".to_string()
            ]
        );
    }
}
