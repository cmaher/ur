use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{error, info, warn};

use ur_rpc::proto::builder::builder_daemon_service_server::BuilderDaemonService;
use ur_rpc::proto::builder::builder_exec_message::Payload as ExecPayload;
use ur_rpc::proto::builder::{BuilderExecMessage, BuilderExecRequest};
use ur_rpc::proto::core::CommandOutput;
use ur_rpc::proto::core::command_output::Payload;

use crate::registry::{OutputSink, ProcessKey, ProcessRegistry, RegisteredProcess};

const WORKSPACE_TEMPLATE: &str = "%WORKSPACE%";

type CommandOutputStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<CommandOutput, Status>> + Send>>;

#[derive(Clone)]
pub struct BuilderDaemonHandler {
    pub workspace: Option<PathBuf>,
    pub registry: ProcessRegistry,
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

    fn process_key(&self, req: &BuilderExecRequest) -> ProcessKey {
        let resolved_dir = self.resolve_working_dir(&req.working_dir);
        (req.command.clone(), resolved_dir)
    }

    /// Spawn a short-lived command and return the output stream.
    #[allow(clippy::result_large_err)]
    fn spawn_command(
        &self,
        req: &BuilderExecRequest,
    ) -> Result<Response<CommandOutputStream>, Status> {
        let resolved_dir = self.resolve_working_dir(&req.working_dir);
        let arg_count = req.args.len();

        info!(
            command = %req.command,
            working_dir = %req.working_dir,
            resolved_dir = %resolved_dir,
            arg_count,
            args = ?req.args,
            long_lived = req.long_lived,
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
        Ok(Response::new(Box::pin(stream) as CommandOutputStream))
    }

    /// Handle a long-lived process request: check registry for deduplication,
    /// return already_running ack or spawn new process and register it.
    #[allow(clippy::result_large_err)]
    fn spawn_long_lived(
        &self,
        req: &BuilderExecRequest,
    ) -> Result<(Response<CommandOutputStream>, mpsc::Sender<Vec<u8>>), Status> {
        let resolved_dir = self.resolve_working_dir(&req.working_dir);
        let key = self.process_key(req);

        // Check if already running — reconnect scenario
        if self.registry.is_running(&key) {
            info!(
                command = %req.command,
                working_dir = %resolved_dir,
                "long-lived process already running, reconnecting"
            );

            let stdin_tx = self
                .registry
                .get_stdin_tx(&key)
                .ok_or_else(|| Status::internal("process was running but stdin_tx disappeared"))?;

            // Create a new output channel and wire it into the existing process
            let (out_tx, out_rx) = mpsc::channel(32);

            // Send already_running ack on the new channel first
            let ack = CommandOutput {
                payload: Some(Payload::AlreadyRunning(true)),
            };
            out_tx.try_send(Ok(ack)).map_err(|e| {
                Status::internal(format!("failed to send already_running ack: {e}"))
            })?;

            // Replace the output sink so the forwarder starts sending to this caller
            self.registry
                .replace_output_sink(&key, out_tx)
                .ok_or_else(|| {
                    Status::internal("process was running but output sink disappeared")
                })?;

            let stream = ReceiverStream::new(out_rx);
            return Ok((
                Response::new(Box::pin(stream) as CommandOutputStream),
                stdin_tx,
            ));
        }

        let arg_count = req.args.len();
        info!(
            command = %req.command,
            working_dir = %req.working_dir,
            resolved_dir = %resolved_dir,
            arg_count,
            args = ?req.args,
            long_lived = true,
            "spawning new long-lived process"
        );

        let mut cmd = tokio::process::Command::new(&req.command);
        cmd.args(&req.args)
            .current_dir(&resolved_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &req.env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().map_err(|e| {
            error!(
                command = %req.command,
                working_dir = %resolved_dir,
                error = %e,
                "failed to spawn long-lived process"
            );
            Status::internal(format!("failed to spawn {}: {e}", req.command))
        })?;

        let child_stdin = child.stdin.take().expect("stdin piped for long_lived");
        let (stdin_tx, stdin_rx) = mpsc::channel::<Vec<u8>>(32);
        let (out_tx, out_rx) = mpsc::channel(32);

        // Track whether the child is still alive via an intermediate channel.
        // spawn_child_output_stream takes ownership of its tx; when it finishes
        // (child exits), it drops that tx, closing the intermediate channel.
        // A watcher task detects the close and flips the alive flag.
        let alive = Arc::new(AtomicBool::new(true));
        let alive_clone = alive.clone();
        let (intermediate_tx, intermediate_rx) = mpsc::channel::<Result<CommandOutput, Status>>(32);

        ur_rpc::stream::spawn_child_output_stream(child, intermediate_tx);

        // Create the replaceable output sink with the initial caller's sender
        let output_sink: OutputSink = Arc::new(std::sync::Mutex::new(Some(out_tx)));

        // Forward from intermediate channel through the output sink, then mark dead.
        let sink_clone = output_sink.clone();
        tokio::spawn(Self::forward_output_via_sink(
            intermediate_rx,
            sink_clone,
            alive_clone,
        ));

        // Spawn stdin forwarder
        tokio::spawn(Self::forward_stdin(child_stdin, stdin_rx));

        // Register the process
        self.registry.register(
            key,
            RegisteredProcess {
                alive,
                stdin_tx: stdin_tx.clone(),
                output_sink,
            },
        );

        let stream = ReceiverStream::new(out_rx);
        Ok((
            Response::new(Box::pin(stream) as CommandOutputStream),
            stdin_tx,
        ))
    }

    /// Forward child output through a replaceable output sink.
    ///
    /// On each message, locks the sink to get the current sender. If no sender
    /// is present (caller disconnected), the message is silently dropped — no
    /// buffering between disconnections.
    async fn forward_output_via_sink(
        mut rx: mpsc::Receiver<Result<CommandOutput, Status>>,
        sink: OutputSink,
        alive: Arc<AtomicBool>,
    ) {
        while let Some(msg) = rx.recv().await {
            let maybe_tx = {
                let guard = sink.lock().expect("output sink lock poisoned");
                guard.clone()
            };
            if let Some(tx) = maybe_tx
                && tx.send(msg).await.is_err()
            {
                // Caller disconnected — clear the sink so future messages are dropped
                let mut guard = sink.lock().expect("output sink lock poisoned");
                *guard = None;
            }
            // If no sender, silently drop the message
        }
        alive.store(false, Ordering::Relaxed);
    }

    async fn forward_stdin(mut stdin: tokio::process::ChildStdin, mut rx: mpsc::Receiver<Vec<u8>>) {
        while let Some(data) = rx.recv().await {
            if let Err(e) = stdin.write_all(&data).await {
                warn!(error = %e, "failed to write to child stdin");
                break;
            }
            if let Err(e) = stdin.flush().await {
                warn!(error = %e, "failed to flush child stdin");
                break;
            }
        }
    }
}

#[tonic::async_trait]
impl BuilderDaemonService for BuilderDaemonHandler {
    type ExecStream = CommandOutputStream;

    async fn exec(
        &self,
        req: Request<tonic::Streaming<BuilderExecMessage>>,
    ) -> Result<Response<Self::ExecStream>, Status> {
        let mut in_stream = req.into_inner();

        // First message must be a start frame.
        let first = in_stream
            .message()
            .await
            .map_err(|e| Status::internal(format!("failed to read start frame: {e}")))?
            .ok_or_else(|| Status::invalid_argument("empty request stream"))?;

        let req = match first.payload {
            Some(ExecPayload::Start(start)) => start,
            _ => {
                return Err(Status::invalid_argument(
                    "first message must be a start frame",
                ));
            }
        };

        if req.long_lived {
            let (resp, stdin_tx) = self.spawn_long_lived(&req)?;

            // Forward subsequent stdin messages from the bidi stream to the process
            tokio::spawn(async move {
                while let Some(Ok(msg)) = in_stream.next().await {
                    if let Some(ExecPayload::Stdin(data)) = msg.payload
                        && stdin_tx.send(data).await.is_err()
                    {
                        break;
                    }
                }
            });

            Ok(resp)
        } else {
            self.spawn_command(&req)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_stream::StreamExt;
    use ur_rpc::proto::core::command_output::Payload;

    async fn has_stdout_in_next(
        stream: &mut (impl tokio_stream::Stream<Item = Result<CommandOutput, Status>> + Unpin),
        attempts: usize,
    ) -> bool {
        for _ in 0..attempts {
            if let Some(Ok(msg)) = stream.next().await
                && matches!(msg.payload, Some(Payload::Stdout(_)))
            {
                return true;
            }
        }
        false
    }

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
            registry: ProcessRegistry::new(),
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
        let req = BuilderExecRequest {
            command: "echo".into(),
            args: vec!["hello".into()],
            working_dir: "/tmp".into(),
            env: std::collections::HashMap::new(),
            long_lived: false,
        };

        let resp = handler.spawn_command(&req).unwrap();
        let (stdout_data, exit_code) = collect_stream(resp.into_inner()).await;

        assert_eq!(String::from_utf8_lossy(&stdout_data).trim(), "hello");
        assert_eq!(exit_code, Some(0));
    }

    #[tokio::test]
    async fn test_exec_nonexistent_command() {
        let handler = handler_with_workspace(None);
        let req = BuilderExecRequest {
            command: "nonexistent_command_xyz".into(),
            args: vec![],
            working_dir: "/tmp".into(),
            env: std::collections::HashMap::new(),
            long_lived: false,
        };

        let result = handler.spawn_command(&req);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_long_lived_already_running() {
        let handler = handler_with_workspace(None);
        let req = BuilderExecRequest {
            command: "sleep".into(),
            args: vec!["60".into()],
            working_dir: "/tmp".into(),
            env: std::collections::HashMap::new(),
            long_lived: true,
        };

        // First call should spawn
        let (resp1, _stdin1) = handler.spawn_long_lived(&req).unwrap();
        // Don't consume the stream — just check the second call returns already_running
        let _ = resp1;

        // Second call should return already_running
        let (resp2, _stdin2) = handler.spawn_long_lived(&req).unwrap();
        let mut stream = resp2.into_inner();
        if let Some(Ok(msg)) = stream.next().await {
            assert!(
                matches!(msg.payload, Some(Payload::AlreadyRunning(true))),
                "expected already_running, got {:?}",
                msg.payload
            );
        } else {
            panic!("expected already_running message");
        }
    }

    #[tokio::test]
    async fn test_long_lived_reconnect_receives_output() {
        let handler = handler_with_workspace(None);
        let req = BuilderExecRequest {
            command: "bash".into(),
            args: vec![
                "-c".into(),
                "for i in $(seq 1 100); do echo line$i; sleep 0.05; done".into(),
            ],
            working_dir: "/tmp".into(),
            env: std::collections::HashMap::new(),
            long_lived: true,
        };

        // First call spawns the process
        let (resp1, _stdin1) = handler.spawn_long_lived(&req).unwrap();
        let mut stream1 = resp1.into_inner();

        // Read a few messages from stream1 to confirm it's working
        assert!(
            has_stdout_in_next(&mut stream1, 3).await,
            "first caller should receive stdout"
        );

        // Drop stream1 to simulate disconnect
        drop(stream1);

        // Small delay to let some output be dropped
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Reconnect — second call should get already_running ack, then new output
        let (resp2, _stdin2) = handler.spawn_long_lived(&req).unwrap();
        let mut stream2 = resp2.into_inner();

        // First message should be already_running ack
        let first = stream2.next().await;
        assert!(
            matches!(
                first,
                Some(Ok(CommandOutput {
                    payload: Some(Payload::AlreadyRunning(true))
                }))
            ),
            "expected already_running ack on reconnect"
        );

        // Should receive subsequent output on the new stream
        assert!(
            has_stdout_in_next(&mut stream2, 20).await,
            "reconnected caller should receive output"
        );
    }
}
