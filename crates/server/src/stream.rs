use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tracing::warn;

use ur_rpc::proto::core::CommandOutput;
use ur_rpc::proto::core::command_output::Payload;

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
