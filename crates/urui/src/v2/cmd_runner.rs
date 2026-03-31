use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};

use super::cmd::{Cmd, FetchCmd};
use super::msg::{
    DataMsg, FlowOpMsg, FlowOpResultMsg, Msg, TicketOpMsg, TicketOpResultMsg, UiEventItem,
    WorkerOpMsg, WorkerOpResultMsg,
};

/// Executes `Cmd` values produced by the update function and sends result
/// `Msg`s back through the event channel.
///
/// The command runner is the boundary between the pure TEA core and the
/// impure world of I/O and async operations. It holds a gRPC server port
/// and optional project filter for scoping data fetches.
#[derive(Clone)]
pub struct CmdRunner {
    msg_tx: mpsc::UnboundedSender<Msg>,
    port: u16,
    project_filter: Option<String>,
    config_dir: std::path::PathBuf,
}

impl CmdRunner {
    /// Create a new command runner that sends result messages through the given channel.
    pub fn new(
        msg_tx: mpsc::UnboundedSender<Msg>,
        port: u16,
        project_filter: Option<String>,
        config_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            msg_tx,
            port,
            project_filter,
            config_dir,
        }
    }

    /// Execute a command. Some commands (like `Quit`) produce messages synchronously;
    /// others will spawn async tasks that send messages when complete.
    pub fn execute(&self, cmd: Cmd) {
        match cmd {
            Cmd::None => {}
            Cmd::Quit => {
                let _ = self.msg_tx.send(Msg::Quit);
            }
            Cmd::Batch(cmds) => {
                for cmd in cmds {
                    self.execute(cmd);
                }
            }
            Cmd::Fetch(fetch) => self.execute_fetch(fetch),
            Cmd::SubscribeUiEvents => self.subscribe_ui_events(),
            Cmd::StopWorker { worker_id } => self.execute_stop_worker(worker_id),
            Cmd::TicketOp(op) => self.execute_ticket_op(op),
            Cmd::FlowOp(op) => self.execute_flow_op(op),
            Cmd::WorkerOp(op) => self.execute_worker_op(op),
            Cmd::FireDesktopNotification(notification) => {
                // Fire-and-forget: spawn a blocking task to avoid blocking
                // the async runtime with process spawn.
                tokio::task::spawn_blocking(move || {
                    super::notifications::fire_desktop_notification(&notification, None);
                });
            }
            Cmd::PersistTheme { theme_name } => self.persist_theme(theme_name),
        }
    }

    /// Execute a list of commands.
    pub fn execute_all(&self, cmds: Vec<Cmd>) {
        for cmd in cmds {
            self.execute(cmd);
        }
    }

    /// Persist the selected theme name to ur.toml in a background thread.
    /// Uses the project_filter to decide per-project vs global save.
    fn persist_theme(&self, theme_name: String) {
        let config_dir = self.config_dir.clone();
        let project = self.project_filter.clone();
        tokio::task::spawn_blocking(move || {
            let result = if let Some(ref key) = project {
                ur_config::save_project_theme_name(&config_dir, key, &theme_name)
            } else {
                ur_config::save_theme_name(&config_dir, &theme_name)
            };
            if let Err(e) = result {
                warn!("failed to persist theme to ur.toml: {e}");
            }
        });
    }

    /// Spawn a long-lived background task that subscribes to the server's UI
    /// event stream and forwards batches as `Msg::UiEvent`.
    fn subscribe_ui_events(&self) {
        let tx = self.msg_tx.clone();
        let port = self.port;

        tokio::spawn(async move {
            debug!(port, "v2: subscribing to UI event stream");
            if let Err(e) = consume_ui_event_stream(port, &tx).await {
                warn!(port, error = %e, "v2: UI event stream disconnected");
            }
        });
    }

    /// Spawn an async task to stop a worker via the `WorkerStop` gRPC call.
    fn execute_stop_worker(&self, worker_id: String) {
        let tx = self.msg_tx.clone();
        let port = self.port;

        tokio::spawn(async move {
            debug!(port, %worker_id, "v2: stopping worker");
            let result = stop_worker(port, &worker_id).await;
            let _ = tx.send(Msg::Data(Box::new(DataMsg::WorkerStopped {
                worker_id,
                result,
            })));
        });
    }

    /// Route a ticket operation to the appropriate handler method.
    fn execute_ticket_op(&self, op: TicketOpMsg) {
        match op {
            TicketOpMsg::Dispatch {
                ticket_id,
                project_key,
                image_id,
            }
            | TicketOpMsg::DispatchAll {
                ticket_id,
                project_key,
                image_id,
            } => self.exec_dispatch(ticket_id, project_key, image_id),
            TicketOpMsg::Close { ticket_id } => self.exec_close(ticket_id),
            TicketOpMsg::ForceClose { ticket_id } => self.exec_force_close(ticket_id),
            TicketOpMsg::SetPriority {
                ticket_id,
                priority,
            } => self.exec_set_priority(ticket_id, priority),
            TicketOpMsg::Create { pending } => self.exec_create(pending),
            TicketOpMsg::CreateAndDispatch {
                pending,
                project_key,
                image_id,
            } => self.exec_create_and_dispatch(pending, project_key, image_id),
            TicketOpMsg::CreateAndDesign {
                pending,
                project_key,
                image_id,
            } => self.exec_create_and_design(pending, project_key, image_id),
            TicketOpMsg::LaunchDesign {
                ticket_id,
                project_key,
                image_id,
            } => self.exec_launch_design(ticket_id, project_key, image_id),
            TicketOpMsg::Redrive { ticket_id } => self.exec_redrive(ticket_id),
            TicketOpMsg::Open { ticket_id } => self.exec_open(ticket_id),
        }
    }

    fn exec_dispatch(&self, ticket_id: String, project_key: String, image_id: String) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        tokio::spawn(async move {
            debug!(port, %ticket_id, "v2: dispatching ticket");
            let result = dispatch_ticket(port, &ticket_id, &project_key, &image_id).await;
            let msg = TicketOpResultMsg::Dispatched {
                result: result.map(|()| format!("Dispatched {ticket_id}")),
            };
            let _ = tx.send(Msg::TicketOpResult(msg));
        });
    }

    fn exec_close(&self, ticket_id: String) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        tokio::spawn(async move {
            debug!(port, %ticket_id, "v2: closing ticket");
            let result = update_ticket_status(port, &ticket_id, "closed", false).await;
            let msg = TicketOpResultMsg::Closed {
                result: result.map(|()| format!("{ticket_id} → closed")),
            };
            let _ = tx.send(Msg::TicketOpResult(msg));
        });
    }

    fn exec_open(&self, ticket_id: String) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        tokio::spawn(async move {
            debug!(port, %ticket_id, "v2: reopening ticket");
            let result = update_ticket_status(port, &ticket_id, "open", false).await;
            let msg = TicketOpResultMsg::Opened {
                result: result.map(|()| format!("{ticket_id} → open")),
            };
            let _ = tx.send(Msg::TicketOpResult(msg));
        });
    }

    fn exec_force_close(&self, ticket_id: String) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        tokio::spawn(async move {
            debug!(port, %ticket_id, "v2: force-closing ticket");
            let result = update_ticket_status(port, &ticket_id, "closed", true).await;
            let msg = TicketOpResultMsg::ForceClosed {
                result: result.map(|()| format!("{ticket_id} → closed (force)")),
            };
            let _ = tx.send(Msg::TicketOpResult(msg));
        });
    }

    fn exec_set_priority(&self, ticket_id: String, priority: i64) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        tokio::spawn(async move {
            debug!(port, %ticket_id, priority, "v2: setting ticket priority");
            let result = update_ticket_priority(port, &ticket_id, priority).await;
            let msg = TicketOpResultMsg::PrioritySet {
                result: result.map(|()| format!("Priority set to P{priority} for {ticket_id}")),
            };
            let _ = tx.send(Msg::TicketOpResult(msg));
        });
    }

    fn exec_create(&self, pending: super::msg::PendingTicket) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        tokio::spawn(async move {
            debug!(port, project = %pending.project, "v2: creating ticket");
            let result = create_ticket(port, &pending).await;
            let msg = TicketOpResultMsg::Created {
                result: result.map(|id| format!("Created {id}")),
            };
            let _ = tx.send(Msg::TicketOpResult(msg));
        });
    }

    fn exec_create_and_dispatch(
        &self,
        pending: super::msg::PendingTicket,
        project_key: String,
        image_id: String,
    ) {
        let tx = self.msg_tx.clone();
        let tx2 = tx.clone();
        let port = self.port;
        tokio::spawn(async move {
            debug!(port, project = %pending.project, "v2: creating and dispatching ticket");
            let result = create_and_dispatch(port, &pending, &project_key, &image_id, &tx2).await;
            let msg = TicketOpResultMsg::Created {
                result: result.map(|id| format!("Created and dispatched {id}")),
            };
            let _ = tx.send(Msg::TicketOpResult(msg));
        });
    }

    fn exec_create_and_design(
        &self,
        pending: super::msg::PendingTicket,
        project_key: String,
        image_id: String,
    ) {
        let tx = self.msg_tx.clone();
        let tx2 = tx.clone();
        let port = self.port;
        tokio::spawn(async move {
            debug!(port, project = %pending.project, "v2: creating ticket with design worker");
            let result = create_and_design(port, &pending, &project_key, &image_id, &tx2).await;
            let msg = TicketOpResultMsg::Created {
                result: result.map(|id| format!("Created {id} with design worker")),
            };
            let _ = tx.send(Msg::TicketOpResult(msg));
        });
    }

    fn exec_launch_design(&self, ticket_id: String, project_key: String, image_id: String) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        tokio::spawn(async move {
            debug!(port, %ticket_id, "v2: launching design worker");
            let result = launch_design_worker(port, &ticket_id, &project_key, &image_id).await;
            let msg = TicketOpResultMsg::DesignLaunched {
                result: result.map(|()| format!("Launched design worker for {ticket_id}")),
            };
            let _ = tx.send(Msg::TicketOpResult(msg));
        });
    }

    fn exec_redrive(&self, ticket_id: String) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        tokio::spawn(async move {
            debug!(port, %ticket_id, "v2: redriving ticket");
            let result = redrive_ticket(port, &ticket_id).await;
            let msg = TicketOpResultMsg::Redriven {
                result: result.map(|()| format!("Moved {ticket_id} to Verify")),
            };
            let _ = tx.send(Msg::TicketOpResult(msg));
        });
    }

    /// Route a flow operation to the appropriate handler method.
    fn execute_flow_op(&self, op: FlowOpMsg) {
        match op {
            FlowOpMsg::Cancel { ticket_id } => self.exec_flow_cancel(ticket_id),
        }
    }

    fn exec_flow_cancel(&self, ticket_id: String) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        tokio::spawn(async move {
            debug!(port, %ticket_id, "v2: cancelling workflow");
            let result = cancel_workflow(port, &ticket_id).await;
            let msg = FlowOpResultMsg::Cancelled {
                result: result.map(|()| format!("Cancelled workflow for {ticket_id}")),
            };
            let _ = tx.send(Msg::FlowOpResult(msg));
        });
    }

    /// Route a worker operation to the appropriate handler method.
    fn execute_worker_op(&self, op: WorkerOpMsg) {
        match op {
            WorkerOpMsg::Kill { worker_id } => self.exec_worker_kill(worker_id),
        }
    }

    fn exec_worker_kill(&self, worker_id: String) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        tokio::spawn(async move {
            debug!(port, %worker_id, "v2: killing worker");
            let result = stop_worker(port, &worker_id).await;
            let msg = WorkerOpResultMsg::Killed {
                result: result.map(|()| format!("Killed worker {worker_id}")),
            };
            let _ = tx.send(Msg::WorkerOpResult(msg));
        });
    }

    /// Spawn an async task to execute a data-fetching command via gRPC.
    fn execute_fetch(&self, fetch: FetchCmd) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        let project_filter = self.project_filter.clone();

        match fetch {
            FetchCmd::Tickets {
                page_size,
                offset,
                include_children,
                statuses,
            } => {
                tokio::spawn(async move {
                    let result = fetch_tickets(
                        port,
                        project_filter,
                        page_size,
                        offset,
                        include_children,
                        statuses,
                    )
                    .await;
                    let _ = tx.send(Msg::Data(Box::new(DataMsg::TicketsLoaded(result))));
                });
            }
            FetchCmd::TicketDetail {
                ticket_id,
                child_page_size,
                child_offset,
                child_status_filter,
            } => {
                tokio::spawn(async move {
                    let result = fetch_ticket_detail(
                        port,
                        &ticket_id,
                        child_page_size,
                        child_offset,
                        child_status_filter,
                    )
                    .await;
                    let _ = tx.send(Msg::Data(Box::new(DataMsg::DetailLoaded(Box::new(result)))));
                });
            }
            FetchCmd::Flows { page_size, offset } => {
                tokio::spawn(async move {
                    let result = fetch_flows(port, page_size, offset, project_filter).await;
                    let _ = tx.send(Msg::Data(Box::new(DataMsg::FlowsLoaded(result))));
                });
            }
            FetchCmd::Workers => {
                tokio::spawn(async move {
                    let result = fetch_workers(port, project_filter).await;
                    let _ = tx.send(Msg::Data(Box::new(DataMsg::WorkersLoaded(result))));
                });
            }
            FetchCmd::Activities {
                ticket_id,
                author_filter,
            } => {
                let tid = ticket_id.clone();
                tokio::spawn(async move {
                    let result = fetch_activities(port, &tid, author_filter).await;
                    let _ = tx.send(Msg::Data(Box::new(DataMsg::ActivitiesLoaded {
                        ticket_id,
                        result,
                    })));
                });
            }
        }
    }
}

