use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use tonic::transport::{Endpoint, Server};

/// Helper: start a gRPC server on TCP and return a connected channel.
async fn spawn_grpc_server(
    handler: ur_server::grpc::CoreServiceHandler,
) -> (tonic::transport::Channel, SocketAddr) {
    use ur_rpc::proto::core::core_service_server::CoreServiceServer;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        Server::builder()
            .add_service(CoreServiceServer::new(handler))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    let channel = Endpoint::try_from(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();

    (channel, addr)
}

/// Helper: create a CoreServiceHandler from a temp dir with workspace.
fn make_grpc_handler(dir: &Path) -> (ur_server::grpc::CoreServiceHandler, Arc<ur_server::RepoRegistry>) {
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let repo_registry = Arc::new(ur_server::RepoRegistry::new(workspace.clone()));
    let credential_manager = ur_server::CredentialManager;
    let network_config = ur_config::NetworkConfig {
        name: ur_config::DEFAULT_NETWORK_NAME.to_string(),
        server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.to_string(),
    };
    let network_manager =
        container::NetworkManager::new("docker".to_string(), network_config.name.clone());
    let process_manager = ur_server::ProcessManager::new(
        workspace.clone(),
        repo_registry.clone(),
        credential_manager,
        ur_config::ProxyConfig {
            port: ur_config::DEFAULT_PROXY_PORT,
            allowlist: vec!["api.anthropic.com".to_string()],
        },
        network_manager,
        network_config,
    );
    let handler = ur_server::grpc::CoreServiceHandler {
        process_manager,
        repo_registry: repo_registry.clone(),
        workspace,
    };
    (handler, repo_registry)
}

#[tokio::test]
async fn grpc_ping_over_tcp() {
    use ur_rpc::proto::core::PingRequest;
    use ur_rpc::proto::core::core_service_client::CoreServiceClient;

    let dir = tempfile::tempdir().unwrap();

    let (handler, _repo_registry) = make_grpc_handler(dir.path());
    let (channel, _addr) = spawn_grpc_server(handler).await;

    let mut client = CoreServiceClient::new(channel);
    let resp = client.ping(PingRequest {}).await.unwrap();
    assert_eq!(resp.into_inner().message, "pong");
}

#[tokio::test]
async fn grpc_git_exec_streams_output() {
    use ur_rpc::proto::core::command_output::Payload;
    use ur_rpc::proto::git::GitExecRequest;
    use ur_rpc::proto::git::git_service_client::GitServiceClient;

    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    // Create a git repo and register it
    let repo_name = "test-repo";
    let repo_dir = workspace.join(repo_name);
    std::fs::create_dir_all(&repo_dir).unwrap();
    let init = std::process::Command::new("git")
        .args(["init"])
        .current_dir(&repo_dir)
        .output()
        .unwrap();
    assert!(init.status.success(), "git init failed");

    let (handler, repo_registry) = make_grpc_handler(dir.path());
    let process_id = "test-process";
    repo_registry.register(process_id, repo_name);

    // Start a custom gRPC server with our process_id bound to GitServiceHandler.
    use ur_rpc::proto::core::core_service_server::CoreServiceServer;
    use ur_rpc::proto::git::git_service_server::GitServiceServer;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let git_handler = ur_server::grpc_git::GitServiceHandler {
        repo_registry: repo_registry.clone(),
        process_id: process_id.to_string(),
    };

    tokio::spawn(async move {
        Server::builder()
            .add_service(CoreServiceServer::new(handler))
            .add_service(GitServiceServer::new(git_handler))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    // Connect client
    let channel = Endpoint::try_from(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();

    let mut client = GitServiceClient::new(channel);
    let response = client
        .exec(GitExecRequest {
            args: vec!["status".into()],
        })
        .await
        .unwrap();

    let mut rpc_stream = response.into_inner();
    let mut stdout_data = Vec::new();
    let mut exit_code = None;

    while let Some(msg) = futures::StreamExt::next(&mut rpc_stream).await {
        let msg = msg.unwrap();
        match msg.payload {
            Some(Payload::Stdout(data)) => stdout_data.extend_from_slice(&data),
            Some(Payload::Stderr(_)) => {}
            Some(Payload::ExitCode(code)) => exit_code = Some(code),
            None => {}
        }
    }

    assert_eq!(exit_code, Some(0), "git status should exit 0");
    let stdout_str = String::from_utf8_lossy(&stdout_data);
    assert!(
        stdout_str.contains("branch") || stdout_str.contains("No commits"),
        "unexpected streaming stdout: {stdout_str}"
    );
}

#[tokio::test]
async fn grpc_git_exec_blocks_dash_c_flag() {
    use ur_rpc::proto::git::GitExecRequest;
    use ur_rpc::proto::git::git_service_client::GitServiceClient;

    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let (handler, repo_registry) = make_grpc_handler(dir.path());
    let process_id = "test-process";
    repo_registry.register(process_id, "test-repo");

    use ur_rpc::proto::core::core_service_server::CoreServiceServer;
    use ur_rpc::proto::git::git_service_server::GitServiceServer;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let git_handler = ur_server::grpc_git::GitServiceHandler {
        repo_registry: repo_registry.clone(),
        process_id: process_id.to_string(),
    };

    tokio::spawn(async move {
        Server::builder()
            .add_service(CoreServiceServer::new(handler))
            .add_service(GitServiceServer::new(git_handler))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    let channel = Endpoint::try_from(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();

    let mut client = GitServiceClient::new(channel);

    // -C should be blocked with an error
    let result = client
        .exec(GitExecRequest {
            args: vec!["-C".into(), "/tmp".into(), "status".into()],
        })
        .await;

    assert!(result.is_err(), "-C flag should be blocked");
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(status.message().contains("-C"));
}

#[tokio::test]
async fn grpc_git_exec_rejects_blocked_flags() {
    use ur_rpc::proto::git::GitExecRequest;
    use ur_rpc::proto::git::git_service_client::GitServiceClient;

    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let (handler, repo_registry) = make_grpc_handler(dir.path());
    let process_id = "test-process";
    repo_registry.register(process_id, "some-repo");

    use ur_rpc::proto::core::core_service_server::CoreServiceServer;
    use ur_rpc::proto::git::git_service_server::GitServiceServer;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let git_handler = ur_server::grpc_git::GitServiceHandler {
        repo_registry: repo_registry.clone(),
        process_id: process_id.to_string(),
    };

    tokio::spawn(async move {
        Server::builder()
            .add_service(CoreServiceServer::new(handler))
            .add_service(GitServiceServer::new(git_handler))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    let channel = Endpoint::try_from(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();

    let mut client = GitServiceClient::new(channel);

    // --git-dir is still a blocked flag
    let result = client
        .exec(GitExecRequest {
            args: vec!["--git-dir=/tmp".into(), "status".into()],
        })
        .await;

    assert!(result.is_err(), "blocked flag should be rejected");
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(status.message().contains("--git-dir"));
}
