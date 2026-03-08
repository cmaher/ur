use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::warn;

use ur_rpc::proto::core::CommandOutput;
use ur_rpc::proto::core::command_output::Payload;
use ur_rpc::proto::git::GitExecRequest;
use ur_rpc::proto::git::git_service_server::GitService;

use crate::git_exec::RepoRegistry;

/// gRPC implementation of the GitService.
///
/// Each per-agent gRPC server will have its own handler instance with
/// the appropriate `process_id` bound.
#[derive(Clone)]
pub struct GitServiceHandler {
    pub repo_registry: Arc<RepoRegistry>,
    pub process_id: String,
}

type CommandOutputStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<CommandOutput, Status>> + Send>>;

#[tonic::async_trait]
impl GitService for GitServiceHandler {
    type ExecStream = CommandOutputStream;

    async fn exec(
        &self,
        req: Request<GitExecRequest>,
    ) -> Result<Response<Self::ExecStream>, Status> {
        let args = req.into_inner().args;

        // Validate args (reuse from git_exec)
        crate::git_exec::validate_args(&args).map_err(Status::invalid_argument)?;

        // Resolve repo path
        let repo_path = self
            .repo_registry
            .resolve(&self.process_id)
            .map_err(Status::not_found)?;

        // Spawn git process
        let mut child = tokio::process::Command::new("git")
            .args(&args)
            .current_dir(&repo_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Status::internal(format!("failed to spawn git: {e}")))?;

        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (tx, rx) = mpsc::channel(32);

        // Background task: read stdout/stderr, send CommandOutput frames
        tokio::spawn(async move {
            let mut stdout = stdout;
            let mut stderr = stderr;
            let mut stdout_buf = vec![0u8; 8192];
            let mut stderr_buf = vec![0u8; 8192];
            let mut stdout_done = false;
            let mut stderr_done = false;

            loop {
                if stdout_done && stderr_done {
                    break;
                }

                tokio::select! {
                    n = stdout.read(&mut stdout_buf), if !stdout_done => {
                        match n {
                            Ok(0) => stdout_done = true,
                            Ok(n) => {
                                let msg = CommandOutput {
                                    payload: Some(Payload::Stdout(stdout_buf[..n].to_vec())),
                                };
                                if tx.send(Ok(msg)).await.is_err() {
                                    return;
                                }
                            }
                            Err(e) => {
                                warn!("grpc git stream read stdout failed: {e}");
                                stdout_done = true;
                            }
                        }
                    }
                    n = stderr.read(&mut stderr_buf), if !stderr_done => {
                        match n {
                            Ok(0) => stderr_done = true,
                            Ok(n) => {
                                let msg = CommandOutput {
                                    payload: Some(Payload::Stderr(stderr_buf[..n].to_vec())),
                                };
                                if tx.send(Ok(msg)).await.is_err() {
                                    return;
                                }
                            }
                            Err(e) => {
                                warn!("grpc git stream read stderr failed: {e}");
                                stderr_done = true;
                            }
                        }
                    }
                }
            }

            let exit_code = match child.wait().await {
                Ok(status) => status.code().unwrap_or(-1),
                Err(e) => {
                    warn!("grpc git stream wait failed: {e}");
                    -1
                }
            };
            let _ = tx
                .send(Ok(CommandOutput {
                    payload: Some(Payload::ExitCode(exit_code)),
                }))
                .await;
        });

        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream) as Self::ExecStream))
    }
}
