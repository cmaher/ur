use std::collections::HashMap;

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace};

use ur_config::ProjectConfig;
use ur_rpc::connection::connect;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::core::{
    WorkerLaunchRequest, WorkerListRequest, WorkerListResponse, WorkerStopRequest, WorkerSummary,
};
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
use ur_rpc::proto::ticket::{
    ActivityEntry, CancelWorkflowRequest, CreateTicketRequest, CreateWorkflowRequest,
    GetTicketRequest, GetTicketResponse, ListTicketsRequest, ListTicketsResponse,
    ListWorkflowsRequest, ListWorkflowsResponse, RedriveTicketRequest, SubscribeUiEventsRequest,
    Ticket, UiEventType, UpdateTicketRequest,
};

use crate::create_ticket::{PendingTicket, is_title_placeholder};
use crate::event::{AppEvent, UiEventItem};

/// Combined result for a ticket detail fetch: (response, child tickets, total children).
pub type TicketDetailResult = Result<(GetTicketResponse, Vec<Ticket>, i32), String>;

/// Payload delivered by gRPC data-fetch tasks.
#[derive(Debug, Clone)]
pub enum DataPayload {
    /// Ticket list fetched from the server (data, total_count).
    Tickets(Result<(Vec<ur_rpc::proto::ticket::Ticket>, i32), String>),
    /// Workflow list fetched from the server (data, total_count).
    Flows(Result<(Vec<ur_rpc::proto::ticket::WorkflowInfo>, i32), String>),
    /// Worker list fetched from the server.
    Workers(Result<Vec<WorkerSummary>, String>),
    /// Full ticket detail: the ticket with metadata/activities, child tickets, and total child count.
    TicketDetail(Box<TicketDetailResult>),
    /// Activities for a single ticket, optionally filtered by author.
    TicketActivities(Result<Vec<ActivityEntry>, String>),
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
    /// When set, scopes all data fetching to a single project key.
    project_filter: Option<String>,
}

impl DataManager {
    /// Create a new `DataManager` targeting the given server port and pushing
    /// results through the provided event sender.
    pub fn new(
        port: u16,
        sender: mpsc::UnboundedSender<AppEvent>,
        project_filter: Option<String>,
    ) -> Self {
        Self {
            port,
            sender,
            project_filter,
        }
    }

