use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tracing::warn;

use crate::proto::core::CommandOutput;
use crate::proto::core::command_output::Payload;

/// Collected result of a completed `CommandOutput` stream.
///
/// Accumulates all stdout/stderr chunks and the final exit code from a
/// `tonic::Streaming<CommandOutput>`. Useful for programmatic callers that
/// don't need real-time streaming and just want the final result.
#[derive(Debug, Clone)]
pub struct CompletedExec {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

impl CompletedExec {
    /// Consume a `tonic::Streaming<CommandOutput>`, collecting all frames
    /// into a single `CompletedExec`. Returns a gRPC error if the stream
    /// fails or ends without an exit code.
    pub async fn collect(
        mut stream: tonic::Streaming<CommandOutput>,
    ) -> Result<Self, tonic::Status> {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code: Option<i32> = None;

        while let Some(msg) = stream.next().await {
            let msg = msg?;
            match msg.payload {
                Some(Payload::Stdout(data)) => stdout.extend_from_slice(&data),
                Some(Payload::Stderr(data)) => stderr.extend_from_slice(&data),
                Some(Payload::ExitCode(code)) => exit_code = Some(code),
                Some(Payload::AlreadyRunning(_)) | None => {}
            }
        }

        let exit_code = exit_code
            .ok_or_else(|| tonic::Status::internal("stream ended without an exit code"))?;

        Ok(Self {
            stdout,
            stderr,
            exit_code,
        })
    }

    /// Return stdout as a lossy UTF-8 string with trailing whitespace trimmed.
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim_end().to_string()
    }

    /// Return stderr as a lossy UTF-8 string with trailing whitespace trimmed.
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim_end().to_string()
    }

    /// Check for a successful (zero) exit code. Returns `Ok(self)` on success,
    /// or an `Err` with the stderr contents as the error message on non-zero exit.
    #[allow(clippy::result_large_err)]
    pub fn check(self) -> Result<Self, tonic::Status> {
        if self.exit_code == 0 {
            Ok(self)
        } else {
            let msg = if self.stderr.is_empty() {
                format!("command exited with code {}", self.exit_code)
            } else {
                format!(
                    "command exited with code {}: {}",
                    self.exit_code,
                    self.stderr_text()
                )
            };
            Err(tonic::Status::internal(msg))
        }
    }
}

/// Stream a child process's stdout/stderr as `CommandOutput` frames,
/// then send the exit code. Spawns a background task that reads from
/// both pipes concurrently and sends frames through the channel.
///
/// The child must have been spawned with `stdout(Stdio::piped())` and
/// `stderr(Stdio::piped())`.
pub fn spawn_child_output_stream(
    mut child: tokio::process::Child,
    tx: mpsc::Sender<Result<CommandOutput, tonic::Status>>,
) {
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");

    tokio::spawn(async move {
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
                            warn!("child stream read stdout failed: {e}");
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
                            warn!("child stream read stderr failed: {e}");
                            stderr_done = true;
                        }
                    }
                }
            }
        }

        let exit_code = match child.wait().await {
            Ok(status) => status.code().unwrap_or(-1),
            Err(e) => {
                warn!("child stream wait failed: {e}");
                -1
            }
        };
        let _ = tx
            .send(Ok(CommandOutput {
                payload: Some(Payload::ExitCode(exit_code)),
            }))
            .await;
    });
}
