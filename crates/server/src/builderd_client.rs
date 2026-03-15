use tokio_stream::StreamExt;
use ur_rpc::proto::builder::BuilderExecRequest;
use ur_rpc::proto::builder::builder_daemon_service_client::BuilderDaemonServiceClient;
use ur_rpc::proto::core::command_output::Payload;

/// Thin client for executing commands on the host via builderd.
///
/// Connects to the builderd gRPC daemon, runs a command, collects output,
/// and checks the exit code. Used by `RepoPoolManager` for git operations
/// and potentially other server-side code that needs host execution.
#[derive(Clone)]
pub struct BuilderdClient {
    /// Address of the builderd daemon (e.g., `http://host.docker.internal:42070`).
    builderd_addr: String,
}

impl BuilderdClient {
    pub fn new(builderd_addr: String) -> Self {
        Self { builderd_addr }
    }

    pub fn addr(&self) -> &str {
        &self.builderd_addr
    }

    /// Execute a command on the host via builderd, collecting output and
    /// checking the exit code. Returns an error if builderd is unreachable, the
    /// command exits non-zero, or the stream ends without an exit code.
    pub async fn exec_and_check(
        &self,
        command: &str,
        args: &[&str],
        working_dir: &str,
    ) -> Result<(), String> {
        let mut client = BuilderDaemonServiceClient::connect(self.builderd_addr.clone())
            .await
            .map_err(|e| format!("builderd unavailable: {e}"))?;

        let req = BuilderExecRequest {
            command: command.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            working_dir: working_dir.to_string(),
            env: std::collections::HashMap::new(),
        };

        let response = client
            .exec(req)
            .await
            .map_err(|e| format!("builderd exec failed: {e}"))?;

        let mut stream = response.into_inner();
        let mut stderr_buf = Vec::new();
        let mut exit_code: Option<i32> = None;

        while let Some(msg) = stream.next().await {
            let msg = msg.map_err(|e| format!("builderd stream error: {e}"))?;
            match msg.payload {
                Some(Payload::Stderr(data)) => stderr_buf.extend(data),
                Some(Payload::ExitCode(code)) => exit_code = Some(code),
                _ => {}
            }
        }

        match exit_code {
            Some(0) => Ok(()),
            Some(code) => {
                let stderr = String::from_utf8_lossy(&stderr_buf);
                Err(format!("exit code {code}: {stderr}"))
            }
            None => Err("builderd stream ended without exit code".into()),
        }
    }
}
