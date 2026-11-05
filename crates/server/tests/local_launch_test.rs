// `runtime = local` launch/stop through `WorkerManager` against a fake
// builderd serving only `BuilderLocalWorkerService`: builderd errors reach the
// caller, a failed launch deletes the provisional row, and a failed stop
// leaves the row `stopping`. Also `LocalWorkerReconcileManager` passes
// against the fake's `ListLocalWorkers`.

use std::sync::{Arc, Mutex};

use tonic::transport::Server;
use tonic::{Request, Response, Status};

use ur_rpc::proto::builder_local_worker::builder_local_worker_service_server::{
    BuilderLocalWorkerService, BuilderLocalWorkerServiceServer,
};
use ur_rpc::proto::builder_local_worker::{
    LaunchLocalWorkerRequest, LaunchLocalWorkerResponse, ListLocalWorkersRequest,
    ListLocalWorkersResponse, LocalWorkerStatus, StopLocalWorkerRequest, StopLocalWorkerResponse,
};
use ur_rpc::proto::builder_pool::builder_pool_service_server::{
    BuilderPoolService, BuilderPoolServiceServer,
};
use ur_rpc::proto::builder_pool::{
    CheckoutBranchRequest, CheckoutBranchResponse, PrepareNewSlotRequest, PrepareNewSlotResponse,
    PrepareSharedSlotRequest, PrepareSharedSlotResponse, RecycleSlotRequest, RecycleSlotResponse,
    ScanSlotsRequest, ScanSlotsResponse,
};
use ur_server::builder_local_worker_client::BuilderLocalWorkerClient;
use ur_server::local_reconcile::LocalWorkerReconcileManager;

const WORKER_PORT: u16 = 23456;

/// Records requests; returns `launch_error`/`stop_error` when set.
#[derive(Clone, Default)]
struct FakeBuilderd {
    launches: Arc<Mutex<Vec<LaunchLocalWorkerRequest>>>,
    stops: Arc<Mutex<Vec<String>>>,
    launch_error: Arc<Mutex<Option<Status>>>,
    stop_error: Arc<Mutex<Option<Status>>>,
    /// `ListLocalWorkers` answer: `(worker_id, session_alive)`.
    listed: Arc<Mutex<Vec<(String, bool)>>>,
    list_error: Arc<Mutex<Option<Status>>>,
    /// Inserted while `ListLocalWorkers` runs: a launch racing a pass.
    insert_on_list: Arc<Mutex<Option<(workflow_db::WorkerRepo, workflow_db::model::Worker)>>>,
    /// When set, the first StopLocalWorker waits for a second to arrive, and
    /// every later one waits for this.
    hold_later_stops: Arc<Mutex<Option<Arc<tokio::sync::Notify>>>>,
    /// Every `BuilderPoolService` call: releasing a slot must issue none.
    pool_calls: Arc<Mutex<Vec<&'static str>>>,
    /// `RecycleSlot` answer (a launch's acquire); refused when unset.
    recycle_to: Arc<Mutex<Option<std::path::PathBuf>>>,
    /// Notified when LaunchLocalWorker is entered.
    launch_entered: Arc<tokio::sync::Notify>,
    /// When set, LaunchLocalWorker waits for this before answering.
    hold_launch: Arc<Mutex<Option<Arc<tokio::sync::Notify>>>>,
}

impl FakeBuilderd {
    fn pool_call<T>(&self, rpc: &'static str) -> Result<Response<T>, Status> {
        self.pool_calls.lock().unwrap().push(rpc);
        Err(Status::unimplemented(rpc))
    }
}

/// Records and refuses every pool RPC (none of these tests acquire slots).
#[tonic::async_trait]
impl BuilderPoolService for FakeBuilderd {
    async fn scan_slots(
        &self,
        _: Request<ScanSlotsRequest>,
    ) -> Result<Response<ScanSlotsResponse>, Status> {
        self.pool_call("scan_slots")
    }
    async fn prepare_new_slot(
        &self,
        _: Request<PrepareNewSlotRequest>,
    ) -> Result<Response<PrepareNewSlotResponse>, Status> {
        self.pool_call("prepare_new_slot")
    }
    async fn recycle_slot(
        &self,
        _: Request<RecycleSlotRequest>,
    ) -> Result<Response<RecycleSlotResponse>, Status> {
        let recycled = self.recycle_to.lock().unwrap().clone();
        match recycled {
            Some(path) => Ok(Response::new(RecycleSlotResponse {
                host_path: path.display().to_string(),
            })),
            None => self.pool_call("recycle_slot"),
        }
    }
    async fn prepare_shared_slot(
        &self,
        _: Request<PrepareSharedSlotRequest>,
    ) -> Result<Response<PrepareSharedSlotResponse>, Status> {
        self.pool_call("prepare_shared_slot")
    }
    async fn checkout_branch(
        &self,
        _: Request<CheckoutBranchRequest>,
    ) -> Result<Response<CheckoutBranchResponse>, Status> {
        self.pool_call("checkout_branch")
    }
}

