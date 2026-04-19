// db_events: Shared pg_notify poller, ui_events schema snippet, and channel name constants.
// Consumed by ticket_db and workflow_db.

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tokio::sync::{Mutex, mpsc, watch};
use tracing::{error, info, warn};

/// Postgres LISTEN/NOTIFY channel used by all ui_events triggers.
pub const UI_EVENTS_CHANNEL: &str = "ui_events";

/// DDL for the `ui_events` table.
///
/// Both `ticket_db/migrations/001_initial.sql` and
/// `workflow_db/migrations/001_initial.sql` embed this verbatim (copy-paste).
/// The canonical definition lives here; the migrations are the authoritative
/// applied copy. See `db_events/CLAUDE.md` for rationale.
pub const UI_EVENTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS ui_events (
    id BIGSERIAL PRIMARY KEY,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (now()::TEXT)
);
"#;

/// A single row from the `ui_events` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiEvent {
    pub id: i64,
    pub entity_type: String,
    pub entity_id: String,
    pub created_at: String,
}

/// Generic Postgres LISTEN/NOTIFY poller for the `ui_events` table.
///
/// Holds a dedicated PgListener connection separate from the pool. On each
/// wake (notification or fallback timeout), it drains all buffered rows,
/// deletes them, and dispatches the batch to registered mpsc receivers.
///
/// Each database crate (`ticket_db`, `workflow_db`) instantiates its own
/// `PgEventPoller` against its own `PgPool` and database URL.
#[derive(Clone)]
pub struct PgEventPoller {
    pool: PgPool,
    fallback_interval: Duration,
    database_url: String,
    listeners: Arc<Mutex<Vec<mpsc::Sender<Vec<UiEvent>>>>>,
}

impl PgEventPoller {
    pub fn new(pool: PgPool, fallback_interval: Duration, database_url: String) -> Self {
        Self {
            pool,
            fallback_interval,
            database_url,
            listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Register a new listener and return its receiver half.
    pub async fn subscribe(&self) -> mpsc::Receiver<Vec<UiEvent>> {
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
        info!("PgEventPoller started on channel '{}'", UI_EVENTS_CHANNEL);

        let mut pg_listener = self.try_connect_listener().await;
        if pg_listener.is_some() {
            info!("PgEventPoller: LISTEN connection established");
        } else {
            warn!("PgEventPoller: LISTEN connection failed, using fallback polling");
        }

        loop {
            self.poll_once().await;

            match self.wait_for_wake(&mut pg_listener, &mut shutdown_rx).await {
                WakeReason::Shutdown => {
                    info!("PgEventPoller shutting down");
                    return;
                }
                WakeReason::Notification | WakeReason::Timeout => {}
                WakeReason::ListenError => {
                    self.handle_listen_error(&mut pg_listener).await;
                }
            }
        }
    }

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
                            Ok(_) => WakeReason::Notification,
                            Err(e) => {
                                warn!(error = %e, "LISTEN connection error");
                                WakeReason::ListenError
                            }
                        }
                    }
                    _ = tokio::time::sleep(self.fallback_interval) => WakeReason::Timeout,
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
                    _ = tokio::time::sleep(self.fallback_interval) => WakeReason::Timeout,
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

    async fn handle_listen_error(&self, pg_listener: &mut Option<PgListener>) {
        *pg_listener = None;
        let new_listener = self.try_connect_listener().await;
        if new_listener.is_some() {
            info!("PgEventPoller: LISTEN connection re-established");
        } else {
            warn!("PgEventPoller: LISTEN reconnect failed, continuing with fallback polling");
        }
        *pg_listener = new_listener;
    }

    async fn try_connect_listener(&self) -> Option<PgListener> {
        let mut listener = match PgListener::connect(&self.database_url).await {
            Ok(l) => l,
            Err(e) => {
                warn!(error = %e, "failed to create PgListener");
                return None;
            }
        };

        if let Err(e) = listener.listen(UI_EVENTS_CHANNEL).await {
            warn!(error = %e, "failed to LISTEN on channel '{}'", UI_EVENTS_CHANNEL);
            return None;
        }

        Some(listener)
    }

