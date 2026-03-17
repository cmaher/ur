pub mod args;
mod execute;
pub mod format;
mod status;

pub use args::TicketArgs;
pub use execute::execute;
pub use format::{format_ticket_detail, format_ticket_list};

use serde::Serialize;
use ur_rpc::proto::ticket::{
    ActivityDetail, ActivityEntry, DispatchableTicket, MetadataEntry, Ticket,
};

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TicketOutput {
    Created {
        id: String,
    },
    Updated {
        id: String,
    },
    Listed {
        tickets: Vec<Ticket>,
    },
    Shown {
        ticket: Box<Ticket>,
        metadata: Vec<MetadataEntry>,
        activities: Vec<ActivityEntry>,
    },
    MetaSet {
        id: String,
        key: String,
        value: String,
    },
    MetaDeleted {
        id: String,
        key: String,
    },
    ActivityAdded {
        id: String,
        activity_id: String,
    },
    ActivitiesListed {
        id: String,
        activities: Vec<ActivityDetail>,
    },
    BlockAdded {
        id: String,
        blocked_by_id: String,
    },
    BlockRemoved {
        id: String,
        blocked_by_id: String,
    },
    LinkAdded {
        id: String,
        linked_id: String,
    },
    LinkRemoved {
        id: String,
        linked_id: String,
    },
    Dispatchable {
        epic_id: String,
        tickets: Vec<DispatchableTicket>,
    },
    StatusReport {
        report: String,
        tickets: Vec<Ticket>,
    },
}

/// Format a `TicketOutput` as human-readable text (same output as the pre-refactor CLI).
pub fn format_output(output: &TicketOutput) -> String {
    match output {
        TicketOutput::Created { id } => format!("Created {id}"),
        TicketOutput::Updated { id } => format!("Updated {id}"),
        TicketOutput::Listed { tickets } => {
            if tickets.is_empty() {
                "No tickets found.".to_string()
            } else {
                format_ticket_list(tickets)
            }
        }
        TicketOutput::Shown {
            ticket,
            metadata,
            activities,
        } => format_ticket_detail(ticket, metadata, activities),
        TicketOutput::MetaSet { id, key, value } => format!("Set {key}={value} on {id}"),
        TicketOutput::MetaDeleted { id, key } => format!("Deleted {key} from {id}"),
        TicketOutput::ActivityAdded { id, activity_id } => {
            format!("Added activity {activity_id} to {id}")
        }
        TicketOutput::ActivitiesListed { id, activities } => {
            if activities.is_empty() {
                format!("No activities found for {id}.")
            } else {
                format::format_activities(activities)
            }
        }
        TicketOutput::BlockAdded { id, blocked_by_id } => {
            format!("{blocked_by_id} now blocks {id}")
        }
        TicketOutput::BlockRemoved { id, blocked_by_id } => {
            format!("{blocked_by_id} no longer blocks {id}")
        }
        TicketOutput::LinkAdded { id, linked_id } => format!("Linked {id} <-> {linked_id}"),
        TicketOutput::LinkRemoved { id, linked_id } => format!("Unlinked {id} <-> {linked_id}"),
        TicketOutput::Dispatchable { epic_id, tickets } => {
            if tickets.is_empty() {
                format!("No dispatchable tickets for {epic_id}.")
            } else {
                format::format_dispatchable(tickets)
            }
        }
        TicketOutput::StatusReport { report, .. } => report.clone(),
    }
}

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
                project,
                ticket_type,
                parent,
                priority,
                body,
                wip,
            } => {
                assert_eq!(title, "My new ticket");
                assert!(project.is_none());
                assert_eq!(ticket_type, "task");
                assert!(parent.is_none());
                assert_eq!(priority, 0);
                assert_eq!(body, "");
                assert!(!wip);
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
            "-p",
            "myproj",
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
                project,
                ticket_type,
                parent,
                priority,
                body,
                wip,
            } => {
                assert_eq!(title, "Epic title");
                assert_eq!(project.as_deref(), Some("myproj"));
                assert_eq!(ticket_type, "epic");
                assert_eq!(parent.as_deref(), Some("ur-abc12"));
                assert_eq!(priority, 3);
                assert_eq!(body, "Some body text");
                assert!(!wip);
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    #[test]
    fn test_list_no_filters() {
        let cmd = parse(&["ticket", "list"]);
        match cmd.command {
            super::TicketArgs::List {
                project,
                all,
                epic,
                ticket_type,
                status,
                lifecycle,
            } => {
                assert!(project.is_none());
                assert!(!all);
                assert!(epic.is_none());
                assert!(ticket_type.is_none());
                assert!(status.is_none());
                assert!(lifecycle.is_none());
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn test_list_with_filters() {
        let cmd = parse(&[
            "ticket", "list", "-p", "myproj", "--epic", "ur-e1", "--type", "task", "--status",
            "open",
        ]);
        match cmd.command {
            super::TicketArgs::List {
                project,
                all,
                epic,
                ticket_type,
                status,
                lifecycle,
            } => {
                assert_eq!(project.as_deref(), Some("myproj"));
                assert!(!all);
                assert_eq!(epic.as_deref(), Some("ur-e1"));
                assert_eq!(ticket_type.as_deref(), Some("task"));
                assert_eq!(status.as_deref(), Some("open"));
                assert!(lifecycle.is_none());
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn test_list_all() {
        let cmd = parse(&["ticket", "list", "--all"]);
        match cmd.command {
            super::TicketArgs::List { all, .. } => {
                assert!(all);
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
                no_parent,
                force,
                lifecycle,
                branch,
                no_branch,
            } => {
                assert_eq!(id, "ur-abc12");
                assert!(title.is_none());
                assert!(body.is_none());
                assert_eq!(status.as_deref(), Some("closed"));
                assert!(priority.is_none());
                assert!(ticket_type.is_none());
                assert!(parent.is_none());
                assert!(!no_parent);
                assert!(!force);
                assert!(lifecycle.is_none());
                assert!(branch.is_none());
                assert!(!no_branch);
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
            super::TicketArgs::Dispatchable { epic_id, project } => {
                assert_eq!(epic_id, "ur-epic1");
                assert!(project.is_none());
            }
            other => panic!("expected Dispatchable, got {other:?}"),
        }
    }

    #[test]
    fn test_dispatchable_with_project() {
        let cmd = parse(&["ticket", "dispatchable", "ur-epic1", "-p", "myproj"]);
        match cmd.command {
            super::TicketArgs::Dispatchable { epic_id, project } => {
                assert_eq!(epic_id, "ur-epic1");
                assert_eq!(project.as_deref(), Some("myproj"));
            }
            other => panic!("expected Dispatchable, got {other:?}"),
        }
    }
}
