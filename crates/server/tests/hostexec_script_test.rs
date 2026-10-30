//! HostExec authorization paths that need a registered worker but no container:
//! the script allowlist (`exec_script`) and the per-project `allow_no_verify`
//! grant reaching the git Lua transform.
//!
//! Workers are registered straight into the worker table, and builderd is a lazy
//! channel to a dead port: a request refused by authorization fails with its own
//! status before builderd is ever dialed, while a request that passes
//! authorization surfaces as a builderd transport failure.

use std::path::{Path, PathBuf};

use tonic::Code;
use tonic::transport::{Endpoint, Server};
use tonic_types::StatusExt;

use ur_rpc::proto::hostexec::host_exec_message::Payload;
use ur_rpc::proto::hostexec::host_exec_service_client::HostExecServiceClient;
use ur_rpc::proto::hostexec::host_exec_service_server::HostExecServiceServer;
use ur_rpc::proto::hostexec::{HostExecMessage, HostExecRequest};

const TEST_TOML: &str = r#"
[projects.scriptproj]
repo = "git@github.com:test/scriptproj.git"
hostexec_scripts = ["host-only.sh"]
[projects.scriptproj.container]
image = "ur-worker"

[projects.noverifyproj]
repo = "git@github.com:test/noverifyproj.git"
[projects.noverifyproj.container]
image = "ur-worker"
[projects.noverifyproj.git]
allow_no_verify = true

[projects.poolproj]
repo = "git@github.com:test/poolproj.git"
[projects.poolproj.container]
image = "ur-worker"
"#;

/// A HostExec server plus the worker manager used to register workers against it.
struct HostExecHarness {
    worker_manager: ur_server::WorkerManager,
    client: HostExecServiceClient<tonic::transport::Channel>,
    workspace: PathBuf,
    _test_db: ur_db_test::TestDb,
}

impl HostExecHarness {
    async fn start(dir: &Path) -> Self {
        let workspace = dir.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let config = ur_config::Config::from_toml_str(TEST_TOML, dir).unwrap();

        let test_db = ur_db_test::TestDb::new().await;
        let worker_repo = workflow_db::WorkerRepo::new(test_db.workflow_pool().clone());
        let dead_builderd =
            tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();
        let builder_container_client =
            ur_server::builder_container_client::BuilderContainerClient::new(dead_builderd.clone());
        let network_manager = ur_server::network_manager::NetworkManager::new(
            builder_container_client.clone(),
            config.network.worker_name.clone(),
        );
        let discovery = ur_server::hostexec::LuaDiscoveryManager::new(
            ur_server::hostexec::LuaTransformManager::new(),
        );
        let project_registry = ur_server::ProjectRegistry::new(
            config.projects.clone(),
            ur_server::hostexec::HostExecConfigManager::load(dir, &discovery).unwrap(),
        );
        let repo_pool_manager = ur_server::RepoPoolManager::new(
            &config,
            worker_repo.clone(),
            project_registry.clone(),
            ur_server::BuilderPoolClient::new(dead_builderd.clone()),
        );
        let worker_manager = ur_server::WorkerManager::new(
            workspace.clone(),
            dir.to_path_buf(),
            dir.to_path_buf(),
            dir.join("logs"),
            dir.join("logs"),
            repo_pool_manager,
            network_manager,
            config.network.clone(),
            config.worker_port,
            ur_server::worker::WorkerModesConfig::default(),
            worker_repo,
            ur_config::GlobalSkillsConfig::default(),
            builder_container_client,
        );
        let handler = ur_server::grpc_hostexec::HostExecServiceHandler {
            script_registry: ur_server::hostexec::ScriptRegistry::from_projects(
                &project_registry.projects(),
            ),
            project_registry,
            lua: ur_server::hostexec::LuaTransformManager::new(),
            worker_manager: worker_manager.clone(),
            builderd_client: ur_rpc::proto::builder::BuilderdClient::new(dead_builderd),
            host_workspace: workspace.clone(),
            git_branch_prefix: String::new(),
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
        tokio::spawn(async move {
            Server::builder()
                .add_service(HostExecServiceServer::new(handler))
                .serve_with_incoming(incoming)
                .await
                .unwrap();
        });
        let channel = Endpoint::try_from(format!("http://{addr}"))
            .unwrap()
            .connect()
            .await
            .unwrap();

        Self {
            worker_manager,
            client: HostExecServiceClient::new(channel),
            workspace,
            _test_db: test_db,
        }
    }

    /// Register a running worker for `project_key` (empty = no project) with a
    /// workspace slot, returning its worker ID.
    async fn register_worker(&self, process_id: &str, project_key: &str) -> String {
        let worker_id = self.worker_manager.generate_worker_id(process_id);
        let strategy = if project_key.is_empty() {
            ur_server::WorkerStrategy::Manual
        } else {
            ur_server::WorkerStrategy::Code
        };
        self.worker_manager
            .register_worker(
                worker_id.clone(),
                process_id.into(),
                project_key.into(),
                Some(self.workspace.join(process_id)),
                strategy,
                "fake-cid".into(),
                "secret".into(),
            )
            .await;
        worker_id.to_string()
    }

    /// Send a single-frame Exec request as `worker_id` and return the RPC error.
    async fn exec_error(&self, worker_id: &str, start: HostExecRequest) -> tonic::Status {
        let frames = tokio_stream::iter(vec![HostExecMessage {
            payload: Some(Payload::Start(start)),
        }]);
        let mut request = tonic::Request::new(frames);
        request
            .metadata_mut()
            .insert(ur_config::WORKER_ID_HEADER, worker_id.parse().unwrap());
        match self.client.clone().exec(request).await {
            Ok(_) => panic!("exec should fail: builderd is unreachable in this test"),
            Err(status) => status,
        }
    }
}

fn script_request(script_path: &str) -> HostExecRequest {
    HostExecRequest {
        script_path: script_path.into(),
        working_dir: "/workspace".into(),
        ..Default::default()
    }
}

fn git_push_no_verify_request() -> HostExecRequest {
    HostExecRequest {
        command: "git".into(),
        args: vec![
            "push".into(),
            "--no-verify".into(),
            "origin".into(),
            "HEAD".into(),
        ],
        working_dir: "/workspace".into(),
        ..Default::default()
    }
}

fn assert_script_not_allowed(status: &tonic::Status) {
    assert_eq!(status.code(), Code::PermissionDenied, "{status:?}");
    let info = status
        .get_details_error_info()
        .expect("script denial should carry ErrorInfo");
    assert_eq!(info.reason, ur_rpc::error::SCRIPT_NOT_ALLOWED);
}

/// A worker with no project (a `-w` workspace launch) may run no scripts at all,
/// even one another project declares — and the denial happens before builderd.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_denied_for_worker_without_project() {
    let dir = tempfile::tempdir().unwrap();
    let harness = HostExecHarness::start(dir.path()).await;
    let worker_id = harness.register_worker("script-workspace-man-0", "").await;

