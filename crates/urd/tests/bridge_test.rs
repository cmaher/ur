use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use tarpc::client;
use tarpc::context;
use tarpc::server::{self, Channel};
use tarpc::tokio_serde::formats::Bincode;
use ur_rpc::stream::bind_stream_listener;
use ur_rpc::*;

#[derive(Clone)]
struct StubBridge {
    /// Temp directory for stream sockets (shared across clones via Arc).
    stream_dir: Arc<PathBuf>,
}

impl StubBridge {
    fn new(stream_dir: PathBuf) -> Self {
        Self {
            stream_dir: Arc::new(stream_dir),
        }
    }
}

impl UrAgentBridge for StubBridge {
    async fn ping(self, _ctx: context::Context) -> String {
        "pong".into()
    }

    async fn ask_human(
        self,
        _ctx: context::Context,
        req: AskHumanRequest,
    ) -> Result<AskHumanResponse, String> {
        Ok(AskHumanResponse {
            answer: format!("answer to: {}", req.question),
        })
    }

    async fn exec_git(
        self,
        _ctx: context::Context,
        req: ExecGitRequest,
    ) -> Result<GitResponse, String> {
        Ok(GitResponse {
            exit_code: 0,
            stdout: format!("git {}", req.args.join(" ")),
            stderr: String::new(),
        })
    }

