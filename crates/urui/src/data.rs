use std::collections::HashMap;

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{debug, error};

use ur_config::ProjectConfig;
use ur_rpc::connection::connect;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::core::{
    WorkerLaunchRequest, WorkerListRequest, WorkerListResponse, WorkerSummary,
};
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
use ur_rpc::proto::ticket::{
    CancelWorkflowRequest, CreateTicketRequest, CreateWorkflowRequest, GetTicketRequest,
    GetWorkflowRequest, ListTicketsRequest, ListTicketsResponse, ListWorkflowsRequest,
    ListWorkflowsResponse, SubscribeUiEventsRequest, Ticket, UiEventType, UpdateTicketRequest,
    WorkflowInfo,
};

use crate::create_ticket::{PendingTicket, is_title_placeholder};
use crate::event::{AppEvent, UiEventItem};

/// Payload delivered by gRPC data-fetch tasks.
#[derive(Debug, Clone)]
pub enum DataPayload {
    /// Ticket list fetched from the server.
    Tickets(Result<Vec<Ticket>, String>),
    /// Workflow list fetched from the server.
    Flows(Result<Vec<WorkflowInfo>, String>),
    /// A single ticket fetched after a UI event notification.
    TicketUpdate(Result<Ticket, String>),
    /// A single workflow fetched after a UI event notification.
    FlowUpdate(Result<WorkflowInfo, String>),
    /// Worker list fetched from the server.
    Workers(Result<Vec<WorkerSummary>, String>),
}

