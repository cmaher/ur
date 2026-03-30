use tokio::sync::mpsc;
use tracing::{debug, error};

use super::cmd::{Cmd, FetchCmd};
use super::msg::{DataMsg, Msg};

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
}

impl CmdRunner {
    /// Create a new command runner that sends result messages through the given channel.
    pub fn new(
        msg_tx: mpsc::UnboundedSender<Msg>,
        port: u16,
        project_filter: Option<String>,
    ) -> Self {
        Self {
            msg_tx,
            port,
            project_filter,
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
        }
    }

    /// Execute a list of commands.
    pub fn execute_all(&self, cmds: Vec<Cmd>) {
        for cmd in cmds {
            self.execute(cmd);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_runner() -> (CmdRunner, mpsc::UnboundedReceiver<Msg>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let runner = CmdRunner::new(tx, 0, None);
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
