// `runtime = local` launch/stop through `WorkerManager` against a fake
// builderd serving only `BuilderLocalWorkerService`: builderd errors reach the
// caller, a failed launch deletes the provisional row, and a failed stop
// leaves the row `stopping`.

use std::sync::{Arc, Mutex};

use tonic::transport::Server;
use tonic::{Request, Response, Status};

use ur_rpc::proto::builder_local_worker::builder_local_worker_service_server::{
    BuilderLocalWorkerService, BuilderLocalWorkerServiceServer,
};
use ur_rpc::proto::builder_local_worker::{
    LaunchLocalWorkerRequest, LaunchLocalWorkerResponse, ListLocalWorkersRequest,
    ListLocalWorkersResponse, StopLocalWorkerRequest, StopLocalWorkerResponse,
};

const WORKER_PORT: u16 = 23456;

/// Records requests; returns `launch_error`/`stop_error` when set.
#[derive(Clone, Default)]
struct FakeBuilderd {
    launches: Arc<Mutex<Vec<LaunchLocalWorkerRequest>>>,
    stops: Arc<Mutex<Vec<String>>>,
    launch_error: Arc<Mutex<Option<Status>>>,
    stop_error: Arc<Mutex<Option<Status>>>,
}

#[tonic::async_trait]
impl BuilderLocalWorkerService for FakeBuilderd {
    async fn launch_local_worker(
        &self,
        request: Request<LaunchLocalWorkerRequest>,
    ) -> Result<Response<LaunchLocalWorkerResponse>, Status> {
        self.launches.lock().unwrap().push(request.into_inner());
        match self.launch_error.lock().unwrap().clone() {
            Some(status) => Err(status),
            None => Ok(Response::new(LaunchLocalWorkerResponse {})),
        }
    }

    async fn stop_local_worker(
        &self,
        request: Request<StopLocalWorkerRequest>,
    ) -> Result<Response<StopLocalWorkerResponse>, Status> {
        self.stops
            .lock()
            .unwrap()
            .push(request.into_inner().worker_id);
        match self.stop_error.lock().unwrap().clone() {
            Some(status) => Err(status),
            None => Ok(Response::new(StopLocalWorkerResponse {})),
        }
    }

    async fn list_local_workers(
        &self,
        _request: Request<ListLocalWorkersRequest>,
    ) -> Result<Response<ListLocalWorkersResponse>, Status> {
        Ok(Response::new(ListLocalWorkersResponse::default()))
    }
}

struct Harness {
    fake: FakeBuilderd,
    worker_manager: ur_server::WorkerManager,
    launch_manager: ur_server::grpc::LaunchManager,
    worker_repo: workflow_db::WorkerRepo,
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
    tokio::spawn(async move {
        Server::builder()
            .add_service(service)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    let local_channel = tonic::transport::Endpoint::try_from(format!("http://{addr}"))
        .unwrap()
        .connect_lazy();

    let tmp = tempfile::tempdir().unwrap();
    let config = ur_config::Config::from_toml_str("", tmp.path()).unwrap();
    let slot = tmp.path().join("pool/proj/0");
    std::fs::create_dir_all(&slot).unwrap();
    let test_db = ur_db_test::TestDb::new().await;
    let worker_repo = workflow_db::WorkerRepo::new(test_db.workflow_pool().clone());
    // Container builderd is unreachable: a local launch must never touch it.
    let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();
    let builder_container_client =
        ur_server::builder_container_client::BuilderContainerClient::new(channel.clone());
    let project_registry = ur_server::ProjectRegistry::new(
        config.projects.clone(),
        ur_server::hostexec::HostExecConfigManager::empty(),
    );
    let pool = ur_server::RepoPoolManager::new(
        &config,
        worker_repo.clone(),
        project_registry.clone(),
        ur_server::BuilderPoolClient::new(channel),
    );
    let worker_manager = ur_server::WorkerManager::new(
        config.workspace.clone(),
        config.config_dir.clone(),
        config.config_dir.clone(),
        config.logs_dir.clone(),
        config.logs_dir.clone(),
        pool.clone(),
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
        ur_server::builder_local_worker_client::BuilderLocalWorkerClient::new(local_channel),
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
        slot_id: None,
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
