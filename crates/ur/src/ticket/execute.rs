use std::collections::HashMap;

use anyhow::{Context, Result};
use ur_rpc::error::StatusResultExt;
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
use ur_rpc::proto::ticket::*;

use ur_db::model::TicketType;
use ur_rpc::lifecycle;

use super::TicketOutput;
use super::args::{KeyValue, TicketArgs};
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
        TicketArgs::Create { .. } => dispatch_create(args, client).await,
        TicketArgs::Update { .. } => dispatch_update(args, client).await,
        TicketArgs::List {
            project,
            all,
            tree,
            ticket_type,
            status,
        } => execute_list(client, project, all, tree, ticket_type, status).await,
        TicketArgs::Show {
            id,
            activity_author,
        } => execute_show(client, id, activity_author).await,
        TicketArgs::SetMeta { id, key, value } => execute_set_meta(client, id, key, value).await,
        TicketArgs::DeleteMeta { id, key } => execute_delete_meta(client, id, key).await,
        TicketArgs::AddActivity { id, message, meta } => {
            execute_add_activity(client, id, message, meta).await
        }
        TicketArgs::ListActivities { id } => execute_list_activities(client, id).await,
        TicketArgs::AddBlock { id, blocked_by_id } => {
            execute_add_block(client, id, blocked_by_id).await
        }
        TicketArgs::RemoveBlock { id, blocked_by_id } => {
            execute_remove_block(client, id, blocked_by_id).await
        }
        TicketArgs::AddLink {
            id,
            linked_id,
            edge,
        } => execute_add_link(client, id, linked_id, edge).await,
        TicketArgs::RemoveLink { id, linked_id } => {
            execute_remove_link(client, id, linked_id).await
        }
        TicketArgs::Approve {
            id,
            feedback_now,
            feedback_later,
        } => execute_approve(client, id, feedback_now, feedback_later).await,
        TicketArgs::Close { id, force } => execute_close(client, id, force).await,
        TicketArgs::Open { id } => execute_open(client, id).await,
        TicketArgs::Dispatchable { epic_id, project } => {
            execute_dispatchable(client, epic_id, project).await
        }
    }
}

/// Destructure and forward `Create` — extracted to keep `execute` under the line limit.
async fn dispatch_create<T>(
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
    let TicketArgs::Create {
        title,
        project,
        ticket_type,
        parent,
        priority,
        body,
        wip,
    } = args
    else {
        unreachable!()
    };
    execute_create(
        client,
        title,
        project,
        ticket_type,
        parent,
        priority,
        body,
        wip,
    )
    .await
}

/// Destructure and forward `Update` — extracted to keep `execute` under the line limit.
async fn dispatch_update<T>(
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
    let TicketArgs::Update {
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
    } = args
    else {
        unreachable!()
    };
    execute_update(
        client,
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
    )
    .await
}

async fn execute_list<T>(
    client: &mut TicketServiceClient<T>,
    project: Option<String>,
    all: bool,
    tree: Option<String>,
    ticket_type: Option<String>,
    status: Option<String>,
) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    let project_filter = if all { None } else { project };
    let resp = client
        .list_tickets(ListTicketsRequest {
            project: project_filter,
            ticket_type: ticket_type.map(|t| TicketType::normalize(&t)),
            status,
            meta_key: None,
            meta_value: None,
            tree_root_id: tree,
            page_size: None,
            offset: None,
            include_children: None,
            parent_id: None,
        })
        .await
        .with_status_context("list tickets")?;
    let tickets = resp.into_inner().tickets;
    Ok(TicketOutput::Listed { tickets })
}

async fn execute_show<T>(
    client: &mut TicketServiceClient<T>,
    id: String,
    activity_author: Option<String>,
) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    let resp = client
        .get_ticket(GetTicketRequest {
            id: id.clone(),
            activity_author_filter: activity_author,
        })
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

async fn execute_set_meta<T>(
    client: &mut TicketServiceClient<T>,
    id: String,
    key: String,
    value: String,
) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
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

async fn execute_delete_meta<T>(
    client: &mut TicketServiceClient<T>,
    id: String,
    key: String,
) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    client
        .delete_meta(DeleteMetaRequest {
            ticket_id: id.clone(),
            key: key.clone(),
        })
        .await
        .with_status_context("delete metadata")?;
    Ok(TicketOutput::MetaDeleted { id, key })
}

async fn execute_add_activity<T>(
    client: &mut TicketServiceClient<T>,
    id: String,
    message: String,
    meta: Vec<KeyValue>,
) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    let metadata: HashMap<String, String> = meta.into_iter().map(|kv| (kv.key, kv.value)).collect();
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

async fn execute_list_activities<T>(
    client: &mut TicketServiceClient<T>,
    id: String,
) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    let resp = client
        .list_activities(ListActivitiesRequest {
            ticket_id: id.clone(),
        })
        .await
        .with_status_context("list activities")?;
    let activities = resp.into_inner().activities;
    Ok(TicketOutput::ActivitiesListed { id, activities })
}