/// Result of an async action (dispatch, etc.) sent back via `AppEvent::ActionResult`.
#[derive(Debug, Clone)]
pub struct ActionResult {
    /// Human-readable success message, or error message.
    pub result: Result<String, String>,
    /// When true, suppress the success banner (errors still shown).
    pub silent_on_success: bool,
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
            let _ = tx.send(AppEvent::DataReady(Box::new(payload)));
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
                    silent_on_success: false,
                },
                Err(e) => ActionResult {
                    result: Err(e.to_string()),
                    silent_on_success: false,
                },
            };
            let _ = tx.send(AppEvent::ActionResult(action_result));
        });
    }

    /// Spawn a background task that updates a ticket's priority via `UpdateTicket`.
    /// On success, no banner is shown (silent). On failure, an error banner appears.
    pub fn update_ticket_priority(&self, ticket_id: String, priority: i64) {
        let port = self.port;
        let tx = self.sender.clone();

        tokio::spawn(async move {
            debug!(port, %ticket_id, priority, "updating ticket priority");
            let result = update_ticket_priority_rpc(port, &ticket_id, priority).await;
            let action_result = match result {
                Ok(()) => ActionResult {
                    result: Ok(format!("Priority set to P{priority} for {ticket_id}")),
                    silent_on_success: true,
                },
                Err(e) => {
                    error!(port, %ticket_id, error = %e, "priority update failed");
                    ActionResult {
                        result: Err(e.to_string()),
                        silent_on_success: false,
                    }
                }
            };
            let _ = tx.send(AppEvent::ActionResult(action_result));
        });
    }

    /// Spawn a background task that updates a ticket's status via `UpdateTicket`.
    /// On success, triggers a data refresh. On failure, an error banner appears.
    pub fn update_ticket_status(&self, ticket_id: String, status: String) {
        let port = self.port;
        let tx = self.sender.clone();

        tokio::spawn(async move {
            debug!(port, %ticket_id, %status, "updating ticket status");
            let result = update_ticket_status_rpc(port, &ticket_id, &status).await;
            let action_result = match result {
                Ok(()) => ActionResult {
                    result: Ok(format!("{ticket_id} → {status}")),
                    silent_on_success: false,
                },
                Err(e) => {
                    error!(port, %ticket_id, error = %e, "status update failed");
                    ActionResult {
                        result: Err(e.to_string()),
                        silent_on_success: false,
                    }
                }
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
            let _ = tx.send(AppEvent::DataReady(Box::new(payload)));
        });
    }

    /// Spawn a background task that subscribes to the server's UI event stream.
    ///
    /// Each batch of events is converted to `AppEvent::UiEvent` and sent through
    /// the event channel. On stream disconnect, a warning is logged and the task
    /// exits (the tick-timer fallback will keep data refreshing).
    pub fn subscribe_events(&self) {
        let port = self.port;
        let tx = self.sender.clone();

        tokio::spawn(async move {
            debug!(port, "subscribing to UI event stream");
            if let Err(e) = consume_ui_event_stream(port, &tx).await {
                tracing::warn!(port, error = %e, "UI event stream disconnected");
            }
        });
    }

    /// Spawn a background task that fetches a single ticket by ID via `GetTicket`
    /// and sends the result as `DataPayload::TicketUpdate`.
    pub fn fetch_ticket(&self, ticket_id: String) {
        let port = self.port;
        let tx = self.sender.clone();

        tokio::spawn(async move {
            debug!(port, %ticket_id, "fetching single ticket");
            let payload = match fetch_ticket_rpc(port, &ticket_id).await {
                Ok(ticket) => DataPayload::TicketUpdate(Ok(ticket)),
                Err(e) => {
                    error!(port, %ticket_id, error = %e, "single ticket fetch failed");
                    DataPayload::TicketUpdate(Err(e.to_string()))
                }
            };
            let _ = tx.send(AppEvent::DataReady(Box::new(payload)));
        });
    }

    /// Spawn a background task that fetches a single workflow by ticket ID via
    /// `GetWorkflow` and sends the result as `DataPayload::FlowUpdate`.
    pub fn fetch_workflow(&self, ticket_id: String) {
        let port = self.port;
        let tx = self.sender.clone();

        tokio::spawn(async move {
            debug!(port, %ticket_id, "fetching single workflow");
            let payload = match fetch_workflow_rpc(port, &ticket_id).await {
                Ok(workflow) => DataPayload::FlowUpdate(Ok(workflow)),
                Err(e) => {
                    error!(port, %ticket_id, error = %e, "single workflow fetch failed");
                    DataPayload::FlowUpdate(Err(e.to_string()))
                }
            };
            let _ = tx.send(AppEvent::DataReady(Box::new(payload)));
        });
    }

    /// Spawn a background task that cancels the active workflow for a ticket
    /// via `CancelWorkflow` and sends the result as `AppEvent::ActionResult`.
    pub fn cancel_flow(&self, ticket_id: String) {
        let port = self.port;
        let tx = self.sender.clone();

        tokio::spawn(async move {
            debug!(port, %ticket_id, "cancelling workflow");
            let result = cancel_workflow_rpc(port, &ticket_id).await;
            let action_result = match result {
                Ok(()) => ActionResult {
                    result: Ok(format!("Cancelled workflow for {ticket_id}")),
                    silent_on_success: false,
                },
                Err(e) => {
                    error!(port, %ticket_id, error = %e, "workflow cancel failed");
                    ActionResult {
                        result: Err(e.to_string()),
                        silent_on_success: false,
                    }
                }
            };
            let _ = tx.send(AppEvent::ActionResult(action_result));
        });
    }

    /// Spawn a background task that fetches all workers via `WorkerList`
    /// and sends the result as `DataPayload::Workers`.
    pub fn fetch_workers(&self) {
        let port = self.port;
        let tx = self.sender.clone();

        tokio::spawn(async move {
            debug!(port, "fetching workers");
            let payload = match fetch_workers_rpc(port).await {
                Ok(resp) => DataPayload::Workers(Ok(resp.workers)),
                Err(e) => {
                    error!(port, error = %e, "worker fetch failed");
                    DataPayload::Workers(Err(e.to_string()))
                }
            };
            let _ = tx.send(AppEvent::DataReady(Box::new(payload)));
        });
    }

    /// Spawn a background task that creates a ticket from a `PendingTicket`.
    ///
    /// If the title is a placeholder, resolves it via `resolve_title` before
    /// calling `CreateTicket` RPC. Sends the result as `AppEvent::ActionResult`.
    pub fn create_ticket(&self, pending: PendingTicket, projects: &HashMap<String, ProjectConfig>) {
        let port = self.port;
        let tx = self.sender.clone();
        let project_key = pending.project.clone();
        let _image_id = projects
            .get(&project_key)
            .map(|p| p.container.image.clone())
            .unwrap_or_default();

        tokio::spawn(async move {
            let action_result = match create_ticket_flow(port, pending).await {
                Ok(ticket_id) => ActionResult {
                    result: Ok(format!("Created {ticket_id}")),
                    silent_on_success: false,
                },
                Err(e) => ActionResult {
                    result: Err(e.to_string()),
                    silent_on_success: false,
                },
            };
            let _ = tx.send(AppEvent::ActionResult(action_result));
        });
    }

    /// Spawn a background task that creates a ticket and dispatches it.
    ///
    /// Resolves title if placeholder, creates via RPC, then dispatches
    /// (CreateWorkflow + WorkerLaunch). Sends `AppEvent::ActionResult`.
    pub fn create_and_dispatch_ticket(
        &self,
        pending: PendingTicket,
        projects: &HashMap<String, ProjectConfig>,
    ) {
        let port = self.port;
        let tx = self.sender.clone();
        let project_key = pending.project.clone();
        let image_id = projects
            .get(&project_key)
            .map(|p| p.container.image.clone())
            .unwrap_or_default();

        tokio::spawn(async move {
            let action_result =
                match create_and_dispatch_flow(port, pending, &project_key, &image_id).await {
                    Ok(ticket_id) => ActionResult {
                        result: Ok(format!("Created and dispatched {ticket_id}")),
                        silent_on_success: false,
                    },
                    Err(e) => ActionResult {
                        result: Err(e.to_string()),
                        silent_on_success: false,
                    },
                };
            let _ = tx.send(AppEvent::ActionResult(action_result));
        });
    }

    /// Spawn a background task that creates a ticket and launches a design worker.
    ///
    /// Resolves title if placeholder, creates via RPC, then launches a worker
    /// with mode=design (no workflow). Sends `AppEvent::ActionResult`.
    pub fn create_and_design_ticket(
        &self,
        pending: PendingTicket,
        projects: &HashMap<String, ProjectConfig>,
    ) {
        let port = self.port;
        let tx = self.sender.clone();
        let project_key = pending.project.clone();
        let image_id = projects
            .get(&project_key)
            .map(|p| p.container.image.clone())
            .unwrap_or_default();

        tokio::spawn(async move {
            let action_result =
                match create_and_design_flow(port, pending, &project_key, &image_id).await {
                    Ok(ticket_id) => ActionResult {
                        result: Ok(format!("Created {ticket_id} with design worker")),
                        silent_on_success: false,
                    },
                    Err(e) => ActionResult {
                        result: Err(e.to_string()),
                        silent_on_success: false,
                    },
                };
            let _ = tx.send(AppEvent::ActionResult(action_result));
        });
    }
}