/// Fetch tickets via `ListTickets` gRPC. Reuses the same RPC logic as v1's
/// `DataManager::fetch_tickets`.
async fn fetch_tickets(
    port: u16,
    project: Option<String>,
    page_size: Option<i32>,
    offset: Option<i32>,
    include_children: Option<bool>,
    statuses: Vec<String>,
) -> Result<(Vec<ur_rpc::proto::ticket::Ticket>, i32), String> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::ticket::ListTicketsRequest;
    use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

    debug!(port, "v2: fetching tickets");
    let status = if statuses.is_empty() {
        None
    } else {
        Some(statuses.join(","))
    };
    let channel = connect(port).await.map_err(|e| e.to_string())?;
    let mut client = TicketServiceClient::new(channel);
    let resp = client
        .list_tickets(ListTicketsRequest {
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
        })
        .await
        .map_err(|e| {
            error!(port, error = %e, "v2: ticket fetch failed");
            e.to_string()
        })?;
    let inner = resp.into_inner();
    Ok((inner.tickets, inner.total_count))
}

/// Fetch full ticket detail (ticket + children) via concurrent `GetTicket`
/// and `ListTickets` RPCs.
async fn fetch_ticket_detail(
    port: u16,
    ticket_id: &str,
    child_page_size: Option<i32>,
    child_offset: Option<i32>,
    child_status_filter: Option<String>,
) -> Result<
    (
        ur_rpc::proto::ticket::GetTicketResponse,
        Vec<ur_rpc::proto::ticket::Ticket>,
        i32,
    ),
    String,
> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
    use ur_rpc::proto::ticket::{GetTicketRequest, ListTicketsRequest};

    debug!(port, %ticket_id, "v2: fetching ticket detail");
    let tid_get = ticket_id.to_owned();
    let tid_children = ticket_id.to_owned();

    let get_fut = async move {
        let channel = connect(port).await?;
        let mut client = TicketServiceClient::new(channel);
        let resp = client
            .get_ticket(GetTicketRequest {
                id: tid_get,
                activity_author_filter: None,
            })
            .await?;
        anyhow::Ok(resp.into_inner())
    };

    let list_fut = async move {
        let channel = connect(port).await?;
        let mut client = TicketServiceClient::new(channel);
        let resp = client
            .list_tickets(ListTicketsRequest {
                project: None,
                ticket_type: None,
                status: child_status_filter,
                meta_key: None,
                meta_value: None,
                tree_root_id: None,
                page_size: child_page_size,
                offset: child_offset,
                include_children: None,
                parent_id: Some(tid_children),
            })
            .await?;
        let inner = resp.into_inner();
        anyhow::Ok((inner.tickets, inner.total_count))
    };

    let (detail, (children, total)) = tokio::try_join!(get_fut, list_fut).map_err(|e| {
        error!(port, %ticket_id, error = %e, "v2: ticket detail fetch failed");
        e.to_string()
    })?;
    Ok((detail, children, total))
}

