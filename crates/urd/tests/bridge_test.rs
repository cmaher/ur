use std::path::Path;

use futures::StreamExt;
use tarpc::client;
use tarpc::context;
use tarpc::server::{self, Channel};
use tarpc::tokio_serde::formats::Bincode;
use ur_rpc::stream::bind_stream_listener;
use ur_rpc::*;

#[derive(Clone)]
struct StubBridge;

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
        // Create a temporary stream socket that sends a canned response.
        let dir = std::env::temp_dir().join("ur-test-streams");
        let _ = std::fs::create_dir_all(&dir);
        let sock_path = dir.join(format!("stub-{}.sock", std::process::id()));
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
}

async fn spawn_server(sock: &Path) {
    let mut listener = tarpc::serde_transport::unix::listen(sock, Bincode::default)
        .await
        .unwrap();

    tokio::spawn(async move {
        while let Some(Ok(transport)) = listener.next().await {
            let channel = server::BaseChannel::with_defaults(transport);
            let responses = channel.execute(StubBridge.serve());
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

    spawn_server(&sock).await;

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
                process_id: "p1".into(),
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

    spawn_server(&sock).await;

    let transport = tarpc::serde_transport::unix::connect(&sock, Bincode::default)
        .await
        .unwrap();
    let client = UrAgentBridgeClient::new(client::Config::default(), transport).spawn();

    let resp = client
        .exec_git_stream(
            context::current(),
            ExecGitRequest {
                process_id: "p1".into(),
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