/// Create a ticket, resolving the title first if it's a placeholder.
async fn create_ticket_flow(port: u16, pending: PendingTicket) -> Result<String> {
    let title = if is_title_placeholder(&pending.title) {
        resolve_title(&pending.body).await?
    } else {
        pending.title
    };
    create_ticket_rpc(
        port,
        &pending.project,
        &title,
        pending.priority,
        &pending.body,
    )
    .await
}

/// Create a ticket and dispatch it (CreateWorkflow + WorkerLaunch).
async fn create_and_dispatch_flow(
    port: u16,
    pending: PendingTicket,
    project_key: &str,
    image_id: &str,
) -> Result<String> {
    let title = if is_title_placeholder(&pending.title) {
        resolve_title(&pending.body).await?
    } else {
        pending.title
    };
    let ticket_id = create_ticket_rpc(
        port,
        &pending.project,
        &title,
        pending.priority,
        &pending.body,
    )
    .await?;
    dispatch_ticket_rpc(port, &ticket_id, project_key, image_id).await?;
    Ok(ticket_id)
}

/// Create a ticket and launch a design worker (no workflow).
async fn create_and_design_flow(
    port: u16,
    pending: PendingTicket,
    project_key: &str,
    image_id: &str,
) -> Result<String> {
    let title = if is_title_placeholder(&pending.title) {
        resolve_title(&pending.body).await?
    } else {
        pending.title
    };
    let ticket_id = create_ticket_rpc(
        port,
        &pending.project,
        &title,
        pending.priority,
        &pending.body,
    )
    .await?;
    launch_design_worker_rpc(port, &ticket_id, project_key, image_id).await?;
    Ok(ticket_id)
}