    let status = harness
        .exec_error(&worker_id, script_request("/workspace/host-only.sh"))
        .await;
    assert_script_not_allowed(&status);
}

/// A project worker may run only its declared scripts: an undeclared one is
/// denied, while the declared one passes authorization and reaches builderd.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_allowlist_is_per_project() {
    let dir = tempfile::tempdir().unwrap();
    let harness = HostExecHarness::start(dir.path()).await;
    let worker_id = harness
        .register_worker("script-pool-test", "scriptproj")
        .await;

    let denied = harness
        .exec_error(&worker_id, script_request("/workspace/other-undeclared.sh"))
        .await;
    assert_script_not_allowed(&denied);

    let allowed = harness
        .exec_error(&worker_id, script_request("/workspace/host-only.sh"))
        .await;
    assert_eq!(allowed.code(), Code::Internal, "{allowed:?}");
    assert!(
        allowed.message().contains("builderd exec failed"),
        "declared script should pass authorization and reach builderd: {allowed:?}"
    );
}

/// `[projects.<key>.git] allow_no_verify` threads from the project registry into
/// the git transform's worker context: granted projects pass the transform (and
/// reach builderd), everyone else is rejected with `blocked flag: --no-verify`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn allow_no_verify_reaches_git_transform() {
    let dir = tempfile::tempdir().unwrap();
    let harness = HostExecHarness::start(dir.path()).await;
    let granted = harness
        .register_worker("no-verify-allowed-test", "noverifyproj")
        .await;
    let default = harness
        .register_worker("no-verify-default-test", "poolproj")
        .await;

    let blocked = harness
        .exec_error(&default, git_push_no_verify_request())
        .await;
    assert_eq!(blocked.code(), Code::InvalidArgument, "{blocked:?}");
    assert!(
        blocked.message().contains("blocked flag: --no-verify"),
        "{blocked:?}"
    );

    let allowed = harness
        .exec_error(&granted, git_push_no_verify_request())
        .await;
    assert_eq!(allowed.code(), Code::Internal, "{allowed:?}");
    assert!(
        allowed.message().contains("builderd exec failed"),
        "allow_no_verify should let the push through the transform: {allowed:?}"
    );
}