/// Fetch workflows via `ListWorkflows` gRPC.
async fn fetch_flows(
    port: u16,
    page_size: Option<i32>,
    offset: Option<i32>,
    project: Option<String>,
) -> Result<(Vec<ur_rpc::proto::ticket::WorkflowInfo>, i32), String> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::ticket::ListWorkflowsRequest;
    use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

    debug!(port, "v2: fetching workflows");
    let channel = connect(port).await.map_err(|e| e.to_string())?;
    let mut client = TicketServiceClient::new(channel);
    let resp = client
        .list_workflows(ListWorkflowsRequest {
            status: None,
            page_size,
            offset,
            project,
        })
        .await
        .map_err(|e| {
            error!(port, error = %e, "v2: workflow fetch failed");
            e.to_string()
        })?;
    let inner = resp.into_inner();
    Ok((inner.workflows, inner.total_count))
}

/// Fetch workers via `WorkerList` gRPC, filtered by project if set.
async fn fetch_workers(
    port: u16,
    project_filter: Option<String>,
) -> Result<Vec<ur_rpc::proto::core::WorkerSummary>, String> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::core::WorkerListRequest;
    use ur_rpc::proto::core::core_service_client::CoreServiceClient;

    debug!(port, "v2: fetching workers");
    let channel = connect(port).await.map_err(|e| e.to_string())?;
    let mut client = CoreServiceClient::new(channel);
    let resp = client
        .worker_list(WorkerListRequest {})
        .await
        .map_err(|e| {
            error!(port, error = %e, "v2: worker fetch failed");
            e.to_string()
        })?;
    let mut workers = resp.into_inner().workers;
    if let Some(ref proj) = project_filter {
        workers.retain(|w| w.project_key == *proj);
    }
    Ok(workers)
}