/// Resolve a ticket title from the body by running `claude -m haiku --print`.
///
/// Falls back to a truncated body (first 80 chars) if the command fails.
/// Returns an error only if the body is also empty.
async fn resolve_title(body: &str) -> Result<String> {
    let prompt = format!(
        "Generate a concise ticket title (under 80 chars) for this description. \
         Output ONLY the title, nothing else:\n\n{body}"
    );
    let output = tokio::process::Command::new("claude")
        .args(["-m", "haiku", "--print", "-p", &prompt])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            let title = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !title.is_empty() {
                return Ok(title);
            }
        }
        Ok(o) => {
            debug!(
                status = %o.status,
                stderr = %String::from_utf8_lossy(&o.stderr),
                "claude title generation failed, falling back to truncation"
            );
        }
        Err(e) => {
            debug!(error = %e, "claude command not available, falling back to truncation");
        }
    }

    // Fallback: truncate body to 80 chars
    let trimmed = body.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Cannot resolve title: body is empty and claude command failed");
    }
    let first_line = trimmed.lines().next().unwrap_or(trimmed);
    if first_line.len() <= 80 {
        Ok(first_line.to_string())
    } else {
        Ok(format!("{}...", &first_line[..77]))
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

/// Perform the CancelWorkflow RPC to cancel the active workflow for a ticket.
async fn cancel_workflow_rpc(port: u16, ticket_id: &str) -> Result<()> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    client
        .cancel_workflow(CancelWorkflowRequest {
            ticket_id: ticket_id.to_owned(),
        })
        .await?;
    Ok(())
}

/// Perform the UpdateTicket RPC to change a ticket's status.
async fn update_ticket_status_rpc(port: u16, ticket_id: &str, status: &str) -> Result<()> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    client
        .update_ticket(UpdateTicketRequest {
            id: ticket_id.to_owned(),
            priority: None,
            status: Some(status.to_owned()),
            title: None,
            body: None,
            force: false,
            ticket_type: None,
            parent_id: None,
            branch: None,
            project: None,
        })
        .await?;
    Ok(())
}

/// Perform the UpdateTicket RPC to change a ticket's priority.
async fn update_ticket_priority_rpc(port: u16, ticket_id: &str, priority: i64) -> Result<()> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    client
        .update_ticket(UpdateTicketRequest {
            id: ticket_id.to_owned(),
            priority: Some(priority),
            status: None,
            title: None,
            body: None,
            force: false,
            ticket_type: None,
            parent_id: None,
            branch: None,
            project: None,
        })
        .await?;
    Ok(())
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

/// Perform the CreateTicket RPC, returning the created ticket ID.
async fn create_ticket_rpc(
    port: u16,
    project: &str,
    title: &str,
    priority: i64,
    body: &str,
) -> Result<String> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    let resp = client
        .create_ticket(CreateTicketRequest {
            project: project.to_owned(),
            ticket_type: "task".to_owned(),
            status: String::new(),
            priority,
            parent_id: None,
            title: title.to_owned(),
            body: body.to_owned(),
            id: None,
            created_at: None,
            wip: false,
        })
        .await?;
    Ok(resp.into_inner().id)
}

/// Launch a design worker for a ticket (no workflow).
async fn launch_design_worker_rpc(
    port: u16,
    ticket_id: &str,
    project_key: &str,
    image_id: &str,
) -> Result<()> {
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
            mode: "design".to_owned(),
            skills: Vec::new(),
            project_key: project_key.to_owned(),
        })
        .await?;
    debug!(ticket_id, "design worker launched");
    Ok(())
}

/// Perform the WorkerList RPC, returning the full response.
async fn fetch_workers_rpc(port: u16) -> Result<WorkerListResponse> {
    let channel = connect(port).await?;
    let mut client = CoreServiceClient::new(channel);
    let response = client.worker_list(WorkerListRequest {}).await?;
    Ok(response.into_inner())
}

/// Fetch a single ticket by ID via GetTicket RPC.
async fn fetch_ticket_rpc(port: u16, ticket_id: &str) -> Result<Ticket> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    let resp = client
        .get_ticket(GetTicketRequest {
            id: ticket_id.to_owned(),
            activity_author_filter: None,
        })
        .await?;
    resp.into_inner()
        .ticket
        .ok_or_else(|| anyhow::anyhow!("GetTicket returned no ticket for {ticket_id}"))
}

/// Fetch a single workflow by ticket ID via GetWorkflow RPC.
async fn fetch_workflow_rpc(port: u16, ticket_id: &str) -> Result<WorkflowInfo> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    let resp = client
        .get_workflow(GetWorkflowRequest {
            ticket_id: ticket_id.to_owned(),
        })
        .await?;
    resp.into_inner()
        .workflow
        .ok_or_else(|| anyhow::anyhow!("GetWorkflow returned no workflow for {ticket_id}"))
}

