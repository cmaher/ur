// Acceptance test: two-DB split end-to-end.
//
// Verifies that the server correctly wires two independent Postgres databases
// (one for tickets, one for workflows/workers) and that UI events from both
// databases flow through the merged UiEventPoller to gRPC subscribers.
//
// Test flow:
//   1. Create two isolated Postgres databases — ticket_db and workflow_db.
//   2. Build TicketServiceHandler with UiEventPoller connected to both pools.
//   3. Spawn a gRPC server serving the TicketService.
//   4. Create a ticket via the CreateTicket RPC → verify it's stored in ticket_db.
//   5. Insert a worker row into workflow_db → verify it's stored there.
//   6. Subscribe to the UI event stream; trigger mutations in both DBs;
//      assert that events from both arrive on the merged stream.
//
// Gated behind `--features acceptance` so this test does not run in normal `cargo test`.
#![cfg(feature = "acceptance")]

use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Row};
use std::str::FromStr;
use tokio::sync::watch;
use tonic::transport::{Endpoint, Server};
use uuid::Uuid;

use ticket_db::{GraphManager, TicketRepo};
use workflow_db::{WorkerRepo, WorkflowRepo};

use ur_rpc::proto::ticket::{
    CreateTicketRequest, SubscribeUiEventsRequest, UiEventType,
    ticket_service_client::TicketServiceClient, ticket_service_server::TicketServiceServer,
};
use ur_server::UiEventPoller;
use ur_server::grpc_ticket::TicketServiceHandler;

/// Connect to the CI postgres admin instance.
async fn admin_pool() -> PgPool {
    let opts = PgConnectOptions::from_str("postgres://ur:ur@localhost:5433/postgres")
        .expect("invalid connection string");
    PgPoolOptions::new()
        .max_connections(2)
        .connect_with(opts)
        .await
        .expect("Cannot connect to ci-postgres on localhost:5433. Run: cargo make test:init")
}

/// Create a named Postgres database and connect to it.
async fn create_test_db(admin: &PgPool, db_name: &str) -> (PgPool, String) {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE DATABASE \"{db_name}\""
    )))
    .execute(admin)
    .await
    .unwrap_or_else(|e| panic!("failed to create database '{db_name}': {e}"));

    let db_url = format!("postgres://ur:ur@localhost:5433/{db_name}");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .unwrap_or_else(|e| panic!("failed to connect to '{db_name}': {e}"));

    (pool, db_url)
}

/// Drop a database (best-effort; called in cleanup).
async fn drop_test_db(admin: &PgPool, db_name: &str) {
    let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
    )))
    .execute(admin)
    .await;
}

/// Spawn a gRPC server serving TicketService and return a connected client.
async fn spawn_ticket_server(
    handler: TicketServiceHandler,
) -> TicketServiceClient<tonic::transport::Channel> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        Server::builder()
            .add_service(TicketServiceServer::new(handler))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    let channel = Endpoint::try_from(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();

    TicketServiceClient::new(channel)
}

/// Verify that the ticket row is in ticket_db and no workflow exists in workflow_db yet.
async fn verify_ticket_db_isolation(ticket_id: &str, ticket_pool: &PgPool, workflow_pool: &PgPool) {
    let ticket_exists: bool = sqlx::query("SELECT EXISTS(SELECT 1 FROM ticket WHERE id = $1)")
        .bind(ticket_id)
        .fetch_one(ticket_pool)
        .await
        .map(|row| row.get::<bool, _>(0))
        .expect("ticket_db query failed");
    assert!(ticket_exists, "ticket should be stored in ticket_db");

    let workflow_exists: bool =
        sqlx::query("SELECT EXISTS(SELECT 1 FROM workflow WHERE ticket_id = $1)")
            .bind(ticket_id)
            .fetch_one(workflow_pool)
            .await
            .map(|row| row.get::<bool, _>(0))
            .expect("workflow_db query failed");
    assert!(
        !workflow_exists,
        "workflow_db should not have a workflow row before CreateWorkflow is called"
    );
}

/// Verify that the worker row lives in workflow_db and not in ticket_db.
async fn verify_worker_db_isolation(
    worker_id: &str,
    ticket_pool: &PgPool,
    worker_repo: &WorkerRepo,
    ticket_id: &str,
) {
    let fetched = worker_repo
        .get_worker(worker_id)
        .await
        .expect("get_worker should succeed")
        .expect("worker should exist in workflow_db");
    assert_eq!(fetched.worker_id, worker_id);
    assert_eq!(fetched.process_id, ticket_id);

    let worker_in_ticket_db: bool =
        sqlx::query("SELECT EXISTS(SELECT 1 FROM worker WHERE worker_id = $1)")
            .bind(worker_id)
            .fetch_one(ticket_pool)
            .await
            .map(|row| row.get::<bool, _>(0))
            .expect("ticket_db worker query failed");
    assert!(
        !worker_in_ticket_db,
        "worker should NOT be in ticket_db — it lives in workflow_db only"
    );
}

/// Scan one batch of UI events and update the (saw_ticket, saw_worker) flags.
fn scan_batch(
    batch: &ur_rpc::proto::ticket::UiEventBatch,
    saw_ticket: &mut bool,
    saw_worker: &mut bool,
) {
    for event in &batch.events {
        let kind = UiEventType::try_from(event.entity_type).unwrap_or(UiEventType::Unknown);
        match kind {
            UiEventType::Ticket => *saw_ticket = true,
            UiEventType::Worker => *saw_worker = true,
            _ => {}
        }
    }
}

