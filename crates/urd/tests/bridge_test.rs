use std::path::Path;

use futures::StreamExt;
use tarpc::client;
use tarpc::context;
use tarpc::server::{self, Channel};
use tarpc::tokio_serde::formats::Bincode;
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
}
