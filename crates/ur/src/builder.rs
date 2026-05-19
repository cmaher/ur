use std::process;

use anyhow::Result;
use clap::Subcommand;
use tokio_stream::StreamExt;
use ur_rpc::proto::builder::{
    BuilderExecMessage, BuilderExecRequest, BuilderdClient,
    builder_exec_message::Payload as ExecPayload,
};
use ur_rpc::proto::core::command_output::Payload as OutputPayload;

/// Builder subcommands.
#[derive(Debug, Subcommand)]
pub enum BuilderCommands {
    /// Print the builderd environment (port, workspace, config dir)
    Env,
    /// Locate the named command on the builderd host PATH
    Which {
        /// Command to look up (e.g. "git", "npm")
        command: String,
    },
}

pub async fn handle(command: BuilderCommands, builderd_port: u16) -> Result<()> {
    match command {
        BuilderCommands::Env => {
            let exit_code = exec_on_builderd(builderd_port, "env", &[]).await?;
            process::exit(exit_code);
        }
        BuilderCommands::Which { command } => {
            let exit_code = exec_on_builderd(builderd_port, "which", &[&command]).await?;
            process::exit(exit_code);
        }
    }
}

/// Connect to builderd and execute a command, streaming stdout/stderr in real time.
///
/// Returns the remote exit code on success. On connection error, prints a
/// user-friendly message and exits with code 1.
async fn exec_on_builderd(port: u16, command: &str, args: &[&str]) -> Result<i32> {
    let addr = format!("http://127.0.0.1:{port}");
    let mut client = BuilderdClient::connect(addr).await.inspect_err(|_| {
        eprintln!("builderd is not running (start with 'ur start')");
    })?;

    let req = BuilderExecRequest {
        command: command.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        working_dir: "/tmp".to_string(),
        env: std::collections::HashMap::new(),
        long_lived: false,
    };

    let start_msg = BuilderExecMessage {
        payload: Some(ExecPayload::Start(req)),
    };

    let response = client
        .exec(tokio_stream::once(start_msg))
        .await
        .inspect_err(|_| {
            eprintln!("builderd is not running (start with 'ur start')");
        })?;

    let mut stream = response.into_inner();
    let mut exit_code = 1i32;

    while let Some(msg) = stream.next().await {
        let msg = msg?;
        match msg.payload {
            Some(OutputPayload::Stdout(data)) => {
                use std::io::Write;
                std::io::stdout().write_all(&data)?;
                std::io::stdout().flush()?;
            }
            Some(OutputPayload::Stderr(data)) => {
                use std::io::Write;
                std::io::stderr().write_all(&data)?;
                std::io::stderr().flush()?;
            }
            Some(OutputPayload::ExitCode(code)) => {
                exit_code = code;
            }
            Some(OutputPayload::AlreadyRunning(_)) | None => {}
        }
    }

    Ok(exit_code)
}
