// Runtime resolution and validation through the real `LaunchManager::launch`.
//
// builderd is unreachable (port 1), so any launch that gets past runtime
// validation fails at pool-slot acquisition. A local-runtime rejection must
// therefore arrive as FailedPrecondition with its own message, and leave no
// slot behind.

use tonic::Code;
use ur_rpc::proto::core::WorkerLaunchRequest;

const PROJECT_TOML: &str = r#"
[projects.plain]
repo = "git@github.com:test/plain.git"

[projects.loc]
repo = "git@github.com:test/loc.git"
runtime = "local"

[worker_modes.localmode]
base = "code"
skills = []
runtime = "local"
"#;

struct Harness {
    launch_manager: ur_server::grpc::LaunchManager,
    worker_repo: workflow_db::WorkerRepo,
    _test_db: ur_db_test::TestDb,
}

async fn harness() -> Harness {
    let config =
        ur_config::Config::from_toml_str(PROJECT_TOML, std::path::Path::new("/tmp/ur-rt-test"))
            .unwrap();
    let worker_modes = ur_server::worker::WorkerModesConfig::from_toml(PROJECT_TOML).unwrap();
    let test_db = ur_db_test::TestDb::new().await;
    let workflow_pool = test_db.workflow_pool().clone();
    let ticket_pool = test_db.ticket_pool().clone();
    let worker_repo = workflow_db::WorkerRepo::new(workflow_pool.clone());
    let ticket_repo = ticket_db::TicketRepo::new(
        ticket_pool.clone(),
        ticket_db::GraphManager::new(ticket_pool),
    );
    let workflow_repo = workflow_db::WorkflowRepo::new(workflow_pool);

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
        config.worker_port,
        worker_modes,
        worker_repo.clone(),
        config.global_skills.clone(),
        builder_container_client,
    );
    let launch_manager = ur_server::grpc::LaunchManager {
        worker_manager,
        repo_pool_manager: pool,
        proxy_hostname: config.proxy.hostname.clone(),
        project_registry,
        worker_repo: worker_repo.clone(),
        ticket_repo,
        workflow_repo,
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
        launch_manager,
        worker_repo,
        _test_db: test_db,
    }
}

fn request(project_key: &str, mode: &str, runtime: &str) -> WorkerLaunchRequest {
    WorkerLaunchRequest {
        worker_id: format!("{project_key}-t1"),
        project_key: project_key.into(),
        mode: mode.into(),
        runtime: runtime.into(),
        ..Default::default()
    }
}

/// Launch `req`, expect FailedPrecondition containing `needle`, and check that
/// no pool slot was created or claimed.
async fn assert_rejected(h: &Harness, req: WorkerLaunchRequest, needle: &str) {
    let status = h
        .launch_manager
        .launch(req)
        .await
        .expect_err("launch must be rejected");
    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert!(status.message().contains(needle), "{}", status.message());
    let slots = h.worker_repo.list_all_slots().await.unwrap();
    assert!(slots.is_empty(), "local rejection must not touch the pool");
}

const UNAVAILABLE: &str = "local runtime launch is not available yet";

#[tokio::test]
async fn cli_local_is_rejected_before_slot_work() {
    let h = harness().await;
    assert_rejected(&h, request("plain", "code", "local"), UNAVAILABLE).await;
}

#[tokio::test]
async fn project_runtime_local_applies_with_empty_request_runtime() {
    let h = harness().await;
    assert_rejected(&h, request("loc", "code", ""), UNAVAILABLE).await;
}

#[tokio::test]
async fn mode_runtime_local_applies() {
    let h = harness().await;
    assert_rejected(&h, request("plain", "localmode", ""), UNAVAILABLE).await;
}

#[tokio::test]
async fn workspace_launch_with_local_has_no_pool_slot() {
    let h = harness().await;
    let mut req = request("plain", "code", "local");
    req.workspace_dir = "/tmp/ur-rt-test/ws".into();
    assert_rejected(&h, req, "pool slot").await;
}

#[tokio::test]
async fn codex_with_local_is_unsupported() {
    let h = harness().await;
    let mut req = request("plain", "code", "local");
    req.agent_type = ur_config::AgentType::Codex.name().into();
    assert_rejected(&h, req, "agent 'codex'").await;
}

/// `--runtime container` beats the project's `runtime = "local"`, so the
/// launch proceeds to slot acquisition and fails there (builderd unreachable)
/// — proving runtime rejection happens before any pool work.
#[tokio::test]
async fn cli_container_overrides_project_local_and_reaches_pool() {
    let h = harness().await;
    let status = h
        .launch_manager
        .launch(request("loc", "code", "container"))
        .await
        .expect_err("builderd is unreachable");
    assert!(
        status.message().contains("pool slot acquisition failed"),
        "{status:?}"
    );
}
