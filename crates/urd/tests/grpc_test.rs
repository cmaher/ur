use std::path::Path;
use std::sync::Arc;

use tonic::transport::Server;

/// Helper: start a gRPC server and return a connected channel.
async fn spawn_grpc_server(
    socket_path: std::path::PathBuf,
    handler: urd::grpc::CoreServiceHandler,
) -> tonic::transport::Channel {
    use hyper_util::rt::TokioIo;
    use tokio::net::UnixStream;
    use tonic::transport::{Endpoint, Uri};
    use tower::service_fn;

    let sp = socket_path.clone();
    tokio::spawn(async move {
        urd::grpc_server::serve_grpc(&sp, handler).await.unwrap();
    });

    // Wait for the socket file to appear
    for _ in 0..50 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    Endpoint::try_from("http://[::]:50051")
        .unwrap()
        .connect_with_connector(service_fn(move |_: Uri| {
            let path = socket_path.clone();
            async move {
                let stream = UnixStream::connect(path).await?;
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        }))
        .await
        .unwrap()
}

/// Helper: create a CoreServiceHandler from a temp dir with workspace.
fn make_grpc_handler(dir: &Path) -> (urd::grpc::CoreServiceHandler, Arc<urd::RepoRegistry>) {
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let repo_registry = Arc::new(urd::RepoRegistry::new(workspace.clone()));
    let process_manager =
        urd::ProcessManager::new(dir.to_path_buf(), workspace.clone(), repo_registry.clone());
    let handler = urd::grpc::CoreServiceHandler {
        process_manager,
        repo_registry: repo_registry.clone(),
        config_dir: dir.to_path_buf(),
        workspace,
    };
    (handler, repo_registry)
}

#[tokio::test]
async fn grpc_ping_over_unix_socket() {
    use ur_rpc::proto::core::PingRequest;
    use ur_rpc::proto::core::core_service_client::CoreServiceClient;

    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("test-grpc.sock");

    let (handler, _repo_registry) = make_grpc_handler(dir.path());
    let channel = spawn_grpc_server(socket_path, handler).await;

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
    let socket_path = dir.path().join("test-git-grpc.sock");
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
    use tokio::net::UnixListener;
    use tokio_stream::wrappers::UnixListenerStream;
    use ur_rpc::proto::core::core_service_server::CoreServiceServer;
    use ur_rpc::proto::git::git_service_server::GitServiceServer;

    let _ = tokio::fs::remove_file(&socket_path).await;
    let listener = UnixListener::bind(&socket_path).unwrap();
    let uds_stream = UnixListenerStream::new(listener);

    let git_handler = urd::grpc_git::GitServiceHandler {
        repo_registry: repo_registry.clone(),
        process_id: process_id.to_string(),
    };

    let sp = socket_path.clone();
    tokio::spawn(async move {
        Server::builder()
            .add_service(CoreServiceServer::new(handler))
            .add_service(GitServiceServer::new(git_handler))
            .serve_with_incoming(uds_stream)
            .await
            .unwrap();
    });

    // Wait for socket
    for _ in 0..50 {
        if sp.exists() {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // Connect client
    use hyper_util::rt::TokioIo;
    use tokio::net::UnixStream;
    use tonic::transport::{Endpoint, Uri};
    use tower::service_fn;

    let channel = Endpoint::try_from("http://[::]:50051")
        .unwrap()
        .connect_with_connector(service_fn(move |_: Uri| {
            let path = sp.clone();
            async move {
                let stream = UnixStream::connect(path).await?;
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        }))
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
async fn grpc_git_exec_rejects_blocked_flags() {
    use ur_rpc::proto::git::GitExecRequest;
    use ur_rpc::proto::git::git_service_client::GitServiceClient;

    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("test-git-blocked.sock");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let (handler, repo_registry) = make_grpc_handler(dir.path());
    let process_id = "test-process";
    repo_registry.register(process_id, "some-repo");

    use tokio::net::UnixListener;
    use tokio_stream::wrappers::UnixListenerStream;
    use ur_rpc::proto::core::core_service_server::CoreServiceServer;
    use ur_rpc::proto::git::git_service_server::GitServiceServer;

    let _ = tokio::fs::remove_file(&socket_path).await;
    let listener = UnixListener::bind(&socket_path).unwrap();
    let uds_stream = UnixListenerStream::new(listener);

    let git_handler = urd::grpc_git::GitServiceHandler {
        repo_registry: repo_registry.clone(),
        process_id: process_id.to_string(),
    };

    let sp = socket_path.clone();
    tokio::spawn(async move {
        Server::builder()
            .add_service(CoreServiceServer::new(handler))
            .add_service(GitServiceServer::new(git_handler))
            .serve_with_incoming(uds_stream)
            .await
            .unwrap();
    });

    for _ in 0..50 {
        if sp.exists() {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    use hyper_util::rt::TokioIo;
    use tokio::net::UnixStream;
    use tonic::transport::{Endpoint, Uri};
    use tower::service_fn;

    let channel = Endpoint::try_from("http://[::]:50051")
        .unwrap()
        .connect_with_connector(service_fn(move |_: Uri| {
            let path = sp.clone();
            async move {
                let uds = UnixStream::connect(path).await?;
                Ok::<_, std::io::Error>(TokioIo::new(uds))
            }
        }))
        .await
        .unwrap();

    let mut client = GitServiceClient::new(channel);
    let result = client
        .exec(GitExecRequest {
            args: vec!["-C".into(), "/tmp".into(), "status".into()],
        })
        .await;

    assert!(result.is_err(), "blocked flag should be rejected");
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(status.message().contains("-C"));
}