    /// Run one poll cycle: read events, delete by max id, dispatch to listeners.
    async fn poll_once(&self) {
        let events = match self.fetch_events().await {
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

        if let Err(e) = self.delete_events(max_id).await {
            error!(error = %e, max_id = max_id, "failed to delete ui_events");
        }

        self.dispatch_batch(events).await;
    }

    async fn fetch_events(&self) -> Result<Vec<UiEvent>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (i64, String, String, String)>(
            "SELECT id, entity_type, entity_id, created_at FROM ui_events ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(id, entity_type, entity_id, created_at)| UiEvent {
                id,
                entity_type,
                entity_id,
                created_at,
            })
            .collect())
    }

    async fn delete_events(&self, max_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM ui_events WHERE id <= $1")
            .bind(max_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Send the batch to all registered listeners, pruning dead channels.
    async fn dispatch_batch(&self, batch: Vec<UiEvent>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::str::FromStr;
    use std::time::Duration;
    use tokio::sync::watch;
    use uuid::Uuid;

    /// Connect to the CI postgres instance (localhost:5433).
    async fn admin_pool() -> sqlx::PgPool {
        let opts = PgConnectOptions::from_str("postgres://ur:ur@localhost:5433/postgres")
            .expect("invalid connection string");
        PgPoolOptions::new()
            .max_connections(2)
            .connect_with(opts)
            .await
            .expect("Cannot connect to ci-postgres on localhost:5433. Run: cargo make test:init")
    }

    /// Create an isolated test database, apply `ui_events` DDL, return (pool, db_name, db_url).
    async fn setup_test_db(admin: &sqlx::PgPool) -> (sqlx::PgPool, String, String) {
        let db_name = format!("db_events_test_{}", Uuid::new_v4().simple());

        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE \"{db_name}\""
        )))
        .execute(admin)
        .await
        .expect("failed to create test database");

        let db_url = format!("postgres://ur:ur@localhost:5433/{db_name}");
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&db_url)
            .await
            .expect("failed to connect to test database");

        // Apply ui_events table DDL.
        sqlx::query(UI_EVENTS_DDL)
            .execute(&pool)
            .await
            .expect("failed to create ui_events table");

        (pool, db_name, db_url)
    }

    async fn teardown_test_db(pool: sqlx::PgPool, admin: &sqlx::PgPool, db_name: &str) {
        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
        )))
        .execute(admin)
        .await
        .expect("failed to drop test database");
    }

    /// Verifies the full round-trip: direct INSERT into ui_events + pg_notify
    /// wakes the PgEventPoller and delivers the event via mpsc.
    #[tokio::test]
    async fn poller_delivers_event_on_pg_notify() {
        let admin = admin_pool().await;
        let (pool, db_name, db_url) = setup_test_db(&admin).await;

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let poller = PgEventPoller::new(pool.clone(), Duration::from_secs(5), db_url.clone());
        let mut rx = poller.subscribe().await;
        poller.spawn(shutdown_rx);

        // Insert a row, then explicitly notify to wake the poller.
        // Two separate execute calls because sqlx only runs the first statement.
        sqlx::query("INSERT INTO ui_events (entity_type, entity_id) VALUES ('ticket', 'test-1')")
            .execute(&pool)
            .await
            .expect("failed to insert ui event");
        sqlx::query("SELECT pg_notify('ui_events', '')")
            .execute(&pool)
            .await
            .expect("failed to send pg_notify");

        // Wait up to 3 seconds for the poller to deliver the event.
        let batch = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("timed out waiting for event from poller")
            .expect("channel closed unexpectedly");

        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].entity_type, "ticket");
        assert_eq!(batch[0].entity_id, "test-1");

        // Signal shutdown.
        shutdown_tx.send(true).expect("failed to send shutdown");

        teardown_test_db(pool, &admin, &db_name).await;
        admin.close().await;
    }
}
