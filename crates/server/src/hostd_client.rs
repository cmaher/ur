use tokio_stream::StreamExt;
use ur_rpc::proto::core::command_output::Payload;
use ur_rpc::proto::hostd::HostDaemonExecRequest;
use ur_rpc::proto::hostd::host_daemon_service_client::HostDaemonServiceClient;

/// Thin client for executing commands on the host via ur-hostd.
///
/// Connects to the ur-hostd gRPC daemon, runs a command, collects output,
/// and checks the exit code. Used by `RepoPoolManager` for git operations
/// and potentially other server-side code that needs host execution.
#[derive(Clone)]
pub struct HostDaemonClientManager {
    /// Address of the ur-hostd daemon (e.g., `http://host.docker.internal:42070`).
    hostd_addr: String,
}

impl HostDaemonClientManager {
    pub fn new(hostd_addr: String) -> Self {
        Self { hostd_addr }
    }

    pub fn addr(&self) -> &str {
        &self.hostd_addr
    }

    /// Execute a command on the host via ur-hostd, collecting output and
    /// checking the exit code. Returns an error if hostd is unreachable, the
    /// command exits non-zero, or the stream ends without an exit code.
    pub async fn exec_and_check(
        &self,
        command: &str,
        args: &[&str],
        working_dir: &str,
    ) -> Result<(), String> {
        let mut client = HostDaemonServiceClient::connect(self.hostd_addr.clone())
            .await
            .map_err(|e| format!("hostd unavailable: {e}"))?;

        let req = HostDaemonExecRequest {
            command: command.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            working_dir: working_dir.to_string(),
        };

        let response = client
            .exec(req)
            .await
            .map_err(|e| format!("hostd exec failed: {e}"))?;

        let mut stream = response.into_inner();
        let mut stderr_buf = Vec::new();
        let mut exit_code: Option<i32> = None;

        while let Some(msg) = stream.next().await {
            let msg = msg.map_err(|e| format!("hostd stream error: {e}"))?;
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
            None => Err("hostd stream ended without exit code".into()),
        }
    }
}