/// Stop a worker via `WorkerStop` gRPC.
async fn stop_worker(port: u16, worker_id: &str) -> Result<(), String> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::core::WorkerStopRequest;
    use ur_rpc::proto::core::core_service_client::CoreServiceClient;

    debug!(port, %worker_id, "v2: stopping worker via RPC");
    let channel = connect(port).await.map_err(|e| e.to_string())?;
    let mut client = CoreServiceClient::new(channel);
    client
        .worker_stop(WorkerStopRequest {
            worker_id: worker_id.to_owned(),
        })
        .await
        .map_err(|e| {
            error!(port, %worker_id, error = %e, "v2: worker stop failed");
            e.to_string()
        })?;
    Ok(())
}

/// Fetch activities for a specific ticket via `GetTicket` with optional
/// author filter.
async fn fetch_activities(
    port: u16,
    ticket_id: &str,
    author_filter: Option<String>,
) -> Result<Vec<ur_rpc::proto::ticket::ActivityEntry>, String> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::ticket::GetTicketRequest;
    use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

    debug!(port, %ticket_id, "v2: fetching activities");
    let channel = connect(port).await.map_err(|e| e.to_string())?;
    let mut client = TicketServiceClient::new(channel);
    let resp = client
        .get_ticket(GetTicketRequest {
            id: ticket_id.to_owned(),
            activity_author_filter: author_filter,
        })
        .await
        .map_err(|e| {
            error!(port, %ticket_id, error = %e, "v2: activities fetch failed");
            e.to_string()
        })?;
    Ok(resp.into_inner().activities)
}

