use std::sync::Arc;
use std::time::Duration;

use sqlx::postgres::PgListener;
use tokio::sync::{Mutex, mpsc, watch};
use tracing::{error, info, warn};

use ur_rpc::proto::ticket::{UiEvent, UiEventBatch, UiEventType};
use workflow_db::UiEventRepo;

/// Channel name used by Postgres triggers to notify of new UI events.
const LISTEN_CHANNEL: &str = "ui_events";

/// Polls the `ui_events` table, waking on Postgres LISTEN/NOTIFY
/// notifications with a configurable fallback timeout.
///
/// Holds a dedicated Postgres LISTEN connection separate from the pool.
/// When the LISTEN connection drops, falls back to periodic polling at
/// the fallback interval and attempts to reconnect.
#[derive(Clone)]
pub struct UiEventPoller {
    repo: UiEventRepo,
    fallback_interval: Duration,
    database_url: String,
    listeners: Arc<Mutex<Vec<mpsc::Sender<UiEventBatch>>>>,
}

impl UiEventPoller {
    pub fn new(repo: UiEventRepo, fallback_interval: Duration, database_url: String) -> Self {
        Self {
            repo,
            fallback_interval,
            database_url,
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

        let mut pg_listener = self.try_connect_listener().await;
        if pg_listener.is_some() {
            info!("ui event poller: LISTEN connection established");
        } else {
            warn!("ui event poller: LISTEN connection failed, using fallback polling");
        }

        loop {
            self.poll_once().await;

            match self.wait_for_wake(&mut pg_listener, &mut shutdown_rx).await {
                WakeReason::Shutdown => {
                    info!("ui event poller shutting down");
                    return;
                }
                WakeReason::Notification => {}
                WakeReason::Timeout => {}
                WakeReason::ListenError => {
                    self.handle_listen_error(&mut pg_listener).await;
                }
            }
        }
    }

    /// Wait for a notification, fallback timeout, or shutdown signal.
    async fn wait_for_wake(
        &self,
        pg_listener: &mut Option<PgListener>,
        shutdown_rx: &mut watch::Receiver<bool>,
    ) -> WakeReason {
        match pg_listener {
            Some(listener) => {
                tokio::select! {
                    result = listener.recv() => {
                        match result {
                            Ok(_notification) => WakeReason::Notification,
                            Err(e) => {
                                warn!(error = %e, "LISTEN connection error");
                                WakeReason::ListenError
                            }
                        }
                    }
                    _ = tokio::time::sleep(self.fallback_interval) => {
                        WakeReason::Timeout
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            WakeReason::Shutdown
                        } else {
                            WakeReason::Timeout
                        }
                    }
                }
            }
            None => {
                tokio::select! {
                    _ = tokio::time::sleep(self.fallback_interval) => {
                        WakeReason::Timeout
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            WakeReason::Shutdown
                        } else {
                            WakeReason::Timeout
                        }
                    }
                }
            }
        }
    }

    /// Handle a LISTEN connection error: drop the broken connection and
    /// attempt to reconnect. If reconnection fails, the next loop iteration
    /// will fall back to periodic polling.
    async fn handle_listen_error(&self, pg_listener: &mut Option<PgListener>) {
        *pg_listener = None;
        let new_listener = self.try_connect_listener().await;
        if new_listener.is_some() {
            info!("ui event poller: LISTEN connection re-established");
        } else {
            warn!("ui event poller: LISTEN reconnect failed, continuing with fallback polling");
        }
        *pg_listener = new_listener;
    }

    /// Attempt to create a new PgListener and subscribe to the channel.
    /// Returns `None` on failure.
    async fn try_connect_listener(&self) -> Option<PgListener> {
        let mut listener = match PgListener::connect(&self.database_url).await {
            Ok(l) => l,
            Err(e) => {
                warn!(error = %e, "failed to create PgListener");
                return None;
            }
        };

        if let Err(e) = listener.listen(LISTEN_CHANNEL).await {
            warn!(error = %e, "failed to LISTEN on channel {}", LISTEN_CHANNEL);
            return None;
        }

        Some(listener)
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

        self.dispatch_batch(batch).await;
    }

    /// Send a batch to all listeners, removing dead channels.
    async fn dispatch_batch(&self, batch: UiEventBatch) {
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

enum WakeReason {
    Notification,
    Timeout,
    Shutdown,
    ListenError,
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
