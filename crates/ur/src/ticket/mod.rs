pub mod args;
mod execute;
pub mod format;

pub use args::TicketArgs;
pub use execute::execute;
pub use format::{format_ticket_detail, format_ticket_list};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use tonic::transport::Channel;
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
use ur_rpc::proto::ticket::{
    ActivityDetail, ActivityEntry, DispatchableTicket, MetadataEntry, Ticket,
};

use crate::output::OutputManager;

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
    Approved {
        id: String,
        feedback_mode: String,
    },
    Dispatchable {
        epic_id: String,
        tickets: Vec<DispatchableTicket>,
    },
}

/// Format a `TicketOutput` as human-readable text.
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
        TicketOutput::Approved { id, feedback_mode } => {
            format!("Approved {id} (feedback_mode={feedback_mode})")
        }
        TicketOutput::Dispatchable { epic_id, tickets } => {
            if tickets.is_empty() {
                format!("No dispatchable tickets for {epic_id}.")
            } else {
                format::format_dispatchable(tickets)
            }
        }
    }
}

// --- Connection and project resolution (host CLI specific) ---

async fn connect_ticket(port: u16) -> Result<TicketServiceClient<Channel>> {
    let addr = format!("http://127.0.0.1:{port}");
    let retry_channel =
        ur_rpc::retry::RetryChannel::new(&addr, ur_rpc::retry::RetryConfig::default())
            .context("invalid server address")?;
    Ok(TicketServiceClient::new(retry_channel.channel().clone()))
}

/// Extract the project prefix from a ticket ID (format: `{project}-{hash}`).
fn project_from_ticket_id(ticket_id: &str) -> Option<String> {
    let dash = ticket_id.find('-')?;
    let project = &ticket_id[..dash];
    if project.is_empty() {
        None
    } else {
        Some(project.to_owned())
    }
}

/// Resolve the project key for commands that require it.
///
/// Resolution order: explicit `--project/-p` flag → `UR_PROJECT` env → current directory name.
/// Returns an error if none resolves.
fn resolve_project(explicit: Option<String>) -> Result<String> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    if let Ok(env_val) = std::env::var("UR_PROJECT")
        && !env_val.is_empty()
    {
        return Ok(env_val);
    }
    let cwd = std::env::current_dir().context("failed to get current working directory")?;
    let dir_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("cannot determine directory name from cwd"))?
        .to_owned();
    if dir_name.is_empty() {
        bail!("could not resolve project: no --project flag, UR_PROJECT env, or directory name");
    }
    Ok(dir_name)
}

/// Inject resolved project into ticket args that require it.
fn resolve_args_project(args: TicketArgs) -> Result<TicketArgs> {
    match args {
        TicketArgs::Create {
            title,
            project,
            ticket_type,
            parent,
            priority,
            body,
            wip,
        } => {
            let resolved = resolve_project(project)?;
            Ok(TicketArgs::Create {
                title,
                project: Some(resolved),
                ticket_type,
                parent,
                priority,
                body,
                wip,
            })
        }
        TicketArgs::List {
            project,
            all,
            tree,
            ticket_type,
            status,
            lifecycle,
        } => {
            let resolved = if all {
                None
            } else if project.is_none() && tree.is_some() {
                tree.as_deref().and_then(project_from_ticket_id)
            } else {
                Some(resolve_project(project)?)
            };
            Ok(TicketArgs::List {
                project: resolved,
                all,
                tree,
                ticket_type,
                status,
                lifecycle,
            })
        }
        TicketArgs::Dispatchable { epic_id, project } => {
            let resolved = if let Some(p) = project {
                p
            } else if let Some(p) = project_from_ticket_id(&epic_id) {
                p
            } else {
                resolve_project(None)?
            };
            Ok(TicketArgs::Dispatchable {
                epic_id,
                project: Some(resolved),
            })
        }
        other => Ok(other),
    }
}

pub async fn handle(port: u16, args: TicketArgs, output: &OutputManager) -> Result<()> {
    let args = resolve_args_project(args)?;
    let mut client = connect_ticket(port).await?;
    let result = execute(args, &mut client).await?;
    if output.is_json() {
        output.print_success(&result);
    } else {
        println!("{}", format_output(&result));
    }
    Ok(())
}