/// Connect to the UI event stream and forward batches as `Msg::UiEvent`.
///
/// Reuses the same gRPC `SubscribeUiEvents` RPC as v1. Each batch of events
/// is converted to `UiEventItem` values and sent through the message channel.
async fn consume_ui_event_stream(port: u16, tx: &mpsc::UnboundedSender<Msg>) -> Result<(), String> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::ticket::SubscribeUiEventsRequest;
    use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

    let channel = connect(port).await.map_err(|e| e.to_string())?;
    let mut client = TicketServiceClient::new(channel);
    let response = client
        .subscribe_ui_events(SubscribeUiEventsRequest {})
        .await
        .map_err(|e| e.to_string())?;
    let mut stream = response.into_inner();
    info!(port, "v2: UI event stream connected successfully");

    while let Some(batch) = stream.message().await.map_err(|e| e.to_string())? {
        let items: Vec<UiEventItem> = batch
            .events
            .into_iter()
            .map(|ev| UiEventItem {
                entity_type: ui_event_type_to_str(ev.entity_type()),
                entity_id: ev.entity_id,
            })
            .collect();
        trace!(batch_size = items.len(), "v2: received UI event batch");
        if !items.is_empty() && tx.send(Msg::UiEvent(items)).is_err() {
            break;
        }
    }
    Ok(())
}

/// Convert a proto UiEventType enum value to a string label.
fn ui_event_type_to_str(t: ur_rpc::proto::ticket::UiEventType) -> String {
    use ur_rpc::proto::ticket::UiEventType;
    match t {
        UiEventType::Ticket => "ticket".to_owned(),
        UiEventType::Workflow => "workflow".to_owned(),
        UiEventType::Worker => "worker".to_owned(),
        UiEventType::Unknown => "unknown".to_owned(),
    }
}