async fn execute_add_block<T>(
    client: &mut TicketServiceClient<T>,
    id: String,
    blocked_by_id: String,
) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    client
        .add_block(AddBlockRequest {
            blocker_id: blocked_by_id.clone(),
            blocked_id: id.clone(),
        })
        .await
        .with_status_context("add block")?;
    Ok(TicketOutput::BlockAdded { id, blocked_by_id })
}

async fn execute_remove_block<T>(
    client: &mut TicketServiceClient<T>,
    id: String,
    blocked_by_id: String,
) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    client
        .remove_block(RemoveBlockRequest {
            blocker_id: blocked_by_id.clone(),
            blocked_id: id.clone(),
        })
        .await
        .with_status_context("remove block")?;
    Ok(TicketOutput::BlockRemoved { id, blocked_by_id })
}

async fn execute_add_link<T>(
    client: &mut TicketServiceClient<T>,
    id: String,
    linked_id: String,
    edge: String,
) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    client
        .add_link(AddLinkRequest {
            left_id: id.clone(),
            right_id: linked_id.clone(),
            edge_kind: Some(edge),
        })
        .await
        .with_status_context("link tickets")?;
    Ok(TicketOutput::LinkAdded { id, linked_id })
}

async fn execute_remove_link<T>(
    client: &mut TicketServiceClient<T>,
    id: String,
    linked_id: String,
) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    client
        .remove_link(RemoveLinkRequest {
            left_id: id.clone(),
            right_id: linked_id.clone(),
        })
        .await
        .with_status_context("unlink tickets")?;
    Ok(TicketOutput::LinkRemoved { id, linked_id })
}

async fn execute_dispatchable<T>(
    client: &mut TicketServiceClient<T>,
    epic_id: String,
    project: Option<String>,
) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
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

#[allow(clippy::too_many_arguments)]
async fn execute_create<T>(
    client: &mut TicketServiceClient<T>,
    title: String,
    project: Option<String>,
    ticket_type: String,
    parent: Option<String>,
    priority: i64,
    body: String,
    wip: bool,
) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    let resp = client
        .create_ticket(CreateTicketRequest {
            project: project.unwrap_or_default(),
            ticket_type: TicketType::normalize(&ticket_type),
            status: lifecycle::OPEN.to_owned(),
            priority,
            parent_id: parent,
            title,
            body,
            id: None,
            created_at: None,
            wip,
            branch: None,
        })
        .await
        .with_status_context("create ticket")?;
    let id = resp.into_inner().id;

    Ok(TicketOutput::Created { id })
}

#[allow(clippy::too_many_arguments)]
async fn execute_update<T>(
    client: &mut TicketServiceClient<T>,
    id: String,
    title: Option<String>,
    body: Option<String>,
    status: Option<String>,
    priority: Option<i64>,
    ticket_type: Option<String>,
    parent: Option<String>,
    unparent: bool,
    force: bool,
    lifecycle: Option<String>,
    branch: Option<String>,
    no_branch: bool,
    project: Option<String>,
) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    let parent_id = if unparent {
        Some("NONE".to_owned())
    } else {
        parent
    };
    let branch_value = if no_branch {
        Some("NONE".to_owned())
    } else {
        branch
    };
    let _ = lifecycle; // lifecycle_status removed from proto; reserved for future use
    client
        .update_ticket(UpdateTicketRequest {
            id: id.clone(),
            status,
            priority,
            title,
            body,
            force,
            ticket_type: ticket_type.map(|t| TicketType::normalize(&t)),
            parent_id,
            branch: branch_value,
            project,
        })
        .await
        .with_status_context("update ticket")?;
    Ok(TicketOutput::Updated { id })
}

async fn execute_approve<T>(
    client: &mut TicketServiceClient<T>,
    id: String,
    feedback_now: bool,
    feedback_later: bool,
) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    let feedback_mode = if feedback_now {
        "now"
    } else if feedback_later {
        "later"
    } else {
        "now"
    }
    .to_owned();

    client
        .set_meta(SetMetaRequest {
            ticket_id: id.clone(),
            key: ur_rpc::ticket_meta::FEEDBACK_MODE.to_owned(),
            value: feedback_mode.clone(),
        })
        .await
        .with_status_context("set feedback_mode metadata")?;

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
            branch: None,
            project: None,
        })
        .await
        .with_status_context("transition lifecycle to addressing_feedback")?;

    Ok(TicketOutput::Approved { id, feedback_mode })
}

async fn execute_close<T>(
    client: &mut TicketServiceClient<T>,
    id: String,
    force: bool,
) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
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
            branch: None,
            project: None,
        })
        .await
        .with_status_context("close ticket")?;
    Ok(TicketOutput::Updated { id })
}

async fn execute_open<T>(client: &mut TicketServiceClient<T>, id: String) -> Result<TicketOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
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
            branch: None,
            project: None,
        })
        .await
        .with_status_context("open ticket")?;
    Ok(TicketOutput::Updated { id })
}