    async fn exec_git_stream(
        self,
        _ctx: context::Context,
        _req: ExecGitRequest,
    ) -> Result<StreamingExecResponse, String> {
        // Create a temporary stream socket inside the per-test stream_dir.
        let sock_path = self
            .stream_dir
            .join(format!("stub-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&sock_path);

        let listener = bind_stream_listener(&sock_path).map_err(|e| e.to_string())?;
        let socket_str = sock_path.to_str().unwrap().to_string();

        tokio::spawn(async move {
            use ur_rpc::stream::{accept_stream_sink, send_output};
            if let Ok(mut sink) = accept_stream_sink(listener).await {
                let _ =
                    send_output(&mut sink, CommandOutput::Stdout(b"git output\n".to_vec())).await;
                let _ = send_output(&mut sink, CommandOutput::Exit(0)).await;
            }
            let _ = tokio::fs::remove_file(&sock_path).await;
        });

        Ok(StreamingExecResponse {
            stream_socket: socket_str,
        })
    }

    async fn report_status(
        self,
        _ctx: context::Context,
        _req: ReportStatusRequest,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn ticket_read(
        self,
        _ctx: context::Context,
        req: TicketReadRequest,
    ) -> Result<TicketReadResponse, String> {
        Ok(TicketReadResponse {
            content: format!("# {}", req.ticket_id),
        })
    }

    async fn ticket_spawn(
        self,
        _ctx: context::Context,
        _req: TicketSpawnRequest,
    ) -> Result<TicketSpawnResponse, String> {
        Ok(TicketSpawnResponse {
            ticket_id: "ur-new1".into(),
        })
    }

    async fn ticket_note(
        self,
        _ctx: context::Context,
        _req: TicketNoteRequest,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn container_build(
        self,
        _ctx: context::Context,
        req: ContainerBuildRequest,
    ) -> Result<ContainerBuildResponse, String> {
        Ok(ContainerBuildResponse { image_id: req.tag })
    }

    async fn container_run(
        self,
        _ctx: context::Context,
        req: ContainerRunRequest,
    ) -> Result<ContainerRunResponse, String> {
        Ok(ContainerRunResponse {
            container_id: req.name,
        })
    }

    async fn container_stop(
        self,
        _ctx: context::Context,
        _req: ContainerIdRequest,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn container_rm(
        self,
        _ctx: context::Context,
        _req: ContainerIdRequest,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn container_exec(
        self,
        _ctx: context::Context,
        req: ContainerExecRequest,
    ) -> Result<ContainerExecResponse, String> {
        Ok(ContainerExecResponse {
            exit_code: 0,
            stdout: req.command.join(" "),
            stderr: String::new(),
        })
    }

    async fn process_launch(
        self,
        _ctx: context::Context,
        req: ProcessLaunchRequest,
    ) -> Result<ProcessLaunchResponse, String> {
        Ok(ProcessLaunchResponse {
            container_id: format!("ur-agent-{}", req.process_id),
        })
    }

    async fn process_stop(
        self,
        _ctx: context::Context,
        _req: ProcessStopRequest,
    ) -> Result<(), String> {
        Ok(())
    }
}

async fn spawn_server(sock: &Path, stub: StubBridge) {
    let mut listener = tarpc::serde_transport::unix::listen(sock, Bincode::default)
        .await
        .unwrap();

    tokio::spawn(async move {
        while let Some(Ok(transport)) = listener.next().await {
            let channel = server::BaseChannel::with_defaults(transport);
            let responses = channel.execute(stub.clone().serve());
            tokio::spawn(responses.for_each(|resp| async {
                tokio::spawn(resp);
            }));
        }
    });
}

#[tokio::test]
async fn roundtrip_over_unix_socket() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("test.sock");
    let stub = StubBridge::new(dir.path().to_path_buf());

    spawn_server(&sock, stub).await;

    // Connect client
    let transport = tarpc::serde_transport::unix::connect(&sock, Bincode::default)
        .await
        .unwrap();
    let client = UrAgentBridgeClient::new(client::Config::default(), transport).spawn();

    // ping
    let resp = client.ping(context::current()).await.unwrap();
    assert_eq!(resp, "pong");

    // ask_human
    let resp = client
        .ask_human(
            context::current(),
            AskHumanRequest {
                process_id: "p1".into(),
                question: "hello?".into(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp.answer, "answer to: hello?");

    // exec_git
    let resp = client
        .exec_git(
            context::current(),
            ExecGitRequest {
                args: vec!["status".into()],
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp.exit_code, 0);
    assert_eq!(resp.stdout, "git status");

    // report_status
    client
        .report_status(
            context::current(),
            ReportStatusRequest {
                process_id: "p1".into(),
                status: "working".into(),
            },
        )
        .await
        .unwrap()
        .unwrap();

    // ticket_read
    let resp = client
        .ticket_read(
            context::current(),
            TicketReadRequest {
                ticket_id: "ur-123".into(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp.content, "# ur-123");

    // ticket_spawn
    let resp = client
        .ticket_spawn(
            context::current(),
            TicketSpawnRequest {
                parent_id: "ur-123".into(),
                title: "sub task".into(),
                description: "do stuff".into(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp.ticket_id, "ur-new1");

    // ticket_note
    client
        .ticket_note(
            context::current(),
            TicketNoteRequest {
                ticket_id: "ur-123".into(),
                note: "progress update".into(),
            },
        )
        .await
        .unwrap()
        .unwrap();

    // container_exec
    let resp = client
        .container_exec(
            context::current(),
            ContainerExecRequest {
                container_id: "test-container".into(),
                command: vec!["echo".into(), "hello".into()],
                workdir: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp.exit_code, 0);
    assert_eq!(resp.stdout, "echo hello");
    assert!(resp.stderr.is_empty());
}

#[tokio::test]
async fn exec_git_stream_roundtrip() {
    use ur_rpc::stream::{connect_stream, recv_output};

    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("test.sock");
    let stub = StubBridge::new(dir.path().to_path_buf());

    spawn_server(&sock, stub).await;

    let transport = tarpc::serde_transport::unix::connect(&sock, Bincode::default)
        .await
        .unwrap();
    let client = UrAgentBridgeClient::new(client::Config::default(), transport).spawn();

    let resp = client
        .exec_git_stream(
            context::current(),
            ExecGitRequest {
                args: vec!["status".into()],
            },
        )
        .await
        .unwrap()
        .unwrap();

    // Connect to the side-channel stream socket
    let stream_path = std::path::Path::new(&resp.stream_socket);
    let mut stream = connect_stream(stream_path).await.unwrap();

    let mut stdout_data = Vec::new();
    let mut exit_code = None;

    while let Some(result) = recv_output(&mut stream).await {
        match result.unwrap() {
            CommandOutput::Stdout(data) => stdout_data.extend_from_slice(&data),
            CommandOutput::Stderr(_) => {}
            CommandOutput::Exit(code) => exit_code = Some(code),
        }
    }

    assert_eq!(String::from_utf8(stdout_data).unwrap(), "git output\n");
    assert_eq!(exit_code, Some(0));
}

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
    let process_manager = urd::ProcessManager::new(
        dir.to_path_buf(),
        workspace.clone(),
        repo_registry.clone(),
    );
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
    use ur_rpc::proto::core::core_service_client::CoreServiceClient;
    use ur_rpc::proto::core::PingRequest;

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
    use ur_rpc::proto::git::git_service_client::GitServiceClient;
    use ur_rpc::proto::git::GitExecRequest;

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
    use tonic::transport::Server;
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
    use ur_rpc::proto::git::git_service_client::GitServiceClient;
    use ur_rpc::proto::git::GitExecRequest;

    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("test-git-blocked.sock");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let (handler, repo_registry) = make_grpc_handler(dir.path());
    let process_id = "test-process";
    repo_registry.register(process_id, "some-repo");

    use tokio::net::UnixListener;
    use tokio_stream::wrappers::UnixListenerStream;
    use tonic::transport::Server;
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
