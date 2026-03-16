use std::collections::HashMap;
use std::fmt::Write;

use anyhow::{Context, Result};
use ur_rpc::error::StatusResultExt;
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
use ur_rpc::proto::ticket::*;

use crate::args::TicketArgs;
use crate::format::{format_ticket_detail, format_ticket_list};
use crate::status::build_status_report;

/// Execute a ticket subcommand against the given gRPC client.
///
/// This is a pure dispatch function with no state. The caller is responsible
/// for constructing the client (with any auth interceptors, channel config, etc).
///
/// Generic over the transport type `T` so callers can pass a plain `Channel`
/// or an `InterceptedService<Channel, F>` with auth headers.
pub async fn execute<T>(args: TicketArgs, client: &mut TicketServiceClient<T>) -> Result<()>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    match args {
        TicketArgs::Create {
            title,
            ticket_type,
            parent,
            priority,
            body,
        } => {
            let resp = client
                .create_ticket(CreateTicketRequest {
                    project: String::new(),
                    ticket_type,
                    status: "open".to_owned(),
                    priority,
                    parent_id: parent,
                    title,
                    body,
                    id: None,
                    created_at: None,
                })
                .await
                .with_status_context("create ticket")?;
            let id = resp.into_inner().id;
            println!("Created {id}");
        }

        TicketArgs::List {
            epic,
            ticket_type,
            status,
        } => {
            let resp = client
                .list_tickets(ListTicketsRequest {
                    project: None,
                    ticket_type,
                    status,
                    parent_id: epic,
                    meta_key: None,
                    meta_value: None,
                })
                .await
                .with_status_context("list tickets")?;
            let tickets = resp.into_inner().tickets;
            if tickets.is_empty() {
                println!("No tickets found.");
            } else {
                println!("{}", format_ticket_list(&tickets));
            }
        }

        TicketArgs::Show { id } => {
            let resp = client
                .get_ticket(GetTicketRequest { id: id.clone() })
                .await
                .with_status_context("get ticket")?;
            let inner = resp.into_inner();
            let t = inner
                .ticket
                .as_ref()
                .context("server returned empty ticket")?;
            println!(
                "{}",
                format_ticket_detail(t, &inner.metadata, &inner.activities)
            );
        }

        TicketArgs::Update {
            id,
            title,
            body,
            status,
            priority,
            ticket_type,
            parent: _,
            force,
        } => {
            client
                .update_ticket(UpdateTicketRequest {
                    id: id.clone(),
                    status,
                    priority,
                    title,
                    body,
                    force,
                    ticket_type,
                })
                .await
                .with_status_context("update ticket")?;
            println!("Updated {id}");
        }

        TicketArgs::SetMeta { id, key, value } => {
            client
                .set_meta(SetMetaRequest {
                    ticket_id: id.clone(),
                    key: key.clone(),
                    value: value.clone(),
                })
                .await
                .with_status_context("set metadata")?;
            println!("Set {key}={value} on {id}");
        }

        TicketArgs::DeleteMeta { id, key } => {
            client
                .delete_meta(DeleteMetaRequest {
                    ticket_id: id.clone(),
                    key: key.clone(),
                })
                .await
                .with_status_context("delete metadata")?;
            println!("Deleted {key} from {id}");
        }

        TicketArgs::AddActivity { id, message, meta } => {
            let metadata: HashMap<String, String> =
                meta.into_iter().map(|kv| (kv.key, kv.value)).collect();
            let resp = client
                .add_activity(AddActivityRequest {
                    ticket_id: id.clone(),
                    author: "cli".to_owned(),
                    message,
                    metadata,
                })
                .await
                .with_status_context("add activity")?;
            let activity_id = resp.into_inner().activity_id;
            println!("Added activity {activity_id} to {id}");
        }

        TicketArgs::ListActivities { id } => {
            let resp = client
                .list_activities(ListActivitiesRequest {
                    ticket_id: id.clone(),
                })
                .await
                .with_status_context("list activities")?;
            let activities = resp.into_inner().activities;
            if activities.is_empty() {
                println!("No activities found for {id}.");
            } else {
                println!("{}", format_activities(&activities));
            }
        }

        TicketArgs::AddBlock { id, blocked_by_id } => {
            client
                .add_block(AddBlockRequest {
                    blocker_id: blocked_by_id.clone(),
                    blocked_id: id.clone(),
                })
                .await
                .with_status_context("add block")?;
            println!("{blocked_by_id} now blocks {id}");
        }

        TicketArgs::RemoveBlock { id, blocked_by_id } => {
            client
                .remove_block(RemoveBlockRequest {
                    blocker_id: blocked_by_id.clone(),
                    blocked_id: id.clone(),
                })
                .await
                .with_status_context("remove block")?;
            println!("{blocked_by_id} no longer blocks {id}");
        }

        TicketArgs::AddLink { id, linked_id } => {
            client
                .add_link(AddLinkRequest {
                    left_id: id.clone(),
                    right_id: linked_id.clone(),
                })
                .await
                .with_status_context("link tickets")?;
            println!("Linked {id} <-> {linked_id}");
        }

        TicketArgs::RemoveLink { id, linked_id } => {
            client
                .remove_link(RemoveLinkRequest {
                    left_id: id.clone(),
                    right_id: linked_id.clone(),
                })
                .await
                .with_status_context("unlink tickets")?;
            println!("Unlinked {id} <-> {linked_id}");
        }

        TicketArgs::Dispatchable { epic_id } => {
            let resp = client
                .dispatchable_tickets(DispatchableTicketsRequest {
                    epic_id: epic_id.clone(),
                })
                .await
                .with_status_context("get dispatchable tickets")?;
            let tickets = resp.into_inner().tickets;
            if tickets.is_empty() {
                println!("No dispatchable tickets for {epic_id}.");
            } else {
                let mut out = String::new();
                writeln!(out, "{:<20} {:<4} TITLE", "ID", "PRI").unwrap();
                let separator: String = std::iter::repeat_n('-', 48).collect();
                writeln!(out, "{separator}").unwrap();
                for t in &tickets {
                    writeln!(out, "{:<20} {:<4} {}", t.id, t.priority, t.title).unwrap();
                }
                write!(out, "\n{} dispatchable ticket(s)", tickets.len()).unwrap();
                println!("{out}");
            }
        }

        TicketArgs::Status { project } => {
            let resp = client
                .list_tickets(ListTicketsRequest {
                    project: None,
                    ticket_type: None,
                    status: None,
                    parent_id: None,
                    meta_key: None,
                    meta_value: None,
                })
                .await
                .with_status_context("list tickets")?;
            let tickets = resp.into_inner().tickets;
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            println!(
                "{}",
                build_status_report(&tickets, &today, project.as_deref())
            );
        }
    }

    Ok(())
}

fn format_activities(activities: &[ActivityDetail]) -> String {
    let mut out = String::new();
    for a in activities {
        let Some(entry) = &a.entry else {
            continue;
        };
        write!(
            out,
            "[{}] {}: {}",
            entry.timestamp, entry.author, entry.message
        )
        .unwrap();
        for m in &a.metadata {
            write!(out, "\n  {}: {}", m.key, m.value).unwrap();
        }
        writeln!(out).unwrap();
    }
    if out.ends_with('\n') {
        out.pop();
    }
    out
}
