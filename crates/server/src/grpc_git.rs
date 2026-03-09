use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use ur_rpc::proto::core::CommandOutput;
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

        // Validate args (blocks -C, --git-dir, --work-tree)
        crate::git_exec::validate_args(&args).map_err(Status::invalid_argument)?;

        // Resolve repo path
        let repo_path = self
            .repo_registry
            .resolve(&self.process_id)
            .map_err(Status::not_found)?;

        // Spawn git process
        let child = tokio::process::Command::new("git")
            .args(&args)
            .current_dir(&repo_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Status::internal(format!("failed to spawn git: {e}")))?;

        let (tx, rx) = mpsc::channel(32);

        crate::stream::spawn_child_output_stream(child, tx);

        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream) as Self::ExecStream))
    }
}
