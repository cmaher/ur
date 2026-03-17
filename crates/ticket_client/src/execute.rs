use std::collections::HashMap;

use anyhow::{Context, Result};
use ur_rpc::error::StatusResultExt;
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
use ur_rpc::proto::ticket::*;

use crate::TicketOutput;
use crate::args::TicketArgs;
use crate::status::build_status_report;

/// Execute a ticket subcommand against the given gRPC client.
///
/// Returns a `TicketOutput` variant describing the result. The caller is
/// responsible for formatting (JSON or text) and printing.
///
/// Generic over the transport type `T` so callers can pass a plain `Channel`
/// or an `InterceptedService<Channel, F>` with auth headers.
pub async fn execute<T>(
    args: TicketArgs,
    client: &mut TicketServiceClient<T>,
) -> Result<TicketOutput>
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
            project,
            ticket_type,
            parent,
            priority,
            body,
            wip,
        } => {
            let resp = client
                .create_ticket(CreateTicketRequest {
                    project: project.unwrap_or_default(),
                    ticket_type,
                    status: "open".to_owned(),
                    priority,
                    parent_id: parent,
                    title,
                    body,
                    id: None,
                    created_at: None,
                    wip,
                })
                .await
                .with_status_context("create ticket")?;
            let id = resp.into_inner().id;
            Ok(TicketOutput::Created { id })
        }

        TicketArgs::List {
            project,
            all,
            epic,
            ticket_type,
            status,
            lifecycle,
        } => {
            let project_filter = if all { None } else { project };
            let resp = client
                .list_tickets(ListTicketsRequest {
                    project: project_filter,
                    ticket_type,
                    status,
                    parent_id: epic,
                    meta_key: None,
                    meta_value: None,
                    lifecycle_status: lifecycle,
                })
                .await
                .with_status_context("list tickets")?;
            let tickets = resp.into_inner().tickets;
            Ok(TicketOutput::Listed { tickets })
        }

        TicketArgs::Show { id } => {
            let resp = client
                .get_ticket(GetTicketRequest { id: id.clone() })
                .await
                .with_status_context("get ticket")?;
            let inner = resp.into_inner();
            let t = inner.ticket.context("server returned empty ticket")?;
            Ok(TicketOutput::Shown {
                ticket: Box::new(t),
                metadata: inner.metadata,
                activities: inner.activities,
            })
        }

        TicketArgs::Update {
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
            let parent_id = if no_parent {
                Some("NONE".to_owned())
            } else {
                parent
            };
            let branch_value = if no_branch {
                Some("NONE".to_owned())
            } else {
                branch
            };
            client
                .update_ticket(UpdateTicketRequest {
                    id: id.clone(),
                    status,
                    priority,
                    title,
                    body,
                    force,
                    ticket_type,
                    parent_id,
                    lifecycle_status: lifecycle,
                    branch: branch_value,
                })
                .await
                .with_status_context("update ticket")?;
            Ok(TicketOutput::Updated { id })
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
            Ok(TicketOutput::MetaSet { id, key, value })
        }

        TicketArgs::DeleteMeta { id, key } => {
            client
                .delete_meta(DeleteMetaRequest {
                    ticket_id: id.clone(),
                    key: key.clone(),
                })
                .await
                .with_status_context("delete metadata")?;
            Ok(TicketOutput::MetaDeleted { id, key })
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
            Ok(TicketOutput::ActivityAdded { id, activity_id })
        }

        TicketArgs::ListActivities { id } => {
            let resp = client
                .list_activities(ListActivitiesRequest {
                    ticket_id: id.clone(),
                })
                .await
                .with_status_context("list activities")?;
            let activities = resp.into_inner().activities;
            Ok(TicketOutput::ActivitiesListed { id, activities })
        }

        TicketArgs::AddBlock { id, blocked_by_id } => {
            client
                .add_block(AddBlockRequest {
                    blocker_id: blocked_by_id.clone(),
                    blocked_id: id.clone(),
                })
                .await
                .with_status_context("add block")?;
            Ok(TicketOutput::BlockAdded { id, blocked_by_id })
        }

        TicketArgs::RemoveBlock { id, blocked_by_id } => {
            client
                .remove_block(RemoveBlockRequest {
                    blocker_id: blocked_by_id.clone(),
                    blocked_id: id.clone(),
                })
                .await
                .with_status_context("remove block")?;
            Ok(TicketOutput::BlockRemoved { id, blocked_by_id })
        }

        TicketArgs::AddLink { id, linked_id } => {
            client
                .add_link(AddLinkRequest {
                    left_id: id.clone(),
                    right_id: linked_id.clone(),
                })
                .await
                .with_status_context("link tickets")?;
            Ok(TicketOutput::LinkAdded { id, linked_id })
        }

        TicketArgs::RemoveLink { id, linked_id } => {
            client
                .remove_link(RemoveLinkRequest {
                    left_id: id.clone(),
                    right_id: linked_id.clone(),
                })
                .await
                .with_status_context("unlink tickets")?;
            Ok(TicketOutput::LinkRemoved { id, linked_id })
        }

        TicketArgs::Approve {
            id,
            feedback_now,
            feedback_later,
        } => {
            let feedback_mode = if feedback_now {
                "now"
            } else if feedback_later {
                "later"
            } else {
                "now" // default to now
            }
            .to_owned();

            // Set feedback_mode metadata
            client
                .set_meta(SetMetaRequest {
                    ticket_id: id.clone(),
                    key: "feedback_mode".to_owned(),
                    value: feedback_mode.clone(),
                })
                .await
                .with_status_context("set feedback_mode metadata")?;

            // Transition lifecycle from in_review to feedback_creating
            client
                .update_ticket(UpdateTicketRequest {
                    id: id.clone(),
                    status: None,
                    priority: None,
                    title: None,
                    body: None,
                    force: false,
                    ticket_type: None,
                    parent_id: None,
                    lifecycle_status: Some("feedback_creating".to_owned()),
                    branch: None,
                })
                .await
                .with_status_context("transition lifecycle to feedback_creating")?;

            Ok(TicketOutput::Approved { id, feedback_mode })
        }

        TicketArgs::Close { id, force } => {
            client
                .update_ticket(UpdateTicketRequest {
                    id: id.clone(),
                    status: Some("closed".to_owned()),
                    priority: None,
                    title: None,
                    body: None,
                    force,
                    ticket_type: None,
                    parent_id: None,
                    lifecycle_status: None,
                    branch: None,
                })
                .await
                .with_status_context("close ticket")?;
            Ok(TicketOutput::Updated { id })
        }

        TicketArgs::Open { id } => {
            client
                .update_ticket(UpdateTicketRequest {
                    id: id.clone(),
                    status: Some("open".to_owned()),
                    priority: None,
                    title: None,
                    body: None,
                    force: false,
                    ticket_type: None,
                    parent_id: None,
                    lifecycle_status: None,
                    branch: None,
                })
                .await
                .with_status_context("open ticket")?;
            Ok(TicketOutput::Updated { id })
        }

        TicketArgs::Dispatchable { epic_id, project } => {
            let resp = client
                .dispatchable_tickets(DispatchableTicketsRequest {
                    epic_id: epic_id.clone(),
                    project,
                })
                .await
                .with_status_context("get dispatchable tickets")?;
            let tickets = resp.into_inner().tickets;
            Ok(TicketOutput::Dispatchable { epic_id, tickets })
        }

        TicketArgs::Status { project } => {
            let resp = client
                .list_tickets(ListTicketsRequest {
                    project: project.clone(),
                    ticket_type: None,
                    status: None,
                    parent_id: None,
                    meta_key: None,
                    meta_value: None,
                    lifecycle_status: None,
                })
                .await
                .with_status_context("list tickets")?;
            let tickets = resp.into_inner().tickets;
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            let report = build_status_report(&tickets, &today, project.as_deref());
            Ok(TicketOutput::StatusReport { report, tickets })
        }
    }
}