#[tonic::async_trait]
impl BuilderLocalWorkerService for FakeBuilderd {
    async fn launch_local_worker(
        &self,
        request: Request<LaunchLocalWorkerRequest>,
    ) -> Result<Response<LaunchLocalWorkerResponse>, Status> {
        self.launches.lock().unwrap().push(request.into_inner());
        self.launch_entered.notify_one();
        let hold = self.hold_launch.lock().unwrap().clone();
        if let Some(hold) = hold {
            hold.notified().await;
        }
        match self.launch_error.lock().unwrap().clone() {
            Some(status) => Err(status),
            None => Ok(Response::new(LaunchLocalWorkerResponse {})),
        }
    }

    async fn stop_local_worker(
        &self,
        request: Request<StopLocalWorkerRequest>,
    ) -> Result<Response<StopLocalWorkerResponse>, Status> {
        let earlier = {
            let mut stops = self.stops.lock().unwrap();
            stops.push(request.into_inner().worker_id);
            stops.len() > 1
        };
        let hold = self.hold_later_stops.lock().unwrap().clone();
        if let Some(hold) = hold {
            if earlier {
                hold.notified().await;
            } else {
                // Let a second stopper get past begin_stopping first.
                for _ in 0..100 {
                    if self.stops.lock().unwrap().len() > 1 {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }
        }
        match self.stop_error.lock().unwrap().clone() {
            Some(status) => Err(status),
            None => Ok(Response::new(StopLocalWorkerResponse {})),
        }
    }

    async fn list_local_workers(
        &self,
        _request: Request<ListLocalWorkersRequest>,
    ) -> Result<Response<ListLocalWorkersResponse>, Status> {
        if let Some(status) = self.list_error.lock().unwrap().clone() {
            return Err(status);
        }
        let insert = self.insert_on_list.lock().unwrap().take();
        if let Some((repo, worker)) = insert {
            repo.insert_worker(&worker).await.unwrap();
        }
        let workers = self
            .listed
            .lock()
            .unwrap()
            .iter()
            .map(|(worker_id, session_alive)| LocalWorkerStatus {
                worker_id: worker_id.clone(),
                session_alive: *session_alive,
            })
            .collect();
        Ok(Response::new(ListLocalWorkersResponse { workers }))
    }
}

struct Harness {
    fake: FakeBuilderd,
    worker_manager: ur_server::WorkerManager,
    launch_manager: ur_server::grpc::LaunchManager,
    worker_repo: workflow_db::WorkerRepo,
    local_client: BuilderLocalWorkerClient,
    slot: std::path::PathBuf,
    _tmp: tempfile::TempDir,
    _test_db: ur_db_test::TestDb,
}

async fn harness() -> Harness {
    let fake = FakeBuilderd::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let service = BuilderLocalWorkerServiceServer::new(fake.clone());
    let pool_service = BuilderPoolServiceServer::new(fake.clone());
    tokio::spawn(async move {
        Server::builder()
            .add_service(service)
            .add_service(pool_service)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    let local_channel = tonic::transport::Endpoint::try_from(format!("http://{addr}"))
        .unwrap()
        .connect_lazy();

    let tmp = tempfile::tempdir().unwrap();
    let config = ur_config::Config::from_toml_str(
        "[projects.proj]\nrepo = \"git@example.com:o/proj.git\"\nruntime = \"local\"\n",
        tmp.path(),
    )
    .unwrap();
    let slot = tmp.path().join("pool/proj/0");
    std::fs::create_dir_all(&slot).unwrap();
    let test_db = ur_db_test::TestDb::new().await;
    let worker_repo = workflow_db::WorkerRepo::new(test_db.workflow_pool().clone());
    // Container builderd is unreachable: a local launch must never touch it.
    let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();
    let builder_container_client =
        ur_server::builder_container_client::BuilderContainerClient::new(channel);
    let project_registry = ur_server::ProjectRegistry::new(
        config.projects.clone(),
        ur_server::hostexec::HostExecConfigManager::empty(),
    );
    let pool = ur_server::RepoPoolManager::new(
        &config,
        worker_repo.clone(),
        project_registry.clone(),
        ur_server::BuilderPoolClient::new(local_channel.clone()),
    );
    let worker_manager = ur_server::WorkerManager::new(
        config.workspace.clone(),
        config.config_dir.clone(),
        config.config_dir.clone(),
        config.logs_dir.clone(),
        config.logs_dir.clone(),
        ur_server::network_manager::NetworkManager::new(
            builder_container_client.clone(),
            config.network.worker_name.clone(),
        ),
        config.network.clone(),
        WORKER_PORT,
        ur_server::worker::WorkerModesConfig::default(),
        worker_repo.clone(),
        config.global_skills.clone(),
        builder_container_client,
        BuilderLocalWorkerClient::new(local_channel.clone()),
    );
    let ticket_pool = test_db.ticket_pool().clone();
    let launch_manager = ur_server::grpc::LaunchManager {
        worker_manager: worker_manager.clone(),
        repo_pool_manager: pool,
        proxy_hostname: config.proxy.hostname.clone(),
        project_registry,
        worker_repo: worker_repo.clone(),
        ticket_repo: ticket_db::TicketRepo::new(
            ticket_pool.clone(),
            ticket_db::GraphManager::new(ticket_pool),
        ),
        workflow_repo: workflow_db::WorkflowRepo::new(test_db.workflow_pool().clone()),
        network_config: config.network.clone(),
        workspace_brain_dir: None,
        config_dir: config.config_dir.clone(),
        worker_image_tag: ur_config::DEFAULT_IMAGE_TAG.to_owned(),
        workerd_resolver: ur_server::WorkerdAddressResolver::new(
            "http://127.0.0.1:1".into(),
            config.network.worker_prefix.clone(),
        ),
    };
    Harness {
        fake,
        worker_manager,
        launch_manager,
        worker_repo,
        local_client: BuilderLocalWorkerClient::new(local_channel),
        slot,
        _tmp: tmp,
        _test_db: test_db,
    }
}

/// No project key, so a stop releases no pool slot (the pool is unreachable).
fn local_config(h: &Harness, worker_id: &str) -> ur_server::WorkerConfig {
    ur_server::WorkerConfig {
        process_id: "local-1".into(),
        worker_id: ur_server::WorkerId(worker_id.into()),
        image_id: String::new(),
        cpus: 0,
        memory: String::new(),
        workspace_dir: Some(h.slot.clone()),
        proxy_hostname: "ur-squid".into(),
        project_key: String::new(),
        agent_type: "claude".into(),
        strategy: ur_server::WorkerStrategy::Manual,
        skills: Vec::new(),
        model: String::new(),
        effort: String::new(),
        instruction_md: None,
        mounts: Vec::new(),
        ports: Vec::new(),
        context_mounts: Vec::new(),
        hostexec_scripts: Vec::new(),
        extra_skill_mounts: Vec::new(),
        memory_dir: None,
        brain_dir: None,
        workspace_brain_dir: None,
        forge: None,
        runtime: ur_config::WorkerRuntime::Local,
        allow_no_verify: false,
    }
}

#[tokio::test]
async fn failed_local_launch_deletes_row_and_returns_builderd_error() {
    let h = harness().await;
    *h.fake.launch_error.lock().unwrap() = Some(Status::failed_precondition(
        "sync startup hooks: hook-stderr-text",
    ));

    let err = h
        .worker_manager
        .run_and_record(local_config(&h, "local-1-aaaa"))
        .await
        .expect_err("builderd rejected the launch");
    assert!(err.contains("hook-stderr-text"), "{err}");
    assert!(err.contains("FailedPrecondition"), "{err}");
    assert_eq!(h.fake.launches.lock().unwrap().len(), 1);
    assert!(
        h.worker_repo
            .get_worker("local-1-aaaa")
            .await
            .unwrap()
            .is_none(),
        "provisional row must be deleted"
    );
}

#[tokio::test]
async fn local_launch_runs_then_stop_keeps_stopping_until_builderd_succeeds() {
    let h = harness().await;
    let (session, secret) = h
        .worker_manager
        .run_and_record(local_config(&h, "local-1-bbbb"))
        .await
        .unwrap();
    assert_eq!(session, "ur-local-1-bbbb");

    let launch = h.fake.launches.lock().unwrap()[0].clone();
    assert_eq!(launch.slot_path, h.slot.to_string_lossy());
    let env: Vec<_> = launch
        .env
        .iter()
        .map(|e| (e.key.as_str(), e.value.as_str()))
        .collect();
    let server_addr = format!("127.0.0.1:{WORKER_PORT}");
    assert!(env.contains(&(ur_config::UR_SERVER_ADDR_ENV, server_addr.as_str())));
    assert!(env.contains(&(ur_config::UR_WORKER_SECRET_ENV, secret.as_str())));
    assert!(env.iter().all(|(k, _)| *k != "HTTP_PROXY"), "{env:?}");

    let row = h
        .worker_repo
        .get_worker("local-1-bbbb")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.container_status, "running");
    assert_eq!(row.container_id, session);
    assert_eq!(row.runtime, ur_config::WorkerRuntime::Local);
    assert!(
        h.worker_manager
            .verify_worker("local-1-bbbb", &secret)
            .await
    );

    let worker_id = ur_server::WorkerId("local-1-bbbb".into());
    *h.fake.stop_error.lock().unwrap() = Some(Status::internal("kill tmux session: boom"));
    // Through the CLI's stop path: builderd's error, not a process-id retry's.
    let status = h
        .launch_manager
        .stop("local-1-bbbb")
        .await
        .expect_err("builderd stop failed");
    assert!(
        status.message().contains("kill tmux session: boom"),
        "{status:?}"
    );
    let row = h
        .worker_repo
        .get_worker("local-1-bbbb")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.container_status, "stopping");

    *h.fake.stop_error.lock().unwrap() = None;
    h.worker_manager
        .stop_by_worker_id(&worker_id)
        .await
        .unwrap();
    let row = h
        .worker_repo
        .get_worker("local-1-bbbb")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.container_status, "stopped");
    assert_eq!(
        *h.fake.stops.lock().unwrap(),
        vec!["local-1-bbbb".to_owned(), "local-1-bbbb".to_owned()]
    );
}

fn reconciler(h: &Harness) -> LocalWorkerReconcileManager {
    LocalWorkerReconcileManager::new(
        h.worker_manager.clone(),
        h.worker_repo.clone(),
        h.local_client.clone(),
        std::time::Duration::from_secs(3600),
    )
}

/// A `running` local row (launched through the fake), optionally moved to `status`.
async fn local_row(h: &Harness, worker_id: &str, status: &str) {
    h.worker_manager
        .run_and_record(local_config(h, worker_id))
        .await
        .unwrap();
    if status != "running" {
        h.worker_repo
            .update_worker_container_status(worker_id, status)
            .await
            .unwrap();
    }
}

async fn status_of(h: &Harness, worker_id: &str) -> String {
    h.worker_repo
        .get_worker(worker_id)
        .await
        .unwrap()
        .unwrap()
        .container_status
}

fn set_listed(h: &Harness, listed: &[(&str, bool)]) {
    *h.fake.listed.lock().unwrap() = listed.iter().map(|(id, a)| (id.to_string(), *a)).collect();
}

#[tokio::test]
async fn reconcile_list_error_changes_nothing() {
    let h = harness().await;
    local_row(&h, "local-1-gone", "running").await;
    local_row(&h, "local-1-stop", "stopping").await;
    *h.fake.list_error.lock().unwrap() = Some(Status::unavailable("builderd restarting"));

    let err = reconciler(&h).reconcile_once().await.unwrap_err();
    assert!(err.contains("builderd restarting"), "{err}");
    assert_eq!(status_of(&h, "local-1-gone").await, "running");
    assert_eq!(status_of(&h, "local-1-stop").await, "stopping");
    assert!(h.fake.stops.lock().unwrap().is_empty());
}

#[tokio::test]
async fn reconcile_leaves_live_and_provisioning_workers_alone() {
    let h = harness().await;
    local_row(&h, "local-1-live", "running").await;
    local_row(&h, "local-1-prov", "provisioning").await;
    local_row(&h, "local-1-prv2", "provisioning").await;
    // A provisioning worker builderd already registered is a launch in flight.
    set_listed(&h, &[("local-1-live", true), ("local-1-prv2", true)]);

    reconciler(&h).reconcile_once().await.unwrap();
    assert_eq!(status_of(&h, "local-1-live").await, "running");
    assert_eq!(status_of(&h, "local-1-prov").await, "provisioning");
    assert_eq!(status_of(&h, "local-1-prv2").await, "provisioning");
    assert!(h.fake.stops.lock().unwrap().is_empty());
}

/// External kill: builderd tore the worker down (unlisted), or its session is
/// dead but still registered. One pass stops both; a failing builderd stop
/// leaves them `stopping` until a later pass succeeds.
#[tokio::test]
async fn reconcile_stops_gone_and_dead_workers_retrying_failed_stops() {
    let h = harness().await;
    local_row(&h, "local-1-gone", "running").await;
    local_row(&h, "local-1-dead", "running").await;
    set_listed(&h, &[("local-1-dead", false)]);
    *h.fake.stop_error.lock().unwrap() = Some(Status::internal("remove drive: busy"));

    reconciler(&h).reconcile_once().await.unwrap();
    assert_eq!(status_of(&h, "local-1-gone").await, "stopping");
    assert_eq!(status_of(&h, "local-1-dead").await, "stopping");

    reconciler(&h).reconcile_once().await.unwrap();
    assert_eq!(
        status_of(&h, "local-1-dead").await,
        "stopping",
        "still failing"
    );

    *h.fake.stop_error.lock().unwrap() = None;
    set_listed(&h, &[]);
    reconciler(&h).reconcile_once().await.unwrap();
    assert_eq!(status_of(&h, "local-1-gone").await, "stopped");
    assert_eq!(status_of(&h, "local-1-dead").await, "stopped");
    let stops = h.fake.stops.lock().unwrap().clone();
    assert_eq!(stops.len(), 6, "{stops:?}");
}

#[tokio::test]
async fn reconcile_stops_listed_workers_without_an_active_row() {
    let h = harness().await;
    local_row(&h, "local-1-done", "stopped").await;
    set_listed(&h, &[("local-1-done", true), ("local-1-ghst", true)]);

    reconciler(&h).reconcile_once().await.unwrap();
    let mut stops = h.fake.stops.lock().unwrap().clone();
    stops.sort();
    assert_eq!(stops, vec!["local-1-done", "local-1-ghst"]);
    assert_eq!(status_of(&h, "local-1-done").await, "stopped");
}

const PROJECT: &str = "proj";

/// Register `h.slot` as pool slot `proj/0`; returns its id.
async fn pool_slot(h: &Harness) -> String {
    let now = chrono::Utc::now().to_rfc3339();
    let slot = workflow_db::model::Slot {
        id: "slot-0".into(),
        project_key: PROJECT.into(),
        slot_name: "0".into(),
        host_path: h.slot.display().to_string(),
        created_at: now.clone(),
        updated_at: now,
    };
    h.worker_repo.insert_slot(&slot).await.unwrap();
    std::fs::write(h.slot.join("uncommitted.txt"), "work").unwrap();
    slot.id
}

/// A Manual pool launch in `h.slot`, so a stop releases (unlinks) it.
fn pool_config(h: &Harness, worker_id: &str) -> ur_server::WorkerConfig {
    ur_server::WorkerConfig {
        project_key: PROJECT.into(),
        ..local_config(h, worker_id)
    }
}

/// The `provisioning` row that claims `slot_id` for `worker_id`, as
/// `LaunchManager` inserts it at launch start.
fn claim_row(h: &Harness, worker_id: &str) -> workflow_db::model::Worker {
    let now = chrono::Utc::now().to_rfc3339();
    workflow_db::model::Worker {
        worker_id: worker_id.into(),
        process_id: "local-1".into(),
        project_key: PROJECT.into(),
        container_id: String::new(),
        worker_secret: "placeholder".into(),
        strategy: "manual".into(),
        agent_type: "claude".into(),
        container_status: "provisioning".into(),
        agent_status: "starting".into(),
        workspace_path: Some(h.slot.display().to_string()),
        created_at: now.clone(),
        updated_at: now,
        idle_redispatch_count: 0,
        brain_path: None,
        runtime: ur_config::WorkerRuntime::Local,
    }
}

async fn claim(h: &Harness, worker_id: &str, slot_id: &str) {
    assert!(
        h.worker_repo
            .claim_slot(&claim_row(h, worker_id), slot_id)
            .await
            .unwrap(),
        "slot {slot_id} already taken"
    );
}

/// Claim the slot, then launch in it (what `LaunchManager::launch` does).
async fn launch_in_slot(
    h: &Harness,
    worker_id: &str,
    slot_id: &str,
) -> Result<(String, String), String> {
    claim(h, worker_id, slot_id).await;
    h.worker_manager
        .run_and_record(pool_config(h, worker_id))
        .await
}

async fn slot_owner(h: &Harness, worker_id: &str) -> Option<String> {
    h.worker_repo
        .get_worker_slot(worker_id)
        .await
        .unwrap()
        .map(|l| l.slot_id)
}

/// Release is DB-only: no pool RPC was issued and the uncommitted work in
/// `h.slot` (written by `pool_slot`) is still there.
fn assert_slot_intact(h: &Harness) {
    assert!(
        h.fake.pool_calls.lock().unwrap().is_empty(),
        "release issued a pool RPC"
    );
    assert!(
        h.slot.join("uncommitted.txt").exists(),
        "slot contents touched"
    );
}

/// The slot stays linked while a stop keeps failing and is released only
/// once builderd confirms it (StopLocalWorker success).
#[tokio::test]
async fn reconcile_releases_slot_only_after_confirmed_stop() {
    let h = harness().await;
    let slot = pool_slot(&h).await;
    launch_in_slot(&h, "local-1-dead", &slot).await.unwrap();
    set_listed(&h, &[("local-1-dead", false)]);
    *h.fake.stop_error.lock().unwrap() = Some(Status::internal("remove drive: busy"));

    reconciler(&h).reconcile_once().await.unwrap();
    assert_eq!(status_of(&h, "local-1-dead").await, "stopping");
    assert_eq!(slot_owner(&h, "local-1-dead").await, Some(slot.clone()));
    assert_slot_intact(&h);

    *h.fake.stop_error.lock().unwrap() = None;
    set_listed(&h, &[]);
    reconciler(&h).reconcile_once().await.unwrap();
    assert_eq!(status_of(&h, "local-1-dead").await, "stopped");
    assert_eq!(slot_owner(&h, "local-1-dead").await, None);
    assert_slot_intact(&h);
}

/// A1: a stale stopper (snapshot taken before W1 was stopped and its slot
/// handed to W2) must leave W2's slot alone.
#[tokio::test]
async fn stale_stop_leaves_the_slots_new_owner_untouched() {
    let h = harness().await;
    let slot = pool_slot(&h).await;
    let w1 = ur_server::WorkerId("local-1-old1".into());
    launch_in_slot(&h, "local-1-old1", &slot).await.unwrap();
    h.worker_manager.stop_by_worker_id(&w1).await.unwrap();
    assert_slot_intact(&h);
    launch_in_slot(&h, "local-1-new2", &slot).await.unwrap();
    let stops_before = h.fake.stops.lock().unwrap().len();

    // Stopped row: the stale stop is a no-op (no builderd call, no release).
    h.worker_manager.stop_by_worker_id(&w1).await.unwrap();
    assert_eq!(h.fake.stops.lock().unwrap().len(), stops_before);
    assert_eq!(status_of(&h, "local-1-old1").await, "stopped");

    // Even an active stale row must not release a slot it no longer holds.
    h.worker_repo
        .update_worker_container_status("local-1-old1", "stopping")
        .await
        .unwrap();
    h.worker_manager.stop_by_worker_id(&w1).await.unwrap();
    assert_slot_intact(&h);
    assert_eq!(slot_owner(&h, "local-1-new2").await, Some(slot));
    assert_eq!(status_of(&h, "local-1-new2").await, "running");
}

/// A2: a failed launch drops its row only once builderd confirms the
/// worker gone; otherwise the row is `stopping` and holds the slot.
#[tokio::test]
async fn failed_launch_keeps_slot_until_cleanup_is_confirmed() {
    let h = harness().await;
    let slot = pool_slot(&h).await;
    *h.fake.launch_error.lock().unwrap() = Some(Status::internal("send agent argv: boom"));

    let err = launch_in_slot(&h, "local-1-lfok", &slot).await.unwrap_err();
    assert!(err.contains("boom"), "{err}");
    assert!(
        h.worker_repo
            .get_worker("local-1-lfok")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(*h.fake.stops.lock().unwrap(), vec!["local-1-lfok"]);

    *h.fake.stop_error.lock().unwrap() = Some(Status::internal("kill tmux session: no"));
    let err = launch_in_slot(&h, "local-1-lfno", &slot).await.unwrap_err();
    assert!(
        err.contains("boom") && err.contains("kill tmux session: no"),
        "{err}"
    );
    assert_eq!(status_of(&h, "local-1-lfno").await, "stopping");
    assert_eq!(slot_owner(&h, "local-1-lfno").await, Some(slot.clone()));
    assert!(
        h.worker_repo
            .find_available_slots(PROJECT)
            .await
            .unwrap()
            .is_empty(),
        "the slot is not reusable while the stop is unconfirmed"
    );

    *h.fake.stop_error.lock().unwrap() = None;
    reconciler(&h).reconcile_once().await.unwrap();
    assert_eq!(status_of(&h, "local-1-lfno").await, "stopped");
    assert_eq!(slot_owner(&h, "local-1-lfno").await, None);
    assert_slot_intact(&h);
}

/// A3: at startup a local `provisioning` row is a launch the previous
/// server never finished: it is stopped while its slot stays linked.
#[tokio::test]
async fn startup_stops_stale_provisioning_rows_holding_their_slot() {
    let h = harness().await;
    let slot = pool_slot(&h).await;
    launch_in_slot(&h, "local-1-prov", &slot).await.unwrap();
    // As if the server died between LaunchLocalWorker and finalize.
    h.worker_repo
        .update_worker_container_status("local-1-prov", "provisioning")
        .await
        .unwrap();
    set_listed(&h, &[("local-1-prov", true)]);
    *h.fake.stop_error.lock().unwrap() = Some(Status::internal("kill tmux session: no"));

    reconciler(&h).startup().await.unwrap();
    assert_eq!(status_of(&h, "local-1-prov").await, "stopping");
    assert_eq!(slot_owner(&h, "local-1-prov").await, Some(slot));
    assert_eq!(*h.fake.stops.lock().unwrap(), vec!["local-1-prov"]);
    assert_slot_intact(&h);

    *h.fake.stop_error.lock().unwrap() = None;
    reconciler(&h).reconcile_once().await.unwrap();
    assert_eq!(status_of(&h, "local-1-prov").await, "stopped");
    assert_slot_intact(&h);
}

/// A row that appears while the pass lists builderd (a launch registering)
/// is re-read and not stopped.
#[tokio::test]
async fn reconcile_rereads_unknown_listed_ids_before_stopping() {
    let h = harness().await;
    local_row(&h, "local-1-tmpl", "running").await;
    let mut racing = h
        .worker_repo
        .get_worker("local-1-tmpl")
        .await
        .unwrap()
        .unwrap();
    h.worker_repo.delete_worker("local-1-tmpl").await.unwrap();
    racing.worker_id = "local-1-race".into();
    racing.container_status = "provisioning".into();
    *h.fake.insert_on_list.lock().unwrap() = Some((h.worker_repo.clone(), racing));
    set_listed(&h, &[("local-1-race", true)]);
    h.fake.stops.lock().unwrap().clear();

    reconciler(&h).reconcile_once().await.unwrap();
    assert!(h.fake.stops.lock().unwrap().is_empty());
    assert_eq!(status_of(&h, "local-1-race").await, "provisioning");
}

/// Two stoppers of W1 race: one finishes and the slot goes to W2, which has
/// uncommitted work; the other completes afterwards. W2's link and files
/// survive (release only deletes W1's own link).
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_stoppers_leave_a_reacquired_slot_alone() {
    let h = harness().await;
    let slot = pool_slot(&h).await;
    launch_in_slot(&h, "local-1-old1", &slot).await.unwrap();
    let hold = Arc::new(tokio::sync::Notify::new());
    *h.fake.hold_later_stops.lock().unwrap() = Some(hold.clone());
    let w1 = ur_server::WorkerId("local-1-old1".into());
    let (a, b) = (h.worker_manager.clone(), h.worker_manager.clone());
    let (wa, wb) = (w1.clone(), w1.clone());
    let stop_a = tokio::spawn(async move { a.stop_by_worker_id(&wa).await });
    let stop_b = tokio::spawn(async move { b.stop_by_worker_id(&wb).await });

    for _ in 0..250 {
        if status_of(&h, "local-1-old1").await == "stopped" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(status_of(&h, "local-1-old1").await, "stopped");
    assert_eq!(h.fake.stops.lock().unwrap().len(), 2, "second stopper held");
    assert_eq!(
        h.worker_repo
            .find_available_slots(PROJECT)
            .await
            .unwrap()
            .len(),
        1,
        "the slot is back in the pool"
    );
    launch_in_slot(&h, "local-1-new2", &slot).await.unwrap();
    std::fs::write(h.slot.join("w2-work.txt"), "work").unwrap();

    hold.notify_one();
    let results = (stop_a.await.unwrap(), stop_b.await.unwrap());
    assert!(results.0.is_ok() && results.1.is_ok(), "{results:?}");
    assert_eq!(slot_owner(&h, "local-1-new2").await, Some(slot));
    assert!(h.slot.join("w2-work.txt").exists());
    assert_slot_intact(&h);
}

/// M7: a stale stop of a `stopped` local row with a lingering slot link
/// calls no StopLocalWorker and releases nothing.
#[tokio::test]
async fn stale_stop_of_stopped_row_with_lingering_link_releases_nothing() {
    let h = harness().await;
    let slot = pool_slot(&h).await;
    launch_in_slot(&h, "local-1-lngr", &slot).await.unwrap();
    h.worker_repo
        .update_worker_container_status("local-1-lngr", "stopped")
        .await
        .unwrap();

    h.worker_manager
        .stop_by_worker_id(&ur_server::WorkerId("local-1-lngr".into()))
        .await
        .unwrap();
    assert!(h.fake.stops.lock().unwrap().is_empty());
    assert_eq!(slot_owner(&h, "local-1-lngr").await, Some(slot));
}

/// A stale provisioning row whose recorded slot W2 now holds: startup links
/// and stops the stale row, and W2's slot, link and files stay untouched.
#[tokio::test]
async fn startup_stop_of_a_stale_row_leaves_the_live_slot_owner_alone() {
    let h = harness().await;
    let slot = pool_slot(&h).await;
    launch_in_slot(&h, "local-1-stal", &slot).await.unwrap();
    h.worker_repo
        .update_worker_container_status("local-1-stal", "provisioning")
        .await
        .unwrap();
    h.worker_repo
        .unlink_worker_slot("local-1-stal")
        .await
        .unwrap();
    launch_in_slot(&h, "local-1-live", &slot).await.unwrap();
    set_listed(&h, &[("local-1-live", true)]);

    reconciler(&h).startup().await.unwrap();
    reconciler(&h).reconcile_once().await.unwrap();
    assert_eq!(status_of(&h, "local-1-stal").await, "stopped");
    assert_eq!(status_of(&h, "local-1-live").await, "running");
    assert_eq!(slot_owner(&h, "local-1-live").await, Some(slot));
    assert_eq!(*h.fake.stops.lock().unwrap(), vec!["local-1-stal"]);
    assert_slot_intact(&h);
}

/// M26: rows are read before the list, so a launch that finalizes to
/// `running` while the pass lists builderd is not taken for "gone".
#[tokio::test]
async fn reconcile_never_stops_a_launch_that_finished_during_the_list() {
    let h = harness().await;
    local_row(&h, "local-1-tmpl", "running").await;
    let mut racing = h
        .worker_repo
        .get_worker("local-1-tmpl")
        .await
        .unwrap()
        .unwrap();
    h.worker_repo.delete_worker("local-1-tmpl").await.unwrap();
    racing.worker_id = "local-1-fin1".into();
    *h.fake.insert_on_list.lock().unwrap() = Some((h.worker_repo.clone(), racing));
    // The list predates builderd registering it.
    set_listed(&h, &[]);

    reconciler(&h).reconcile_once().await.unwrap();
    assert!(h.fake.stops.lock().unwrap().is_empty());
    assert_eq!(status_of(&h, "local-1-fin1").await, "running");
}

/// A stop that lands while a local launch is in flight only marks the row
/// `stopping`; the launch's finalize then fails and the launch stops the
/// worker itself. The slot stays linked (never available) until
/// StopLocalWorker succeeds.
#[tokio::test(flavor = "multi_thread")]
async fn stop_during_launch_is_finished_by_the_launch() {
    for stop_fails in [false, true] {
        let h = harness().await;
        let slot = pool_slot(&h).await;
        let hold = Arc::new(tokio::sync::Notify::new());
        *h.fake.hold_launch.lock().unwrap() = Some(hold.clone());
        claim(&h, "local-1-infl", &slot).await;
        let wm = h.worker_manager.clone();
        let config = pool_config(&h, "local-1-infl");
        let launch = tokio::spawn(async move { wm.run_and_record(config).await });
        h.fake.launch_entered.notified().await;

        let w = ur_server::WorkerId("local-1-infl".into());
        h.worker_manager.stop_by_worker_id(&w).await.unwrap();
        assert_eq!(status_of(&h, "local-1-infl").await, "stopping");
        assert!(
            h.fake.stops.lock().unwrap().is_empty(),
            "stopper leaves it to the launch"
        );
        assert_eq!(slot_owner(&h, "local-1-infl").await, Some(slot.clone()));
        assert!(
            h.worker_repo
                .find_available_slots(PROJECT)
                .await
                .unwrap()
                .is_empty()
        );

        if stop_fails {
            *h.fake.stop_error.lock().unwrap() = Some(Status::internal("kill tmux session: no"));
        }
        hold.notify_one();
        let err = launch.await.unwrap().unwrap_err();
        assert!(err.contains("stopped during launch"), "{err}");
        assert_eq!(*h.fake.stops.lock().unwrap(), vec!["local-1-infl"]);
        if stop_fails {
            assert_eq!(status_of(&h, "local-1-infl").await, "stopping");
            assert_eq!(slot_owner(&h, "local-1-infl").await, Some(slot.clone()));
            *h.fake.stop_error.lock().unwrap() = None;
            reconciler(&h).reconcile_once().await.unwrap();
        }
        assert_eq!(status_of(&h, "local-1-infl").await, "stopped");
        assert_eq!(slot_owner(&h, "local-1-infl").await, None);
    }
}

/// Dropping the launch RPC after builderd registered the worker cannot cut
/// the launch short: it still finalizes (row `running`, slot linked).
#[tokio::test(flavor = "multi_thread")]
async fn cancelled_launch_rpc_still_finalizes() {
    let h = harness().await;
    let slot = pool_slot(&h).await;
    *h.fake.recycle_to.lock().unwrap() = Some(h.slot.clone());
    let hold = Arc::new(tokio::sync::Notify::new());
    *h.fake.hold_launch.lock().unwrap() = Some(hold.clone());

    let req = ur_rpc::proto::core::WorkerLaunchRequest {
        worker_id: "ignored".into(),
        mode: "manual".into(),
        project_key: PROJECT.into(),
        runtime: "local".into(),
        agent_type: "claude".into(),
        ..Default::default()
    };
    let launch = h.launch_manager.launch(req);
    tokio::select! {
        _ = launch => panic!("launch finished while builderd held it"),
        _ = h.fake.launch_entered.notified() => {}
    }
    // The RPC future is dropped here, mid-launch.
    let row = h
        .worker_repo
        .list_active_workers()
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(row.container_status, "provisioning");
    assert_eq!(slot_owner(&h, &row.worker_id).await, Some(slot.clone()));

    hold.notify_one();
    for _ in 0..250 {
        if status_of(&h, &row.worker_id).await == "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(status_of(&h, &row.worker_id).await, "running");
    assert_eq!(slot_owner(&h, &row.worker_id).await, Some(slot));
    assert!(
        h.worker_repo
            .find_available_slots(PROJECT)
            .await
            .unwrap()
            .is_empty()
    );
}

/// `claim` from synchronous code (an `acquire_slot` owner closure).
fn claim_blocking(h: &Harness, worker_id: &str, slot_id: &str) {
    let rival = claim_row(h, worker_id);
    let claim = h.worker_repo.claim_slot(&rival, slot_id);
    let won = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(claim));
    assert!(won.unwrap());
}

/// A launch that loses the claim of a candidate slot (another launch
/// inserted its claim between the candidate read and this insert) moves on
/// to the next slot; the rival keeps the slot it won.
#[tokio::test(flavor = "multi_thread")]
async fn acquire_moves_on_when_a_rival_claims_the_candidate_first() {
    let h = harness().await;
    let slot0 = pool_slot(&h).await;
    let slot1_path = h.slot.with_file_name("1");
    let now = chrono::Utc::now().to_rfc3339();
    h.worker_repo
        .insert_slot(&workflow_db::model::Slot {
            id: "slot-1".into(),
            project_key: PROJECT.into(),
            slot_name: "1".into(),
            host_path: slot1_path.display().to_string(),
            created_at: now.clone(),
            updated_at: now,
        })
        .await
        .unwrap();
    *h.fake.recycle_to.lock().unwrap() = Some(slot1_path.clone());
    let rival_done = std::sync::atomic::AtomicBool::new(false);
    let owner = |path: &std::path::Path| {
        // The candidates were already read: race a rival's claim of slot 0
        // in before this launch's insert.
        if path == h.slot && !rival_done.swap(true, std::sync::atomic::Ordering::SeqCst) {
            claim_blocking(&h, "local-1-rivl", "slot-0");
        }
        claim_row(&h, "local-1-mine")
    };
    let (path, row) = h
        .launch_manager
        .repo_pool_manager
        .acquire_slot(PROJECT, owner)
        .await
        .unwrap();
    assert_eq!(path, slot1_path);
    assert_eq!(
        slot_owner(&h, &row.worker_id).await.as_deref(),
        Some("slot-1")
    );
    assert_eq!(slot_owner(&h, "local-1-rivl").await, Some(slot0));
}

/// A claimed launch stopped before anything was started never reaches
/// builderd, and its stop is finished on the spot.
#[tokio::test]
async fn launch_stopped_before_start_never_reaches_builderd() {
    let h = harness().await;
    let slot = pool_slot(&h).await;
    claim(&h, "local-1-early", &slot).await;
    h.worker_manager
        .stop_by_worker_id(&ur_server::WorkerId("local-1-early".into()))
        .await
        .unwrap();
    let err = h
        .worker_manager
        .run_and_record(pool_config(&h, "local-1-early"))
        .await
        .unwrap_err();
    assert!(err.contains("stopped during launch"), "{err}");
    assert!(h.fake.launches.lock().unwrap().is_empty());
    assert_eq!(status_of(&h, "local-1-early").await, "stopped");
    assert_eq!(slot_owner(&h, "local-1-early").await, None);
}
