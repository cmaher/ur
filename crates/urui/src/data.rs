use std::collections::HashMap;

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{debug, error};

use ur_config::ProjectConfig;
use ur_rpc::connection::connect;
use ur_rpc::proto::core::WorkerLaunchRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
use ur_rpc::proto::ticket::{
    CreateWorkflowRequest, ListTicketsRequest, ListTicketsResponse, ListWorkflowsRequest,
    ListWorkflowsResponse, Ticket, WorkflowInfo,
};

use crate::event::AppEvent;

/// Payload delivered by gRPC data-fetch tasks.
#[derive(Debug, Clone)]
pub enum DataPayload {
    /// Ticket list fetched from the server.
    Tickets(Result<Vec<Ticket>, String>),
    /// Workflow list fetched from the server.
    Flows(Result<Vec<WorkflowInfo>, String>),
}

/// Result of an async action (dispatch, etc.) sent back via `AppEvent::ActionResult`.
#[derive(Debug, Clone)]
pub struct ActionResult {
    /// Human-readable success message, or error message.
    pub result: Result<String, String>,
}

/// Async data-fetching manager that spawns tokio tasks for gRPC calls.
///
/// Each fetch method spawns an independent task that connects to the server,
/// performs the RPC, and sends the result back as a `DataReady(DataPayload)`
/// event through the event channel. Multiple concurrent fetches are safe
/// because each task creates its own gRPC channel.
#[derive(Clone)]
pub struct DataManager {
    port: u16,
    sender: mpsc::UnboundedSender<AppEvent>,
}

impl DataManager {
    /// Create a new `DataManager` targeting the given server port and pushing
    /// results through the provided event sender.
    pub fn new(port: u16, sender: mpsc::UnboundedSender<AppEvent>) -> Self {
        Self { port, sender }
    }

    /// Spawn a background task that fetches all tickets via `ListTickets` and
    /// sends the result as `DataPayload::Tickets`.
    pub fn fetch_tickets(&self) {
        let port = self.port;
        let tx = self.sender.clone();

        tokio::spawn(async move {
            debug!(port, "fetching tickets");
            let payload = match fetch_tickets_rpc(port).await {
                Ok(resp) => DataPayload::Tickets(Ok(resp.tickets)),
                Err(e) => {
                    error!(port, error = %e, "ticket fetch failed");
                    DataPayload::Tickets(Err(e.to_string()))
                }
            };
            let _ = tx.send(AppEvent::DataReady(payload));
        });
    }

    /// Spawn a background task that dispatches a ticket: creates a workflow
    /// with AWAITING_DISPATCH status, then launches a worker container.
    ///
    /// The result is sent as `AppEvent::ActionResult` with a success or error message.
    pub fn dispatch_ticket(&self, ticket_id: String, projects: &HashMap<String, ProjectConfig>) {
        let port = self.port;
        let tx = self.sender.clone();
        let project_key = resolve_project_from_ticket(&ticket_id, projects);
        let image_id = projects
            .get(&project_key)
            .map(|p| p.container.image.clone())
            .unwrap_or_default();

        tokio::spawn(async move {
            let result = dispatch_ticket_rpc(port, &ticket_id, &project_key, &image_id).await;
            let action_result = match result {
                Ok(()) => ActionResult {
                    result: Ok(format!("Dispatched {ticket_id}")),
                },
                Err(e) => ActionResult {
                    result: Err(e.to_string()),
                },
            };
            let _ = tx.send(AppEvent::ActionResult(action_result));
        });
    }

    /// Spawn a background task that fetches all workflows via `ListWorkflows`
    /// and sends the result as `DataPayload::Flows`.
    pub fn fetch_flows(&self) {
        let port = self.port;
        let tx = self.sender.clone();

        tokio::spawn(async move {
            debug!(port, "fetching workflows");
            let payload = match fetch_workflows_rpc(port).await {
                Ok(resp) => DataPayload::Flows(Ok(resp.workflows)),
                Err(e) => {
                    error!(port, error = %e, "workflow fetch failed");
                    DataPayload::Flows(Err(e.to_string()))
                }
            };
            let _ = tx.send(AppEvent::DataReady(payload));
        });
    }
}

/// Perform the ListTickets RPC call, returning the full response.
async fn fetch_tickets_rpc(port: u16) -> Result<ListTicketsResponse> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    let request = tonic::Request::new(ListTicketsRequest {
        project: None,
        ticket_type: None,
        status: None,
        meta_key: None,
        meta_value: None,
        tree_root_id: None,
    });
    let response = client.list_tickets(request).await?;
    Ok(response.into_inner())
}

/// Perform the ListWorkflows RPC call, returning the full response.
async fn fetch_workflows_rpc(port: u16) -> Result<ListWorkflowsResponse> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    let request = tonic::Request::new(ListWorkflowsRequest { status: None });
    let response = client.list_workflows(request).await?;
    Ok(response.into_inner())
}

/// Resolve the project key from a ticket ID by extracting the prefix before
/// the first `-` and matching it against known project keys.
fn resolve_project_from_ticket(
    ticket_id: &str,
    projects: &HashMap<String, ProjectConfig>,
) -> String {
    let prefix = ticket_id
        .split(&['-', '.'][..])
        .next()
        .unwrap_or("")
        .to_owned();
    if !prefix.is_empty() && projects.contains_key(&prefix) {
        return prefix;
    }
    String::new()
}

/// Perform the two-step dispatch: CreateWorkflow then WorkerLaunch.
async fn dispatch_ticket_rpc(
    port: u16,
    ticket_id: &str,
    project_key: &str,
    image_id: &str,
) -> Result<()> {
    // Step 1: Create workflow with AWAITING_DISPATCH status
    let channel = connect(port).await?;
    let mut ticket_client = TicketServiceClient::new(channel);
    ticket_client
        .create_workflow(CreateWorkflowRequest {
            ticket_id: ticket_id.to_owned(),
            status: ur_rpc::lifecycle::AWAITING_DISPATCH.to_owned(),
        })
        .await?;
    debug!(ticket_id, "created workflow with awaiting_dispatch status");

    // Step 2: Launch worker container
    let channel = connect(port).await?;
    let mut core_client = CoreServiceClient::new(channel);
    core_client
        .worker_launch(WorkerLaunchRequest {
            worker_id: ticket_id.to_owned(),
            image_id: image_id.to_owned(),
            cpus: 2,
            memory: "8G".into(),
            workspace_dir: String::new(),
            claude_credentials: String::new(),
            mode: String::new(),
            skills: Vec::new(),
            project_key: project_key.to_owned(),
        })
        .await?;
    debug!(ticket_id, "worker launched via dispatch");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_manager_is_clone() {
        fn assert_clone<T: Clone>() {}
        assert_clone::<DataManager>();
    }

    #[test]
    fn data_payload_variants() {
        // Verify the enum variants can be constructed with the expected types.
        let tickets_ok = DataPayload::Tickets(Ok(vec![]));
        let tickets_err = DataPayload::Tickets(Err("connection refused".into()));
        let flows_ok = DataPayload::Flows(Ok(vec![]));
        let flows_err = DataPayload::Flows(Err("timeout".into()));

        assert!(matches!(tickets_ok, DataPayload::Tickets(Ok(_))));
        assert!(matches!(tickets_err, DataPayload::Tickets(Err(_))));
        assert!(matches!(flows_ok, DataPayload::Flows(Ok(_))));
        assert!(matches!(flows_err, DataPayload::Flows(Err(_))));
    }
}
