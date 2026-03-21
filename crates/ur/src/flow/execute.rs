use anyhow::{Context, Result, bail};
use tracing::info;
use ur_rpc::error::StatusResultExt;
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
use ur_rpc::proto::ticket::*;

use super::FlowOutput;
use super::args::FlowArgs;

/// Execute a flow subcommand against the given gRPC client.
pub async fn execute<T>(args: FlowArgs, client: &mut TicketServiceClient<T>) -> Result<FlowOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    match args {
        FlowArgs::Show { ticket_id } => execute_show(client, ticket_id).await,
        FlowArgs::List { status } => execute_list(client, status).await,
        FlowArgs::Cancel { ticket_id } => execute_cancel(client, ticket_id).await,
        FlowArgs::Noverify { ticket_id } => execute_noverify(client, ticket_id).await,
        FlowArgs::Redrive { id, to, advance } => execute_redrive(client, id, to, advance).await,
        FlowArgs::Autoapprove {
            ticket_id,
            feedback_now,
            feedback_later,
        } => execute_autoapprove(client, ticket_id, feedback_now, feedback_later).await,
    }
}

async fn execute_show<T>(
    client: &mut TicketServiceClient<T>,
    ticket_id: String,
) -> Result<FlowOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    let resp = client
        .get_workflow(GetWorkflowRequest {
            ticket_id: ticket_id.clone(),
        })
        .await
        .with_status_context("get workflow")?;
    let workflow = resp
        .into_inner()
        .workflow
        .context("server returned empty workflow")?;
    Ok(FlowOutput::Shown { workflow })
}

async fn execute_list<T>(
    client: &mut TicketServiceClient<T>,
    status: Option<String>,
) -> Result<FlowOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    let resp = client
        .list_workflows(ListWorkflowsRequest { status })
        .await
        .with_status_context("list workflows")?;
    let workflows = resp.into_inner().workflows;
    Ok(FlowOutput::Listed { workflows })
}

async fn execute_cancel<T>(
    client: &mut TicketServiceClient<T>,
    ticket_id: String,
) -> Result<FlowOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    client
        .cancel_workflow(CancelWorkflowRequest {
            ticket_id: ticket_id.clone(),
        })
        .await
        .with_status_context("cancel workflow")?;
    Ok(FlowOutput::Cancelled { ticket_id })
}

async fn execute_noverify<T>(
    client: &mut TicketServiceClient<T>,
    ticket_id: String,
) -> Result<FlowOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    info!(ticket_id = %ticket_id, "setting noverify meta");
    client
        .set_meta(SetMetaRequest {
            ticket_id: ticket_id.clone(),
            key: "noverify".to_owned(),
            value: "true".to_owned(),
        })
        .await
        .with_status_context("set noverify metadata")?;
    Ok(FlowOutput::NoverifySet { id: ticket_id })
}

async fn execute_redrive<T>(
    client: &mut TicketServiceClient<T>,
    id: String,
    to: Option<String>,
    advance: bool,
) -> Result<FlowOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    let to = if advance {
        bail!(
            "--continue is not yet supported after proto migration — use --to <status> explicitly"
        );
    } else {
        to.unwrap()
    };

    info!(id = %id, to = %to, "redriving ticket");

    let resp = client
        .redrive_ticket(RedriveTicketRequest {
            id: id.clone(),
            to_status: to,
        })
        .await
        .with_status_context("redrive ticket")?;

    let lifecycle_status = resp.into_inner().lifecycle_status;
    Ok(FlowOutput::Redriven {
        id,
        lifecycle_status,
    })
}

async fn execute_autoapprove<T>(
    client: &mut TicketServiceClient<T>,
    ticket_id: String,
    feedback_now: bool,
    feedback_later: bool,
) -> Result<FlowOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    if !feedback_now && !feedback_later {
        bail!("one of --feedback-now or --feedback-later is required");
    }

    let feedback_mode = if feedback_now {
        ur_rpc::feedback_mode::NOW
    } else {
        ur_rpc::feedback_mode::LATER
    }
    .to_owned();

    info!(ticket_id = %ticket_id, feedback_mode = %feedback_mode, "setting autoapprove");

    client
        .set_meta(SetMetaRequest {
            ticket_id: ticket_id.clone(),
            key: "feedback_mode".to_owned(),
            value: feedback_mode.clone(),
        })
        .await
        .with_status_context("set feedback_mode metadata")?;

    Ok(FlowOutput::AutoapproveSet {
        id: ticket_id,
        feedback_mode,
    })
}