/// Connect to the UI event stream and forward batches as `AppEvent::UiEvent`.
async fn consume_ui_event_stream(port: u16, tx: &mpsc::UnboundedSender<AppEvent>) -> Result<()> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    let response = client
        .subscribe_ui_events(SubscribeUiEventsRequest {})
        .await?;
    let mut stream = response.into_inner();

    while let Some(batch) = stream.message().await? {
        let items: Vec<UiEventItem> = batch
            .events
            .into_iter()
            .map(|ev| UiEventItem {
                entity_type: ui_event_type_to_str(ev.entity_type()),
                entity_id: ev.entity_id,
            })
            .collect();
        if !items.is_empty() && tx.send(AppEvent::UiEvent(items)).is_err() {
            break;
        }
    }
    Ok(())
}

/// Convert a proto UiEventType enum value to a string label.
fn ui_event_type_to_str(t: UiEventType) -> String {
    match t {
        UiEventType::Ticket => "ticket".to_owned(),
        UiEventType::Workflow => "workflow".to_owned(),
        UiEventType::Worker => "worker".to_owned(),
        UiEventType::Unknown => "unknown".to_owned(),
    }
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

    #[test]
    fn ticket_update_variant_round_trip() {
        let ticket = Ticket {
            id: "ur-test".into(),
            ..Default::default()
        };
        let payload = DataPayload::TicketUpdate(Ok(ticket.clone()));
        assert!(matches!(payload, DataPayload::TicketUpdate(Ok(ref t)) if t.id == "ur-test"));

        let err_payload = DataPayload::TicketUpdate(Err("not found".into()));
        assert!(matches!(err_payload, DataPayload::TicketUpdate(Err(_))));
    }

    #[test]
    fn flow_update_variant_round_trip() {
        let flow = WorkflowInfo {
            ticket_id: "ur-flow".into(),
            ..Default::default()
        };
        let payload = DataPayload::FlowUpdate(Ok(flow.clone()));
        assert!(matches!(payload, DataPayload::FlowUpdate(Ok(ref f)) if f.ticket_id == "ur-flow"));

        let err_payload = DataPayload::FlowUpdate(Err("timeout".into()));
        assert!(matches!(err_payload, DataPayload::FlowUpdate(Err(_))));
    }

    #[test]
    fn ui_event_type_conversion() {
        assert_eq!(ui_event_type_to_str(UiEventType::Ticket), "ticket");
        assert_eq!(ui_event_type_to_str(UiEventType::Workflow), "workflow");
        assert_eq!(ui_event_type_to_str(UiEventType::Worker), "worker");
        assert_eq!(ui_event_type_to_str(UiEventType::Unknown), "unknown");
    }

    #[test]
    fn workers_variant_round_trip() {
        let workers_ok = DataPayload::Workers(Ok(vec![]));
        assert!(matches!(workers_ok, DataPayload::Workers(Ok(ref w)) if w.is_empty()));

        let workers_err = DataPayload::Workers(Err("connection refused".into()));
        assert!(matches!(workers_err, DataPayload::Workers(Err(_))));

        let summary = WorkerSummary {
            worker_id: "ur-abc".into(),
            ..Default::default()
        };
        let workers_one = DataPayload::Workers(Ok(vec![summary]));
        assert!(matches!(workers_one, DataPayload::Workers(Ok(ref w)) if w.len() == 1));
    }

    #[tokio::test]
    async fn resolve_title_fallback_truncates_body() {
        // claude command is not available in test, so it should fall back to truncation
        let body = "This is a short body";
        let title = resolve_title(body).await.unwrap();
        assert_eq!(title, "This is a short body");
    }

    #[tokio::test]
    async fn resolve_title_fallback_truncates_long_body() {
        let body = "A".repeat(200);
        let title = resolve_title(&body).await.unwrap();
        assert_eq!(title.len(), 80); // 77 chars + "..."
        assert!(title.ends_with("..."));
    }

    #[tokio::test]
    async fn resolve_title_fallback_uses_first_line() {
        let body = "First line title\nSecond line detail\nMore detail";
        let title = resolve_title(body).await.unwrap();
        assert_eq!(title, "First line title");
    }

    #[tokio::test]
    async fn resolve_title_empty_body_errors() {
        let result = resolve_title("").await;
        assert!(result.is_err());
        let result2 = resolve_title("   \n  ").await;
        assert!(result2.is_err());
    }
}
