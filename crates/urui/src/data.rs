use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{debug, error};

use ur_rpc::connection::connect;
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
use ur_rpc::proto::ticket::{
    ListTicketsRequest, ListTicketsResponse, ListWorkflowsRequest, ListWorkflowsResponse, Ticket,
    WorkflowInfo,
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
