use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::{Mutex, mpsc, watch};
use tracing::info;

use db_events::PgEventPoller;
use ur_rpc::proto::ticket::{UiEvent, UiEventBatch, UiEventType};

/// Polls two `ui_events` tables (ticket DB and workflow DB) via dedicated
/// `PgEventPoller` tasks and merges their event streams into a single
/// `UiEventBatch` channel consumed by the gRPC `SubscribeUiEvents` RPC.
///
/// Each database gets its own `PgEventPoller` with its own LISTEN/NOTIFY
/// connection. Events from both are forwarded to all registered listeners
/// over a single merged stream, so TUI consumers see no behavioral change.
#[derive(Clone)]
pub struct UiEventPoller {
    ticket_poller: PgEventPoller,
    workflow_poller: PgEventPoller,
    listeners: Arc<Mutex<Vec<mpsc::Sender<UiEventBatch>>>>,
}

impl UiEventPoller {
    pub fn new(
        ticket_pool: PgPool,
        ticket_url: String,
        workflow_pool: PgPool,
        workflow_url: String,
        fallback_interval: Duration,
    ) -> Self {
        Self {
            ticket_poller: PgEventPoller::new(ticket_pool, fallback_interval, ticket_url),
            workflow_poller: PgEventPoller::new(workflow_pool, fallback_interval, workflow_url),
            listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Register a new listener and return its receiver half.
    pub async fn add_listener(&self) -> mpsc::Receiver<UiEventBatch> {
        let (tx, rx) = mpsc::channel(64);
        self.listeners.lock().await.push(tx);
        rx
    }

    /// Spawn the merge loop and both underlying pollers as background tasks.
    pub fn spawn(&self, shutdown_rx: watch::Receiver<bool>) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        let shutdown_rx_clone = shutdown_rx.clone();
        tokio::spawn(async move {
            this.run(shutdown_rx, shutdown_rx_clone).await;
        })
    }

    async fn run(
        self,
        shutdown_rx_ticket: watch::Receiver<bool>,
        shutdown_rx_workflow: watch::Receiver<bool>,
    ) {
        info!("ui event poller started (ticket + workflow)");

        // Each poller gets its own internal mpsc channel.
        let mut ticket_rx = self.ticket_poller.subscribe().await;
        let mut workflow_rx = self.workflow_poller.subscribe().await;

        // Spawn the two underlying pollers.
        self.ticket_poller.spawn(shutdown_rx_ticket.clone());
        self.workflow_poller.spawn(shutdown_rx_workflow.clone());

        let mut shutdown_rx = shutdown_rx_ticket;

        loop {
            tokio::select! {
                batch = ticket_rx.recv() => {
                    match batch {
                        Some(events) => self.dispatch_batch(events).await,
                        None => {
                            info!("ticket ui event poller channel closed, shutting down merge loop");
                            return;
                        }
                    }
                }
                batch = workflow_rx.recv() => {
                    match batch {
                        Some(events) => self.dispatch_batch(events).await,
                        None => {
                            info!("workflow ui event poller channel closed, shutting down merge loop");
                            return;
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("ui event merge loop shutting down");
                        return;
                    }
                }
            }
        }
    }

    /// Convert raw `db_events::UiEvent` rows into a proto `UiEventBatch`
    /// and send to all registered listeners, pruning dead channels.
    async fn dispatch_batch(&self, events: Vec<db_events::UiEvent>) {
        if events.is_empty() {
            return;
        }

        let batch = UiEventBatch {
            events: events
                .into_iter()
                .map(|row| UiEvent {
                    entity_type: map_entity_type(&row.entity_type).into(),
                    entity_id: row.entity_id,
                })
                .collect(),
        };

        let mut listeners = self.listeners.lock().await;
        if listeners.is_empty() {
            return;
        }

        let mut alive = Vec::with_capacity(listeners.len());
        for tx in listeners.drain(..) {
            if tx.send(batch.clone()).await.is_ok() {
                alive.push(tx);
            }
        }
        *listeners = alive;
    }
}

/// Map a database `entity_type` string to the proto `UiEventType` enum.
fn map_entity_type(entity_type: &str) -> UiEventType {
    match entity_type {
        "ticket" => UiEventType::Ticket,
        "workflow" => UiEventType::Workflow,
        "worker" => UiEventType::Worker,
        _ => UiEventType::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_known_entity_types() {
        assert_eq!(map_entity_type("ticket"), UiEventType::Ticket);
        assert_eq!(map_entity_type("workflow"), UiEventType::Workflow);
        assert_eq!(map_entity_type("worker"), UiEventType::Worker);
    }

    #[test]
    fn map_unknown_entity_type() {
        assert_eq!(map_entity_type("other"), UiEventType::Unknown);
        assert_eq!(map_entity_type(""), UiEventType::Unknown);
    }
}
