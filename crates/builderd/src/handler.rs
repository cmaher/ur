use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{error, info};

use ur_rpc::proto::core::CommandOutput;
use ur_rpc::proto::builder::BuilderExecRequest;
use ur_rpc::proto::builder::builder_daemon_service_server::BuilderDaemonService;

const WORKSPACE_TEMPLATE: &str = "%WORKSPACE%";

type CommandOutputStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<CommandOutput, Status>> + Send>>;

#[derive(Clone)]
pub struct BuilderDaemonHandler {
    pub workspace: Option<PathBuf>,
}

impl BuilderDaemonHandler {
    fn resolve_working_dir(&self, working_dir: &str) -> String {
        if working_dir.starts_with(WORKSPACE_TEMPLATE)
            && let Some(workspace) = &self.workspace
        {
            let workspace_str = workspace.to_string_lossy();
            return working_dir.replacen(WORKSPACE_TEMPLATE, &workspace_str, 1);
        }
        working_dir.to_string()
    }
}

#[tonic::async_trait]
impl BuilderDaemonService for BuilderDaemonHandler {
    type ExecStream = CommandOutputStream;

    async fn exec(
        &self,
        req: Request<BuilderExecRequest>,
    ) -> Result<Response<Self::ExecStream>, Status> {
        let req = req.into_inner();

        let resolved_dir = self.resolve_working_dir(&req.working_dir);
        let arg_count = req.args.len();

        info!(
            command = %req.command,
            working_dir = %req.working_dir,
            resolved_dir = %resolved_dir,
            arg_count,
            args = ?req.args,
            "host exec request received"
        );

        let mut cmd = tokio::process::Command::new(&req.command);
        cmd.args(&req.args)
            .current_dir(&resolved_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &req.env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().map_err(|e| {
            error!(
                command = %req.command,
                working_dir = %resolved_dir,
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

    fn handler_with_workspace(workspace: Option<&str>) -> BuilderDaemonHandler {
        BuilderDaemonHandler {
            workspace: workspace.map(PathBuf::from),
        }
    }

    #[test]
    fn test_resolve_workspace_with_subpath() {
        let handler = handler_with_workspace(Some("/home/builder/ws"));
        assert_eq!(
            handler.resolve_working_dir("%WORKSPACE%/pool/ur/0"),
            "/home/builder/ws/pool/ur/0"
        );
    }

    #[test]
    fn test_resolve_workspace_alone() {
        let handler = handler_with_workspace(Some("/home/builder/ws"));
        assert_eq!(
            handler.resolve_working_dir("%WORKSPACE%"),
            "/home/builder/ws"
        );
    }

    #[test]
    fn test_resolve_absolute_path_no_replacement() {
        let handler = handler_with_workspace(Some("/home/builder/ws"));
        assert_eq!(
            handler.resolve_working_dir("/absolute/path"),
            "/absolute/path"
        );
    }

    #[test]
    fn test_resolve_empty_string() {
        let handler = handler_with_workspace(Some("/home/builder/ws"));
        assert_eq!(handler.resolve_working_dir(""), "");
    }

    #[test]
    fn test_resolve_workspace_template_without_configured_workspace() {
        let handler = handler_with_workspace(None);
        assert_eq!(
            handler.resolve_working_dir("%WORKSPACE%/pool/ur/0"),
            "%WORKSPACE%/pool/ur/0"
        );
    }

    #[tokio::test]
    async fn test_exec_echo() {
        let handler = handler_with_workspace(None);
        let req = Request::new(BuilderExecRequest {
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
        let handler = handler_with_workspace(None);
        let req = Request::new(BuilderExecRequest {
            command: "nonexistent_command_xyz".into(),
            args: vec![],
            working_dir: "/tmp".into(),
            env: std::collections::HashMap::new(),
        });

        let result = handler.exec(req).await;
        assert!(result.is_err());
    }
}