    /// Spawn a background task that fetches tickets via `ListTickets` and
    /// sends the result as `DataPayload::Tickets`.
    pub fn fetch_tickets(
        &self,
        page_size: Option<i32>,
        offset: Option<i32>,
        include_children: Option<bool>,
        statuses: &[String],
    ) {
        let port = self.port;
        let tx = self.sender.clone();
        let project = self.project_filter.clone();
        let status = if statuses.is_empty() {
            None
        } else {
            Some(statuses.join(","))
        };

        tokio::spawn(async move {
            debug!(port, "fetching tickets");
            let payload =
                match fetch_tickets_rpc(port, project, page_size, offset, include_children, status)
                    .await
                {
                    Ok(resp) => DataPayload::Tickets(Ok((resp.tickets, resp.total_count))),
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
    /// Design-type tickets are routed through `launch_design_worker_rpc` instead,
    /// which sets mode=design and dispatch=true so the server sends `/design <ticket>`.
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
            let is_design = is_design_ticket(port, &ticket_id).await.unwrap_or(false);
            let result = if is_design {
                launch_design_worker_rpc(port, &ticket_id, &project_key, &image_id).await
            } else {
                dispatch_ticket_rpc(port, &ticket_id, &project_key, &image_id).await
            };
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
            let result = update_ticket_status_rpc(port, &ticket_id, &status, false).await;
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

    /// Spawn a background task that force-closes a ticket (recursively closing
    /// all open children) via `UpdateTicket` with `force: true`.
    /// On success, no banner is shown. On failure, an error banner appears.
    pub fn force_close_ticket(&self, ticket_id: String) {
        let port = self.port;
        let tx = self.sender.clone();

        tokio::spawn(async move {
            debug!(port, %ticket_id, "force-closing ticket");
            let result = update_ticket_status_rpc(port, &ticket_id, "closed", true).await;
            let action_result = match result {
                Ok(()) => ActionResult {
                    result: Ok(format!("{ticket_id} → closed (force)")),
                    silent_on_success: true,
                },
                Err(e) => {
                    error!(port, %ticket_id, error = %e, "force close failed");
                    ActionResult {
                        result: Err(e.to_string()),
                        silent_on_success: false,
                    }
                }
            };
            let _ = tx.send(AppEvent::ActionResult(action_result));
        });
    }

    /// Spawn a background task that fetches workflows via `ListWorkflows`
    /// and sends the result as `DataPayload::Flows`.
    pub fn fetch_flows(&self, page_size: Option<i32>, offset: Option<i32>) {
        let port = self.port;
        let tx = self.sender.clone();
        let project = self.project_filter.clone();

        tokio::spawn(async move {
            debug!(port, "fetching workflows");
            let payload = match fetch_workflows_rpc(port, page_size, offset, project).await {
                Ok(resp) => DataPayload::Flows(Ok((resp.workflows, resp.total_count))),
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

    /// Spawn a background task that redrives a ticket to `verifying` status
    /// via `RedriveTicket` and sends the result as `AppEvent::ActionResult`.
    pub fn redrive_flow(&self, ticket_id: String) {
        let port = self.port;
        let tx = self.sender.clone();

        tokio::spawn(async move {
            debug!(port, %ticket_id, "redriving ticket to verifying");
            let result = redrive_ticket_rpc(port, &ticket_id).await;
            let action_result = match result {
                Ok(()) => ActionResult {
                    result: Ok(format!("Redrove {ticket_id} to verifying")),
                    silent_on_success: false,
                },
                Err(e) => {
                    error!(port, %ticket_id, error = %e, "redrive failed");
                    ActionResult {
                        result: Err(e.to_string()),
                        silent_on_success: false,
                    }
                }
            };
            let _ = tx.send(AppEvent::ActionResult(action_result));
        });
    }

    /// Spawn a background task that stops a worker via `WorkerStop`
    /// and sends the result as `AppEvent::ActionResult`.
    pub fn stop_worker(&self, worker_id: String) {
        let port = self.port;
        let tx = self.sender.clone();

        tokio::spawn(async move {
            debug!(port, %worker_id, "stopping worker");
            let result = stop_worker_rpc(port, &worker_id).await;
            let action_result = match result {
                Ok(()) => ActionResult {
                    result: Ok(format!("Killed {worker_id}")),
                    silent_on_success: false,
                },
                Err(e) => {
                    error!(port, %worker_id, error = %e, "worker stop failed");
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
        let project = self.project_filter.clone();

        tokio::spawn(async move {
            debug!(port, "fetching workers");
            let payload = match fetch_workers_rpc(port).await {
                Ok(resp) => {
                    let workers = filter_workers_by_project(resp.workers, &project);
                    DataPayload::Workers(Ok(workers))
                }
                Err(e) => {
                    error!(port, error = %e, "worker fetch failed");
                    DataPayload::Workers(Err(e.to_string()))
                }
            };
            let _ = tx.send(AppEvent::DataReady(Box::new(payload)));
        });
    }

    /// Spawn a background task that fetches full ticket detail by firing both
    /// `GetTicket` and `ListTickets` (with `tree_root_id`) concurrently, then
    /// sends the combined result as `DataPayload::TicketDetail`.
    pub fn fetch_ticket_detail(
        &self,
        ticket_id: String,
        child_page_size: Option<i32>,
        child_offset: Option<i32>,
    ) {
        let port = self.port;
        let tx = self.sender.clone();

        tokio::spawn(async move {
            debug!(port, %ticket_id, "fetching ticket detail");
            let payload = match fetch_ticket_detail_rpc(
                port,
                &ticket_id,
                child_page_size,
                child_offset,
            )
            .await
            {
                Ok((detail, children, total)) => {
                    DataPayload::TicketDetail(Box::new(Ok((detail, children, total))))
                }
                Err(e) => {
                    error!(port, %ticket_id, error = %e, "ticket detail fetch failed");
                    DataPayload::TicketDetail(Box::new(Err(e.to_string())))
                }
            };
            let _ = tx.send(AppEvent::DataReady(Box::new(payload)));
        });
    }

    /// Spawn a background task that fetches activities for a single ticket via
    /// `GetTicket` (with optional `activity_author_filter`) and sends the result
    /// as `DataPayload::TicketActivities`.
    pub fn fetch_ticket_activities(&self, ticket_id: String, author_filter: Option<String>) {
        let port = self.port;
        let tx = self.sender.clone();

        tokio::spawn(async move {
            debug!(port, %ticket_id, ?author_filter, "fetching ticket activities");
            let payload = match fetch_ticket_activities_rpc(port, &ticket_id, author_filter).await {
                Ok(activities) => DataPayload::TicketActivities(Ok(activities)),
                Err(e) => {
                    error!(port, %ticket_id, error = %e, "ticket activities fetch failed");
                    DataPayload::TicketActivities(Err(e.to_string()))
                }
            };
            let _ = tx.send(AppEvent::DataReady(Box::new(payload)));
        });
    }

    /// Spawn a background task that launches a design worker for an existing ticket.
    ///
    /// Resolves the project from the ticket ID prefix, then calls
    /// `launch_design_worker_rpc` (no workflow). Sends `AppEvent::ActionResult`.
    pub fn launch_design_worker(
        &self,
        ticket_id: String,
        projects: &HashMap<String, ProjectConfig>,
    ) {
        let port = self.port;
        let tx = self.sender.clone();
        let project_key = resolve_project_from_ticket(&ticket_id, projects);
        let image_id = projects
            .get(&project_key)
            .map(|p| p.container.image.clone())
            .unwrap_or_default();

        tokio::spawn(async move {
            let action_result =
                match launch_design_worker_rpc(port, &ticket_id, &project_key, &image_id).await {
                    Ok(()) => ActionResult {
                        result: Ok(format!("Launched design worker for {ticket_id}")),
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
                match create_and_dispatch_flow(port, pending, &project_key, &image_id, &tx).await {
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
                match create_and_design_flow(port, pending, &project_key, &image_id, &tx).await {
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
        "task",
    )
    .await
}

/// Create a ticket and dispatch it (CreateWorkflow + WorkerLaunch).
async fn create_and_dispatch_flow(
    port: u16,
    pending: PendingTicket,
    project_key: &str,
    image_id: &str,
    tx: &mpsc::UnboundedSender<AppEvent>,
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
        "task",
    )
    .await?;
    let _ = tx.send(AppEvent::SetStatus(format!(
        "Launching worker for {ticket_id}..."
    )));
    dispatch_ticket_rpc(port, &ticket_id, project_key, image_id).await?;
    Ok(ticket_id)
}

/// Create a ticket and launch a design worker (no workflow).
async fn create_and_design_flow(
    port: u16,
    pending: PendingTicket,
    project_key: &str,
    image_id: &str,
    tx: &mpsc::UnboundedSender<AppEvent>,
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
        "design",
    )
    .await?;
    let _ = tx.send(AppEvent::SetStatus(format!(
        "Launching worker for {ticket_id}..."
    )));
    launch_design_worker_rpc(port, &ticket_id, project_key, image_id).await?;
    Ok(ticket_id)
}

/// Resolve a ticket title from the body by running `claude --model haiku --print`.
///
/// Falls back to a truncated body (first 80 chars) if the command fails.
/// Returns an error only if the body is also empty.
async fn resolve_title(body: &str) -> Result<String> {
    let prompt = format!(
        "Generate a concise ticket title (under 80 chars) for this description. \
         Output ONLY the title, nothing else:\n\n{body}"
    );
    let output = tokio::process::Command::new("claude")
        .args(["--model", "haiku", "--print", "-p", &prompt])
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

    fallback_title(body)
}

/// Truncate the body to produce a fallback title (first line, max 80 chars).
///
/// Returns an error if the body is empty.
fn fallback_title(body: &str) -> Result<String> {
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
async fn fetch_tickets_rpc(
    port: u16,
    project: Option<String>,
    page_size: Option<i32>,
    offset: Option<i32>,
    include_children: Option<bool>,
    status: Option<String>,
) -> Result<ListTicketsResponse> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    let request = tonic::Request::new(ListTicketsRequest {
        project,
        ticket_type: None,
        status,
        meta_key: None,
        meta_value: None,
        tree_root_id: None,
        page_size,
        offset,
        include_children,
        parent_id: None,
    });
    let response = client.list_tickets(request).await?;
    Ok(response.into_inner())
}

/// Fire `GetTicket` and `ListTickets` (with `tree_root_id`) concurrently and
/// return `(detail_response, children, total_child_count)`.
///
/// Both RPCs create their own gRPC channels. If either fails, the error is
/// returned and the `DataPayload::TicketDetail` variant will hold the `Err`
/// side.
async fn fetch_ticket_detail_rpc(
    port: u16,
    ticket_id: &str,
    child_page_size: Option<i32>,
    child_offset: Option<i32>,
) -> Result<(GetTicketResponse, Vec<Ticket>, i32)> {
    let ticket_id_owned = ticket_id.to_owned();
    let ticket_id_for_children = ticket_id.to_owned();

    let get_ticket_fut = async move {
        let channel = connect(port).await?;
        let mut client = TicketServiceClient::new(channel);
        let resp = client
            .get_ticket(GetTicketRequest {
                id: ticket_id_owned,
                activity_author_filter: None,
            })
            .await?;
        anyhow::Ok(resp.into_inner())
    };

    let list_children_fut = async move {
        let channel = connect(port).await?;
        let mut client = TicketServiceClient::new(channel);
        let resp = client
            .list_tickets(ListTicketsRequest {
                project: None,
                ticket_type: None,
                status: None,
                meta_key: None,
                meta_value: None,
                tree_root_id: None,
                page_size: child_page_size,
                offset: child_offset,
                include_children: None,
                parent_id: Some(ticket_id_for_children),
            })
            .await?;
        let inner = resp.into_inner();
        anyhow::Ok((inner.tickets, inner.total_count))
    };

    let (detail, (children, total)) = tokio::try_join!(get_ticket_fut, list_children_fut)?;
    Ok((detail, children, total))
}

/// Perform the `GetTicket` RPC with an optional author filter and return only
/// the activities list from the response.
async fn fetch_ticket_activities_rpc(
    port: u16,
    ticket_id: &str,
    author_filter: Option<String>,
) -> Result<Vec<ActivityEntry>> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    let resp = client
        .get_ticket(GetTicketRequest {
            id: ticket_id.to_owned(),
            activity_author_filter: author_filter,
        })
        .await?;
    Ok(resp.into_inner().activities)
}

/// Filter workers by project key. Returns all workers when no project filter
/// is set.
fn filter_workers_by_project(
    workers: Vec<WorkerSummary>,
    project: &Option<String>,
) -> Vec<WorkerSummary> {
    let Some(proj) = project else {
        return workers;
    };
    workers
        .into_iter()
        .filter(|w| w.project_key == *proj)
        .collect()
}

/// Perform the ListWorkflows RPC call, returning the full response.
async fn fetch_workflows_rpc(
    port: u16,
    page_size: Option<i32>,
    offset: Option<i32>,
    project: Option<String>,
) -> Result<ListWorkflowsResponse> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    let request = tonic::Request::new(ListWorkflowsRequest {
        status: None,
        page_size,
        offset,
        project,
    });
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

/// Perform the RedriveTicket RPC to redrive a ticket to verifying status.
async fn redrive_ticket_rpc(port: u16, ticket_id: &str) -> Result<()> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    client
        .redrive_ticket(RedriveTicketRequest {
            id: ticket_id.to_owned(),
            to_status: "verifying".to_owned(),
        })
        .await?;
    Ok(())
}

/// Perform the UpdateTicket RPC to change a ticket's status.
async fn update_ticket_status_rpc(
    port: u16,
    ticket_id: &str,
    status: &str,
    force: bool,
) -> Result<()> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    client
        .update_ticket(UpdateTicketRequest {
            id: ticket_id.to_owned(),
            priority: None,
            status: Some(status.to_owned()),
            title: None,
            body: None,
            force,
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
            context_repos: vec![],
            dispatch: false,
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
    ticket_type: &str,
) -> Result<String> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    let resp = client
        .create_ticket(CreateTicketRequest {
            project: project.to_owned(),
            ticket_type: ticket_type.to_owned(),
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

/// Check if a ticket is a design-type ticket by querying the server.
async fn is_design_ticket(port: u16, ticket_id: &str) -> Result<bool> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    let resp = client
        .get_ticket(GetTicketRequest {
            id: ticket_id.to_owned(),
            activity_author_filter: None,
        })
        .await?;
    let ticket = resp
        .into_inner()
        .ticket
        .ok_or_else(|| anyhow::anyhow!("ticket {ticket_id} not found"))?;
    Ok(ticket.ticket_type == "design")
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
            context_repos: vec![],
            dispatch: true,
        })
        .await?;
    debug!(ticket_id, "design worker launched");
    Ok(())
}

/// Perform the WorkerStop RPC to stop a running worker.
async fn stop_worker_rpc(port: u16, worker_id: &str) -> Result<()> {
    let channel = connect(port).await?;
    let mut client = CoreServiceClient::new(channel);
    client
        .worker_stop(WorkerStopRequest {
            worker_id: worker_id.to_owned(),
        })
        .await?;
    Ok(())
}

/// Perform the WorkerList RPC, returning the full response.
async fn fetch_workers_rpc(port: u16) -> Result<WorkerListResponse> {
    let channel = connect(port).await?;
    let mut client = CoreServiceClient::new(channel);
    let response = client.worker_list(WorkerListRequest {}).await?;
    Ok(response.into_inner())
}

/// Connect to the UI event stream and forward batches as `AppEvent::UiEvent`.
async fn consume_ui_event_stream(port: u16, tx: &mpsc::UnboundedSender<AppEvent>) -> Result<()> {
    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    let response = client
        .subscribe_ui_events(SubscribeUiEventsRequest {})
        .await?;
    let mut stream = response.into_inner();
    info!(port, "UI event stream connected successfully");

    while let Some(batch) = stream.message().await? {
        let items: Vec<UiEventItem> = batch
            .events
            .into_iter()
            .map(|ev| UiEventItem {
                entity_type: ui_event_type_to_str(ev.entity_type()),
                entity_id: ev.entity_id,
            })
            .collect();
        trace!(batch_size = items.len(), "received UI event batch");
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
        let tickets_ok = DataPayload::Tickets(Ok((vec![], 0)));
        let tickets_err = DataPayload::Tickets(Err("connection refused".into()));
        let flows_ok = DataPayload::Flows(Ok((vec![], 0)));
        let flows_err = DataPayload::Flows(Err("timeout".into()));

        assert!(matches!(tickets_ok, DataPayload::Tickets(Ok(_))));
        assert!(matches!(tickets_err, DataPayload::Tickets(Err(_))));
        assert!(matches!(flows_ok, DataPayload::Flows(Ok(_))));
        assert!(matches!(flows_err, DataPayload::Flows(Err(_))));
    }

    #[test]
    fn ticket_detail_variant_ok() {
        use ur_rpc::proto::ticket::{GetTicketResponse, Ticket};

        let detail = GetTicketResponse {
            ticket: Some(Ticket {
                id: "ur-abc".into(),
                ..Default::default()
            }),
            metadata: vec![],
            activities: vec![],
        };
        let children: Vec<Ticket> = vec![];
        let total = 0i32;

        let payload = DataPayload::TicketDetail(Box::new(Ok((detail, children, total))));
        assert!(matches!(payload, DataPayload::TicketDetail(_)));

        if let DataPayload::TicketDetail(boxed) = payload {
            let (resp, ch, tc) = boxed.unwrap();
            assert_eq!(resp.ticket.unwrap().id, "ur-abc");
            assert!(ch.is_empty());
            assert_eq!(tc, 0);
        } else {
            panic!("expected TicketDetail(Ok(...))");
        }
    }

    #[test]
    fn ticket_detail_variant_err() {
        let payload = DataPayload::TicketDetail(Box::new(Err("rpc failed".into())));
        assert!(matches!(payload, DataPayload::TicketDetail(_)));

        if let DataPayload::TicketDetail(boxed) = payload {
            let msg = boxed.unwrap_err();
            assert_eq!(msg, "rpc failed");
        } else {
            panic!("expected TicketDetail(Err(...))");
        }
    }

    #[test]
    fn ticket_detail_variant_with_children() {
        use ur_rpc::proto::ticket::{GetTicketResponse, Ticket};

        let children = vec![
            Ticket {
                id: "ur-child1".into(),
                ..Default::default()
            },
            Ticket {
                id: "ur-child2".into(),
                ..Default::default()
            },
        ];
        let detail = GetTicketResponse {
            ticket: Some(Ticket {
                id: "ur-parent".into(),
                ..Default::default()
            }),
            metadata: vec![],
            activities: vec![],
        };
        let total = 2i32;

        let payload = DataPayload::TicketDetail(Box::new(Ok((detail, children, total))));

        if let DataPayload::TicketDetail(boxed) = payload {
            let (resp, ch, tc) = boxed.unwrap();
            assert_eq!(resp.ticket.unwrap().id, "ur-parent");
            assert_eq!(ch.len(), 2);
            assert_eq!(tc, 2);
        } else {
            panic!("expected TicketDetail(Ok(...))");
        }
    }

    #[test]
    fn ticket_activities_variant_ok() {
        use ur_rpc::proto::ticket::ActivityEntry;

        let activities = vec![
            ActivityEntry {
                id: "act-1".into(),
                timestamp: "2026-03-25 14:32:10".into(),
                author: "alice".into(),
                message: "Fixed linter errors".into(),
            },
            ActivityEntry {
                id: "act-2".into(),
                timestamp: "2026-03-25 14:10:05".into(),
                author: "claude".into(),
                message: "Starting implementation".into(),
            },
        ];

        let payload = DataPayload::TicketActivities(Ok(activities));
        assert!(matches!(payload, DataPayload::TicketActivities(Ok(_))));

        if let DataPayload::TicketActivities(Ok(entries)) = payload {
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].author, "alice");
            assert_eq!(entries[1].author, "claude");
        } else {
            panic!("expected TicketActivities(Ok(...))");
        }
    }

    #[test]
    fn ticket_activities_variant_err() {
        let payload = DataPayload::TicketActivities(Err("rpc failed".into()));
        assert!(matches!(payload, DataPayload::TicketActivities(Err(_))));

        if let DataPayload::TicketActivities(Err(msg)) = payload {
            assert_eq!(msg, "rpc failed");
        } else {
            panic!("expected TicketActivities(Err(...))");
        }
    }

    #[test]
    fn ticket_activities_variant_empty() {
        let payload = DataPayload::TicketActivities(Ok(vec![]));
        assert!(matches!(payload, DataPayload::TicketActivities(Ok(ref a)) if a.is_empty()));
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

    #[test]
    fn fallback_title_truncates_body() {
        let body = "This is a short body";
        let title = fallback_title(body).unwrap();
        assert_eq!(title, "This is a short body");
    }

    #[test]
    fn fallback_title_truncates_long_body() {
        let body = "A".repeat(200);
        let title = fallback_title(&body).unwrap();
        assert_eq!(title.len(), 80); // 77 chars + "..."
        assert!(title.ends_with("..."));
    }

    #[test]
    fn fallback_title_uses_first_line() {
        let body = "First line title\nSecond line detail\nMore detail";
        let title = fallback_title(body).unwrap();
        assert_eq!(title, "First line title");
    }

    #[test]
    fn fallback_title_empty_body_errors() {
        let result = fallback_title("");
        assert!(result.is_err());
        let result2 = fallback_title("   \n  ");
        assert!(result2.is_err());
    }

    #[test]
    fn filter_workers_no_project_returns_all() {
        let workers = vec![
            WorkerSummary {
                worker_id: "ur-abc".into(),
                project_key: "ur".into(),
                ..Default::default()
            },
            WorkerSummary {
                worker_id: "foo-xyz".into(),
                project_key: "foo".into(),
                ..Default::default()
            },
        ];
        let result = filter_workers_by_project(workers, &None);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn filter_workers_by_project_filters_correctly() {
        let workers = vec![
            WorkerSummary {
                worker_id: "ur-abc".into(),
                project_key: "ur".into(),
                ..Default::default()
            },
            WorkerSummary {
                worker_id: "foo-xyz".into(),
                project_key: "foo".into(),
                ..Default::default()
            },
            WorkerSummary {
                worker_id: "ur-def".into(),
                project_key: "ur".into(),
                ..Default::default()
            },
        ];
        let result = filter_workers_by_project(workers, &Some("ur".to_string()));
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|w| w.project_key == "ur"));
    }
}