/// Drain the UI event stream for up to `deadline` and return (saw_ticket, saw_worker).
async fn collect_ui_event_flags(
    stream: &mut tonic::codec::Streaming<ur_rpc::proto::ticket::UiEventBatch>,
    deadline: Duration,
) -> (bool, bool) {
    let mut saw_ticket = false;
    let mut saw_worker = false;

    let _ = tokio::time::timeout(deadline, async {
        loop {
            let Ok(Some(batch)) = stream.message().await else {
                return;
            };
            scan_batch(&batch, &mut saw_ticket, &mut saw_worker);
            if saw_ticket && saw_worker {
                return;
            }
        }
    })
    .await;

    (saw_ticket, saw_worker)
}

/// End-to-end test: two-DB launch path exercises both ticket_db and workflow_db pools
/// and verifies that UI events from both databases arrive on the merged stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_db_split_events_arrive_on_merged_stream() {
    // ---- (1) Create two isolated test databases ----
    let admin = admin_pool().await;

    let ticket_db_name = format!("ur_2db_ticket_{}", Uuid::new_v4().simple());
    let workflow_db_name = format!("ur_2db_workflow_{}", Uuid::new_v4().simple());

    let (ticket_pool, ticket_url) = create_test_db(&admin, &ticket_db_name).await;
    let (workflow_pool, workflow_url) = create_test_db(&admin, &workflow_db_name).await;

    ticket_db::migrate(&ticket_pool)
        .await
        .expect("ticket_db migration failed");
    workflow_db::migrate(&workflow_pool)
        .await
        .expect("workflow_db migration failed");

    // ---- (2) Build repos and TicketServiceHandler with two-pool UiEventPoller ----
    let graph_manager = GraphManager::new(ticket_pool.clone());
    let ticket_repo = TicketRepo::new(ticket_pool.clone(), graph_manager);
    let workflow_repo = WorkflowRepo::new(workflow_pool.clone());
    let worker_repo = WorkerRepo::new(workflow_pool.clone());

    let ui_event_poller = UiEventPoller::new(
        ticket_pool.clone(),
        ticket_url.clone(),
        workflow_pool.clone(),
        workflow_url.clone(),
        Duration::from_millis(500),
    );

    let project_registry = ur_server::ProjectRegistry::new(
        std::collections::HashMap::new(),
        ur_server::hostexec::HostExecConfigManager::empty(),
    );

    let handler = TicketServiceHandler {
        ticket_repo: ticket_repo.clone(),
        workflow_repo: workflow_repo.clone(),
        project_registry,
        transition_tx: None,
        cancel_tx: None,
        ui_event_poller: Some(ui_event_poller.clone()),
        worker_manager: None,
        worker_repo: None,
        worker_prefix: String::new(),
    };

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    ui_event_poller.spawn(shutdown_rx);

    // ---- (3) Spawn gRPC server and subscribe to UI events ----
    let mut client = spawn_ticket_server(handler).await;
    let mut event_stream = client
        .subscribe_ui_events(tonic::Request::new(SubscribeUiEventsRequest {}))
        .await
        .expect("SubscribeUiEvents should succeed")
        .into_inner();

    // ---- (4) Create a ticket via gRPC → verify it's in ticket_db ----
    let create_resp = client
        .create_ticket(tonic::Request::new(CreateTicketRequest {
            title: "Two-DB acceptance test ticket".to_string(),
            body: "Verifies ticket_db is separate from workflow_db".to_string(),
            ticket_type: "code".to_string(),
            priority: 0,
            project: "ur".to_string(),
            ..Default::default()
        }))
        .await
        .expect("CreateTicket RPC should succeed");

    let ticket_id = create_resp.into_inner().id;
    assert!(
        !ticket_id.is_empty(),
        "created ticket should have a non-empty ID"
    );

    verify_ticket_db_isolation(&ticket_id, &ticket_pool, &workflow_pool).await;

    // ---- (5) Insert a worker row into workflow_db → verify isolation ----
    let worker_id = format!("tw-worker-{}", Uuid::new_v4().simple());
    let now = chrono::Utc::now().to_rfc3339();
    let worker = workflow_db::model::Worker {
        worker_id: worker_id.clone(),
        process_id: ticket_id.clone(),
        project_key: "ur".to_string(),
        container_id: "two-db-test-container".to_string(),
        worker_secret: "test-secret".to_string(),
        strategy: "code".to_string(),
        container_status: "running".to_string(),
        agent_status: "starting".to_string(),
        workspace_path: Some("/tmp/two-db-test".to_string()),
        created_at: now.clone(),
        updated_at: now,
        idle_redispatch_count: 0,
    };
    worker_repo
        .insert_worker(&worker)
        .await
        .expect("inserting worker into workflow_db should succeed");

    verify_worker_db_isolation(&worker_id, &ticket_pool, &worker_repo, &ticket_id).await;

    // ---- (6) Collect UI events and verify both DBs contribute to the merged stream ----
    //
    // The ticket INSERT triggers a TICKET event in ticket_db's ui_events.
    // The worker INSERT triggers a WORKER event in workflow_db's ui_events.
    // Both pollers forward events to the merged stream; we assert both arrive.
    let (saw_ticket_event, saw_worker_event) =
        collect_ui_event_flags(&mut event_stream, Duration::from_secs(5)).await;

    assert!(
        saw_ticket_event,
        "should have received at least one TICKET event from ticket_db's ui_events"
    );
    assert!(
        saw_worker_event,
        "should have received at least one WORKER event from workflow_db's ui_events"
    );

    // ---- Cleanup ----
    let _ = shutdown_tx.send(true);
    ticket_pool.close().await;
    workflow_pool.close().await;
    drop_test_db(&admin, &ticket_db_name).await;
    drop_test_db(&admin, &workflow_db_name).await;
    admin.close().await;
}
