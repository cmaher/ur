use ur_rpc::proto::builder::BuilderExecMessage;
use ur_rpc::proto::builder::BuilderExecRequest;
use ur_rpc::proto::builder::builder_daemon_service_client::BuilderDaemonServiceClient;
use ur_rpc::proto::builder::builder_exec_message::Payload as ExecPayload;
use ur_rpc::stream::CompletedExec;

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

    /// Execute a command on the host via builderd and return the collected
    /// output. Returns an error if builderd is unreachable, the stream fails,
    /// or the stream ends without an exit code.
    pub async fn exec_and_collect(
        &self,
        command: &str,
        args: &[&str],
        working_dir: &str,
    ) -> Result<CompletedExec, String> {
        let mut client = BuilderDaemonServiceClient::connect(self.builderd_addr.clone())
            .await
            .map_err(|e| format!("builderd unavailable: {e}"))?;

        let req = BuilderExecRequest {
            command: command.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            working_dir: working_dir.to_string(),
            env: std::collections::HashMap::new(),
            long_lived: false,
        };

        let start_msg = BuilderExecMessage {
            payload: Some(ExecPayload::Start(req)),
        };

        // Send a single start frame (no stdin) — backwards compatible one-shot execution.
        let response = client
            .exec(tokio_stream::once(start_msg))
            .await
            .map_err(|e| format!("builderd exec failed: {e}"))?;

        let stream = response.into_inner();
        CompletedExec::collect(stream)
            .await
            .map_err(|e| format!("builderd stream error: {e}"))
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
        self.exec_and_collect(command, args, working_dir)
            .await?
            .check()
            .map(|_| ())
            .map_err(|e| e.message().to_string())
    }
}
