use std::path::PathBuf;

use clap::Parser;
use futures::StreamExt;
use tarpc::server::{self, Channel};
use tarpc::tokio_serde::formats::Bincode;
use tracing::info;

use ur_rpc::*;

#[derive(Parser)]
#[command(
    name = "urd",
    about = "Ur daemon — coordination server for containerized agents"
)]
struct Cli {
    /// Socket directory for agent UDS connections
    #[arg(long, default_value = "/tmp/ur/sockets")]
    socket_dir: PathBuf,
}

#[derive(Clone)]
struct BridgeServer;

impl UrAgentBridge for BridgeServer {
    async fn ask_human(
        self,
        _ctx: tarpc::context::Context,
        _req: AskHumanRequest,
    ) -> Result<AskHumanResponse, String> {
        Err("ask_human not yet implemented".into())
    }

    async fn exec_git(
        self,
        _ctx: tarpc::context::Context,
        _req: ExecGitRequest,
    ) -> Result<GitResponse, String> {
        Err("exec_git not yet implemented".into())
    }

    async fn report_status(
        self,
        _ctx: tarpc::context::Context,
        _req: ReportStatusRequest,
    ) -> Result<(), String> {
        Err("report_status not yet implemented".into())
    }

    async fn ticket_read(
        self,
        _ctx: tarpc::context::Context,
        _req: TicketReadRequest,
    ) -> Result<TicketReadResponse, String> {
        Err("ticket_read not yet implemented".into())
    }

    async fn ticket_spawn(
        self,
        _ctx: tarpc::context::Context,
        _req: TicketSpawnRequest,
    ) -> Result<TicketSpawnResponse, String> {
        Err("ticket_spawn not yet implemented".into())
    }

    async fn ticket_note(
        self,
        _ctx: tarpc::context::Context,
        _req: TicketNoteRequest,
    ) -> Result<(), String> {
        Err("ticket_note not yet implemented".into())
    }
}

async fn accept_loop(socket_path: PathBuf) -> anyhow::Result<()> {
    // Remove stale socket if it exists
    let _ = tokio::fs::remove_file(&socket_path).await;

    let mut listener =
        tarpc::serde_transport::unix::listen(&socket_path, Bincode::default).await?;
    info!("urd listening on {}", socket_path.display());

    while let Some(transport) = listener.next().await {
        let transport = transport?;
        let channel = server::BaseChannel::with_defaults(transport);
        tokio::spawn(
            channel
                .execute(BridgeServer.serve())
                .for_each(|response| async {
                    tokio::spawn(response);
                }),
        );
    }

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    tokio::fs::create_dir_all(&cli.socket_dir).await?;

    let socket_path = cli.socket_dir.join("ur.sock");
    accept_loop(socket_path).await
}
