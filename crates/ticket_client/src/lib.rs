pub mod args;
mod execute;
pub mod format;
mod status;

pub use args::TicketArgs;
pub use execute::execute;
pub use format::{format_ticket_detail, format_ticket_list};

#[cfg(test)]
mod format_tests;
#[cfg(test)]
mod grpc_tests;

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::args::TicketCommand;

    fn parse(args: &[&str]) -> TicketCommand {
        TicketCommand::parse_from(args)
    }

    #[test]
    fn test_create_minimal() {
        let cmd = parse(&["ticket", "create", "My new ticket"]);
        match cmd.command {
            super::TicketArgs::Create {
                title,
                ticket_type,
                parent,
                priority,
                body,
            } => {
                assert_eq!(title, "My new ticket");
                assert_eq!(ticket_type, "task");
                assert!(parent.is_none());
                assert_eq!(priority, 0);
                assert_eq!(body, "");
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    #[test]
    fn test_create_full() {
        let cmd = parse(&[
            "ticket",
            "create",
            "Epic title",
            "--type",
            "epic",
            "--parent",
            "ur-abc12",
            "--priority",
            "3",
            "--body",
            "Some body text",
        ]);
        match cmd.command {
            super::TicketArgs::Create {
                title,
                ticket_type,
                parent,
                priority,
                body,
            } => {
                assert_eq!(title, "Epic title");
                assert_eq!(ticket_type, "epic");
                assert_eq!(parent.as_deref(), Some("ur-abc12"));
                assert_eq!(priority, 3);
                assert_eq!(body, "Some body text");
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    #[test]
    fn test_list_no_filters() {
        let cmd = parse(&["ticket", "list"]);
        match cmd.command {
            super::TicketArgs::List {
                epic,
                ticket_type,
                status,
            } => {
                assert!(epic.is_none());
                assert!(ticket_type.is_none());
                assert!(status.is_none());
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn test_list_with_filters() {
        let cmd = parse(&[
            "ticket", "list", "--epic", "ur-e1", "--type", "task", "--status", "open",
        ]);
        match cmd.command {
            super::TicketArgs::List {
                epic,
                ticket_type,
                status,
            } => {
                assert_eq!(epic.as_deref(), Some("ur-e1"));
                assert_eq!(ticket_type.as_deref(), Some("task"));
                assert_eq!(status.as_deref(), Some("open"));
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn test_show() {
        let cmd = parse(&["ticket", "show", "ur-abc12"]);
        match cmd.command {
            super::TicketArgs::Show { id } => assert_eq!(id, "ur-abc12"),
            other => panic!("expected Show, got {other:?}"),
        }
    }

    #[test]
    fn test_update_partial() {
        let cmd = parse(&["ticket", "update", "ur-abc12", "--status", "closed"]);
        match cmd.command {
            super::TicketArgs::Update {
                id,
                title,
                body,
                status,
                priority,
                ticket_type,
                parent,
                force,
            } => {
                assert_eq!(id, "ur-abc12");
                assert!(title.is_none());
                assert!(body.is_none());
                assert_eq!(status.as_deref(), Some("closed"));
                assert!(priority.is_none());
                assert!(ticket_type.is_none());
                assert!(parent.is_none());
                assert!(!force);
            }
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[test]
    fn test_set_meta() {
        let cmd = parse(&["ticket", "set-meta", "ur-abc12", "env", "prod"]);
        match cmd.command {
            super::TicketArgs::SetMeta { id, key, value } => {
                assert_eq!(id, "ur-abc12");
                assert_eq!(key, "env");
                assert_eq!(value, "prod");
            }
            other => panic!("expected SetMeta, got {other:?}"),
        }
    }

    #[test]
    fn test_delete_meta() {
        let cmd = parse(&["ticket", "delete-meta", "ur-abc12", "env"]);
        match cmd.command {
            super::TicketArgs::DeleteMeta { id, key } => {
                assert_eq!(id, "ur-abc12");
                assert_eq!(key, "env");
            }
            other => panic!("expected DeleteMeta, got {other:?}"),
        }
    }

    #[test]
    fn test_add_activity_no_meta() {
        let cmd = parse(&["ticket", "add-activity", "ur-abc12", "did some work"]);
        match cmd.command {
            super::TicketArgs::AddActivity { id, message, meta } => {
                assert_eq!(id, "ur-abc12");
                assert_eq!(message, "did some work");
                assert!(meta.is_empty());
            }
            other => panic!("expected AddActivity, got {other:?}"),
        }
    }

    #[test]
    fn test_add_activity_with_meta() {
        let cmd = parse(&[
            "ticket",
            "add-activity",
            "ur-abc12",
            "deployed",
            "--meta",
            "env=prod",
            "--meta",
            "version=1.2",
        ]);
        match cmd.command {
            super::TicketArgs::AddActivity { id, message, meta } => {
                assert_eq!(id, "ur-abc12");
                assert_eq!(message, "deployed");
                assert_eq!(meta.len(), 2);
                assert_eq!(meta[0].key, "env");
                assert_eq!(meta[0].value, "prod");
                assert_eq!(meta[1].key, "version");
                assert_eq!(meta[1].value, "1.2");
            }
            other => panic!("expected AddActivity, got {other:?}"),
        }
    }

    #[test]
    fn test_list_activities() {
        let cmd = parse(&["ticket", "list-activities", "ur-abc12"]);
        match cmd.command {
            super::TicketArgs::ListActivities { id } => assert_eq!(id, "ur-abc12"),
            other => panic!("expected ListActivities, got {other:?}"),
        }
    }

    #[test]
    fn test_add_block() {
        let cmd = parse(&["ticket", "add-block", "ur-abc12", "ur-def34"]);
        match cmd.command {
            super::TicketArgs::AddBlock { id, blocked_by_id } => {
                assert_eq!(id, "ur-abc12");
                assert_eq!(blocked_by_id, "ur-def34");
            }
            other => panic!("expected AddBlock, got {other:?}"),
        }
    }

    #[test]
    fn test_remove_block() {
        let cmd = parse(&["ticket", "remove-block", "ur-abc12", "ur-def34"]);
        match cmd.command {
            super::TicketArgs::RemoveBlock { id, blocked_by_id } => {
                assert_eq!(id, "ur-abc12");
                assert_eq!(blocked_by_id, "ur-def34");
            }
            other => panic!("expected RemoveBlock, got {other:?}"),
        }
    }

    #[test]
    fn test_add_link() {
        let cmd = parse(&["ticket", "add-link", "ur-abc12", "ur-def34"]);
        match cmd.command {
            super::TicketArgs::AddLink { id, linked_id } => {
                assert_eq!(id, "ur-abc12");
                assert_eq!(linked_id, "ur-def34");
            }
            other => panic!("expected AddLink, got {other:?}"),
        }
    }

    #[test]
    fn test_remove_link() {
        let cmd = parse(&["ticket", "remove-link", "ur-abc12", "ur-def34"]);
        match cmd.command {
            super::TicketArgs::RemoveLink { id, linked_id } => {
                assert_eq!(id, "ur-abc12");
                assert_eq!(linked_id, "ur-def34");
            }
            other => panic!("expected RemoveLink, got {other:?}"),
        }
    }

    #[test]
    fn test_dispatchable() {
        let cmd = parse(&["ticket", "dispatchable", "ur-epic1"]);
        match cmd.command {
            super::TicketArgs::Dispatchable { epic_id } => assert_eq!(epic_id, "ur-epic1"),
            other => panic!("expected Dispatchable, got {other:?}"),
        }
    }
}
