use crate::proto::builder::{
    BuilderExecMessage, BuilderExecRequest, BuilderdClient,
    builder_exec_message::Payload as ExecPayload,
};
use crate::stream::CompletedExec;

/// Extension methods for one-shot command execution via builderd.
impl BuilderdClient {
    /// Execute a command on the host via builderd and return the collected output.
    pub async fn exec_collect(
        &self,
        command: &str,
        args: &[&str],
        working_dir: &str,
    ) -> Result<CompletedExec, String> {
        let mut client = self.clone();

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

        let response = client
            .exec(tokio_stream::once(start_msg))
            .await
            .map_err(|e| format!("builderd exec failed: {e}"))?;

        let stream = response.into_inner();
        CompletedExec::collect(stream)
            .await
            .map_err(|e| format!("builderd stream error: {e}"))
    }

    /// Execute a command on the host via builderd, checking for a zero exit code.
    pub async fn exec_check(
        &self,
        command: &str,
        args: &[&str],
        working_dir: &str,
    ) -> Result<(), String> {
        self.exec_collect(command, args, working_dir)
            .await?
            .check()
            .map(|_| ())
            .map_err(|e| e.message().to_string())
    }
}
