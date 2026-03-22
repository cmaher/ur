use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, mpsc, watch};
use tracing::{error, info};

use ur_db::UiEventRepo;
use ur_rpc::proto::ticket::{UiEvent, UiEventBatch, UiEventType};

/// Polls the `ui_events` table on a configurable interval, dispatches batches
/// to registered listeners, and cleans up dead channels.
///
/// Follows the `GithubPollerManager` pattern: Clone-able manager with
/// `spawn(shutdown_rx)` and a `select! { sleep / shutdown }` loop.
#[derive(Clone)]
pub struct UiEventPoller {
    repo: UiEventRepo,
    poll_interval: Duration,
    listeners: Arc<Mutex<Vec<mpsc::Sender<UiEventBatch>>>>,
}

impl UiEventPoller {
    pub fn new(repo: UiEventRepo, poll_interval: Duration) -> Self {
        Self {
            repo,
            poll_interval,
            listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Register a new listener and return its receiver half.
    pub async fn add_listener(&self) -> mpsc::Receiver<UiEventBatch> {
        let (tx, rx) = mpsc::channel(64);
        self.listeners.lock().await.push(tx);
        rx
    }

    /// Spawn the polling loop as a background tokio task.
    pub fn spawn(&self, shutdown_rx: watch::Receiver<bool>) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(this.run(shutdown_rx))
    }

    async fn run(self, mut shutdown_rx: watch::Receiver<bool>) {
        info!("ui event poller started");
        loop {
            self.poll_once().await;

            tokio::select! {
                _ = tokio::time::sleep(self.poll_interval) => {}
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("ui event poller shutting down");
                        return;
                    }
                }
            }
        }
    }

    /// Run one poll cycle: read events, delete by max ID, send batch to listeners.
    async fn poll_once(&self) {
        let events = match self.repo.poll_ui_events().await {
            Ok(rows) => rows,
            Err(e) => {
                error!(error = %e, "failed to poll ui_events");
                return;
            }
        };

        if events.is_empty() {
            return;
        }

        let max_id = events.iter().map(|e| e.id).max().unwrap_or(0);

        let batch = UiEventBatch {
            events: events
                .into_iter()
                .map(|row| UiEvent {
                    entity_type: map_entity_type(&row.entity_type).into(),
                    entity_id: row.entity_id,
                })
                .collect(),
        };

        // Delete processed events regardless of listener count.
        if let Err(e) = self.repo.delete_ui_events(max_id).await {
            error!(error = %e, max_id = max_id, "failed to delete ui_events");
        }

        // Send batch to all listeners, removing dead channels.
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