/// Dispatch a ticket: create workflow + launch worker container.
///
/// Checks if the ticket is a design type first; if so, routes through
/// the design worker launch path instead.
async fn dispatch_ticket(
    port: u16,
    ticket_id: &str,
    project_key: &str,
    image_id: &str,
) -> Result<(), String> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::core::WorkerLaunchRequest;
    use ur_rpc::proto::core::core_service_client::CoreServiceClient;
    use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
    use ur_rpc::proto::ticket::{CreateWorkflowRequest, GetTicketRequest};

    // Check if design ticket
    let channel = connect(port).await.map_err(|e| e.to_string())?;
    let mut client = TicketServiceClient::new(channel);
    let resp = client
        .get_ticket(GetTicketRequest {
            id: ticket_id.to_owned(),
            activity_author_filter: None,
        })
        .await
        .map_err(|e| e.to_string())?;
    let is_design = resp
        .into_inner()
        .ticket
        .map(|t| t.ticket_type == "design")
        .unwrap_or(false);

    if is_design {
        return launch_design_worker(port, ticket_id, project_key, image_id).await;
    }

    // Create workflow
    let channel = connect(port).await.map_err(|e| e.to_string())?;
    let mut ticket_client = TicketServiceClient::new(channel);
    ticket_client
        .create_workflow(CreateWorkflowRequest {
            ticket_id: ticket_id.to_owned(),
            status: ur_rpc::lifecycle::AWAITING_DISPATCH.to_owned(),
        })
        .await
        .map_err(|e| e.to_string())?;

    // Launch worker
    let channel = connect(port).await.map_err(|e| e.to_string())?;
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
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Update a ticket's status, optionally forcing recursive close of children.
async fn update_ticket_status(
    port: u16,
    ticket_id: &str,
    status: &str,
    force: bool,
) -> Result<(), String> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::ticket::UpdateTicketRequest;
    use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

    let channel = connect(port).await.map_err(|e| e.to_string())?;
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
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Update a ticket's priority.
async fn update_ticket_priority(port: u16, ticket_id: &str, priority: i64) -> Result<(), String> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::ticket::UpdateTicketRequest;
    use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

    let channel = connect(port).await.map_err(|e| e.to_string())?;
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
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Create a ticket, returning the new ticket ID.
async fn create_ticket(port: u16, pending: &super::msg::PendingTicket) -> Result<String, String> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::ticket::CreateTicketRequest;
    use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

    let channel = connect(port).await.map_err(|e| e.to_string())?;
    let mut client = TicketServiceClient::new(channel);
    let resp = client
        .create_ticket(CreateTicketRequest {
            project: pending.project.clone(),
            ticket_type: "task".to_owned(),
            status: String::new(),
            priority: pending.priority,
            parent_id: pending.parent_id.clone(),
            title: pending.title.clone(),
            body: String::new(),
            id: None,
            created_at: None,
            wip: false,
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(resp.into_inner().id)
}

/// Create a ticket and dispatch it (create workflow + launch worker).
async fn create_and_dispatch(
    port: u16,
    pending: &super::msg::PendingTicket,
    project_key: &str,
    image_id: &str,
    tx: &tokio::sync::mpsc::UnboundedSender<Msg>,
) -> Result<String, String> {
    let ticket_id = create_ticket(port, pending).await?;
    let _ = tx.send(Msg::StatusShow(format!(
        "Launching worker for {ticket_id}..."
    )));
    dispatch_ticket(port, &ticket_id, project_key, image_id).await?;
    Ok(ticket_id)
}

/// Create a ticket and launch a design worker for it.
async fn create_and_design(
    port: u16,
    pending: &super::msg::PendingTicket,
    project_key: &str,
    image_id: &str,
    tx: &tokio::sync::mpsc::UnboundedSender<Msg>,
) -> Result<String, String> {
    let ticket_id = create_ticket(port, pending).await?;
    let _ = tx.send(Msg::StatusShow(format!(
        "Launching design worker for {ticket_id}..."
    )));
    launch_design_worker(port, &ticket_id, project_key, image_id).await?;
    Ok(ticket_id)
}

/// Launch a design worker for an existing ticket (no workflow).
fn launch_design_worker(
    port: u16,
    ticket_id: &str,
    project_key: &str,
    image_id: &str,
) -> impl std::future::Future<Output = Result<(), String>> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::core::WorkerLaunchRequest;
    use ur_rpc::proto::core::core_service_client::CoreServiceClient;

    let ticket_id = ticket_id.to_owned();
    let project_key = project_key.to_owned();
    let image_id = image_id.to_owned();

    async move {
        let channel = connect(port).await.map_err(|e| e.to_string())?;
        let mut core_client = CoreServiceClient::new(channel);
        core_client
            .worker_launch(WorkerLaunchRequest {
                worker_id: ticket_id,
                image_id,
                cpus: 2,
                memory: "8G".into(),
                workspace_dir: String::new(),
                claude_credentials: String::new(),
                mode: "design".to_owned(),
                skills: Vec::new(),
                project_key,
                context_repos: vec![],
                dispatch: true,
            })
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Redrive a ticket to verifying status.
async fn redrive_ticket(port: u16, ticket_id: &str) -> Result<(), String> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::ticket::RedriveTicketRequest;
    use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

    let channel = connect(port).await.map_err(|e| e.to_string())?;
    let mut client = TicketServiceClient::new(channel);
    client
        .redrive_ticket(RedriveTicketRequest {
            id: ticket_id.to_owned(),
            to_status: "verifying".to_owned(),
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Cancel the active workflow for a ticket via `CancelWorkflow` gRPC.
async fn cancel_workflow(port: u16, ticket_id: &str) -> Result<(), String> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::ticket::CancelWorkflowRequest;
    use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

    let channel = connect(port).await.map_err(|e| e.to_string())?;
    let mut client = TicketServiceClient::new(channel);
    client
        .cancel_workflow(CancelWorkflowRequest {
            ticket_id: ticket_id.to_owned(),
        })
        .await
        .map_err(|e| {
            error!(port, %ticket_id, error = %e, "v2: workflow cancel failed");
            e.to_string()
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_runner() -> (CmdRunner, mpsc::UnboundedReceiver<Msg>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let runner = CmdRunner::new(tx, 0, None, std::path::PathBuf::from("/tmp"));
        (runner, rx)
    }

    #[tokio::test]
    async fn quit_cmd_sends_quit_msg() {
        let (runner, mut rx) = make_runner();

        runner.execute(Cmd::Quit);

        let msg = rx.recv().await.unwrap();
        assert!(matches!(msg, Msg::Quit));
    }

    #[tokio::test]
    async fn none_cmd_sends_nothing() {
        let (runner, mut rx) = make_runner();

        runner.execute(Cmd::None);
        drop(runner);

        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn batch_cmd_executes_all() {
        let (runner, mut rx) = make_runner();

        runner.execute(Cmd::Batch(vec![Cmd::Quit, Cmd::Quit]));
        drop(runner);

        let msg1 = rx.recv().await.unwrap();
        let msg2 = rx.recv().await.unwrap();
        assert!(matches!(msg1, Msg::Quit));
        assert!(matches!(msg2, Msg::Quit));
    }

    #[tokio::test]
    async fn fetch_tickets_sends_error_when_server_unavailable() {
        let (runner, mut rx) = make_runner();

        runner.execute(Cmd::Fetch(FetchCmd::Tickets {
            page_size: None,
            offset: None,
            include_children: None,
            statuses: vec![],
        }));

        let msg = rx.recv().await.unwrap();
        match msg {
            Msg::Data(data) => match *data {
                DataMsg::TicketsLoaded(Err(_)) => {} // expected
                other => panic!("expected Tickets error, got {other:?}"),
            },
            other => panic!("expected Data msg, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_workers_sends_error_when_server_unavailable() {
        let (runner, mut rx) = make_runner();

        runner.execute(Cmd::Fetch(FetchCmd::Workers));

        let msg = rx.recv().await.unwrap();
        match msg {
            Msg::Data(data) => match *data {
                DataMsg::WorkersLoaded(Err(_)) => {}
                other => panic!("expected WorkersLoaded error, got {other:?}"),
            },
            other => panic!("expected Data msg, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_flows_sends_error_when_server_unavailable() {
        let (runner, mut rx) = make_runner();

        runner.execute(Cmd::Fetch(FetchCmd::Flows {
            page_size: None,
            offset: None,
        }));

        let msg = rx.recv().await.unwrap();
        match msg {
            Msg::Data(data) => match *data {
                DataMsg::FlowsLoaded(Err(_)) => {}
                other => panic!("expected FlowsLoaded error, got {other:?}"),
            },
            other => panic!("expected Data msg, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_detail_sends_error_when_server_unavailable() {
        let (runner, mut rx) = make_runner();

        runner.execute(Cmd::Fetch(FetchCmd::TicketDetail {
            ticket_id: "ur-test".to_string(),
            child_page_size: None,
            child_offset: None,
            child_status_filter: None,
        }));

        let msg = rx.recv().await.unwrap();
        match msg {
            Msg::Data(data) => match *data {
                DataMsg::DetailLoaded(result) => assert!(result.is_err()),
                other => panic!("expected DetailLoaded error, got {other:?}"),
            },
            other => panic!("expected Data msg, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_activities_sends_error_when_server_unavailable() {
        let (runner, mut rx) = make_runner();

        runner.execute(Cmd::Fetch(FetchCmd::Activities {
            ticket_id: "ur-test".to_string(),
            author_filter: None,
        }));

        let msg = rx.recv().await.unwrap();
        match msg {
            Msg::Data(data) => match *data {
                DataMsg::ActivitiesLoaded { ticket_id, result } => {
                    assert_eq!(ticket_id, "ur-test");
                    assert!(result.is_err());
                }
                other => panic!("expected ActivitiesLoaded error, got {other:?}"),
            },
            other => panic!("expected Data msg, got {other:?}"),
        }
    }
}
