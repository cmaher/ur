use anyhow::{Context, Result};
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