#[cfg(test)]
mod format_tests;
#[cfg(test)]
mod grpc_tests;

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::ticket::args::TicketCommand;

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
            "Design doc title",
            "-p",
            "myproj",
            "--type",
            "design",
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
                assert_eq!(title, "Design doc title");
                assert_eq!(project.as_deref(), Some("myproj"));
                assert_eq!(ticket_type, "design");
                assert_eq!(parent.as_deref(), Some("ur-abc12"));
                assert_eq!(priority, 3);
                assert_eq!(body, "Some body text");
                assert!(!wip);
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    #[test]
    fn test_create_rejects_epic_type() {
        let result = TicketCommand::try_parse_from(["ticket", "create", "Bad", "--type", "epic"]);
        assert!(result.is_err(), "epic type should be rejected");
    }

    #[test]
    fn test_create_rejects_bug_type() {
        let result = TicketCommand::try_parse_from(["ticket", "create", "Bad", "--type", "bug"]);
        assert!(result.is_err(), "bug type should be rejected");
    }

    #[test]
    fn test_list_no_filters() {
        let cmd = parse(&["ticket", "list"]);
        match cmd.command {
            super::TicketArgs::List {
                project,
                all,
                tree,
                ticket_type,
                status,
                lifecycle,
            } => {
                assert!(project.is_none());
                assert!(!all);
                assert!(tree.is_none());
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
            "ticket", "list", "-p", "myproj", "--tree", "ur-e1", "--type", "task", "--status",
            "open",
        ]);
        match cmd.command {
            super::TicketArgs::List {
                project,
                all,
                tree,
                ticket_type,
                status,
                lifecycle,
            } => {
                assert_eq!(project.as_deref(), Some("myproj"));
                assert!(!all);
                assert_eq!(tree.as_deref(), Some("ur-e1"));
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
                unparent,
                force,
                lifecycle,
                branch,
                no_branch,
                project,
            } => {
                assert_eq!(id, "ur-abc12");
                assert!(title.is_none());
                assert!(body.is_none());
                assert_eq!(status.as_deref(), Some("closed"));
                assert!(priority.is_none());
                assert!(ticket_type.is_none());
                assert!(parent.is_none());
                assert!(!unparent);
                assert!(!force);
                assert!(lifecycle.is_none());
                assert!(branch.is_none());
                assert!(!no_branch);
                assert!(project.is_none());
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
            super::TicketArgs::AddLink {
                id,
                linked_id,
                edge,
            } => {
                assert_eq!(id, "ur-abc12");
                assert_eq!(linked_id, "ur-def34");
                assert_eq!(edge, "relates_to");
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
    fn test_approve_feedback_now() {
        let cmd = parse(&["ticket", "approve", "ur-abc12", "--feedback-now"]);
        match cmd.command {
            super::TicketArgs::Approve {
                id,
                feedback_now,
                feedback_later,
            } => {
                assert_eq!(id, "ur-abc12");
                assert!(feedback_now);
                assert!(!feedback_later);
            }
            other => panic!("expected Approve, got {other:?}"),
        }
    }

    #[test]
    fn test_approve_feedback_later() {
        let cmd = parse(&["ticket", "approve", "ur-abc12", "--feedback-later"]);
        match cmd.command {
            super::TicketArgs::Approve {
                id,
                feedback_now,
                feedback_later,
            } => {
                assert_eq!(id, "ur-abc12");
                assert!(!feedback_now);
                assert!(feedback_later);
            }
            other => panic!("expected Approve, got {other:?}"),
        }
    }

    #[test]
    fn test_approve_default() {
        let cmd = parse(&["ticket", "approve", "ur-abc12"]);
        match cmd.command {
            super::TicketArgs::Approve {
                id,
                feedback_now,
                feedback_later,
            } => {
                assert_eq!(id, "ur-abc12");
                assert!(!feedback_now);
                assert!(!feedback_later);
            }
            other => panic!("expected Approve, got {other:?}"),
        }
    }

    #[test]
    fn test_list_with_lifecycle() {
        let cmd = parse(&["ticket", "list", "--lifecycle", "implementing"]);
        match cmd.command {
            super::TicketArgs::List { lifecycle, .. } => {
                assert_eq!(lifecycle.as_deref(), Some("implementing"));
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn test_create_wip() {
        let cmd = parse(&["ticket", "create", "WIP ticket", "--wip"]);
        match cmd.command {
            super::TicketArgs::Create { title, wip, .. } => {
                assert_eq!(title, "WIP ticket");
                assert!(wip);
            }
            other => panic!("expected Create, got {other:?}"),
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
