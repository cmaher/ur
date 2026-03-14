use std::pin::Pin;
use std::process::Stdio;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{error, info};

use ur_rpc::proto::core::CommandOutput;
use ur_rpc::proto::hostd::HostDaemonExecRequest;
use ur_rpc::proto::hostd::host_daemon_service_server::HostDaemonService;

type CommandOutputStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<CommandOutput, Status>> + Send>>;

#[derive(Clone)]
pub struct HostDaemonHandler;

#[tonic::async_trait]
impl HostDaemonService for HostDaemonHandler {
    type ExecStream = CommandOutputStream;

    async fn exec(
        &self,
        req: Request<HostDaemonExecRequest>,
    ) -> Result<Response<Self::ExecStream>, Status> {
        let req = req.into_inner();

        let arg_count = req.args.len();

        info!(
            command = %req.command,
            working_dir = %req.working_dir,
            arg_count,
            args = ?req.args,
            "host exec request received"
        );

        let mut cmd = tokio::process::Command::new(&req.command);
        cmd.args(&req.args)
            .current_dir(&req.working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &req.env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().map_err(|e| {
            error!(
                command = %req.command,
                working_dir = %req.working_dir,
                error = %e,
                "failed to spawn process"
            );
            Status::internal(format!("failed to spawn {}: {e}", req.command))
        })?;

        let (tx, rx) = mpsc::channel(32);
        ur_rpc::stream::spawn_child_output_stream(child, tx);

        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream) as Self::ExecStream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_stream::StreamExt;
    use ur_rpc::proto::core::command_output::Payload;

    async fn collect_stream(
        mut stream: impl tokio_stream::Stream<Item = Result<CommandOutput, Status>> + Unpin,
    ) -> (Vec<u8>, Option<i32>) {
        let mut stdout_data = Vec::new();
        let mut exit_code = None;
        while let Some(Ok(msg)) = stream.next().await {
            match msg.payload {
                Some(Payload::Stdout(data)) => stdout_data.extend(data),
                Some(Payload::ExitCode(code)) => exit_code = Some(code),
                _ => {}
            }
        }
        (stdout_data, exit_code)
    }

    #[tokio::test]
    async fn test_exec_echo() {
        let handler = HostDaemonHandler;
        let req = Request::new(HostDaemonExecRequest {
            command: "echo".into(),
            args: vec!["hello".into()],
            working_dir: "/tmp".into(),
            env: std::collections::HashMap::new(),
        });

        let resp = handler.exec(req).await.unwrap();
        let (stdout_data, exit_code) = collect_stream(resp.into_inner()).await;

        assert_eq!(String::from_utf8_lossy(&stdout_data).trim(), "hello");
        assert_eq!(exit_code, Some(0));
    }

    #[tokio::test]
    async fn test_exec_nonexistent_command() {
        let handler = HostDaemonHandler;
        let req = Request::new(HostDaemonExecRequest {
            command: "nonexistent_command_xyz".into(),
            args: vec![],
            working_dir: "/tmp".into(),
            env: std::collections::HashMap::new(),
        });

        let result = handler.exec(req).await;
        assert!(result.is_err());
    }
}
