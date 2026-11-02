use std::path::{Path, PathBuf};
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

use ur_config::{WORKSPACE_TEMPLATE, resolve_workspace_content};

use crate::overlay::{LocalOverlayManager, OVERLAY_BRACKETED_COMMANDS, OverlayLease};
use crate::registry::{OutputSink, ProcessKey, ProcessRegistry, RegisteredProcess};

type CommandOutputStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<CommandOutput, Status>> + Send>>;

#[derive(Clone)]
pub struct BuilderDaemonHandler {
    pub workspace: Option<PathBuf>,
    pub registry: ProcessRegistry,
    /// Conceals pool-slot local overlays around git commands so git never sees them.
    pub overlays: LocalOverlayManager,
}

impl BuilderDaemonHandler {
    fn resolve_working_dir(&self, working_dir: &str) -> String {
        if working_dir.contains(WORKSPACE_TEMPLATE)
            && let Some(workspace) = &self.workspace
        {
            let workspace_str = workspace.to_string_lossy();
            return resolve_workspace_content(working_dir, &workspace_str);
        }
        working_dir.to_string()
    }

    fn process_key(&self, req: &BuilderExecRequest) -> ProcessKey {
        let resolved_dir = self.resolve_working_dir(&req.working_dir);
        (req.command.clone(), resolved_dir)
    }

    /// Spawn a short-lived command and return the output stream plus a sender
    /// for its stdin.
    ///
    /// stdin is piped rather than null so bidi commands (e.g. `gh api --input -`)
    /// can be fed from the caller's forwarded stdin. When the caller's stream
    /// ends, the sender is dropped, which closes the child's stdin and yields
    /// EOF — the same thing a non-bidi caller previously got from `Stdio::null`.
    ///
    /// Git commands run inside a pool slot are bracketed by the overlay manager: local
    /// overlays are concealed before the child spawns and revealed before its exit code
    /// is forwarded, so the caller sees its overlays again as soon as the command returns.
    async fn spawn_command(
        &self,
        req: &BuilderExecRequest,
    ) -> Result<(Response<CommandOutputStream>, mpsc::Sender<Vec<u8>>), Status> {
        let resolved_dir = self.resolve_working_dir(&req.working_dir);
        let resolved_command = self.resolve_working_dir(&req.command);
        let arg_count = req.args.len();

        info!(
            command = %req.command,
            resolved_command = %resolved_command,
            working_dir = %req.working_dir,
            resolved_dir = %resolved_dir,
            arg_count,
            args = ?req.args,
            long_lived = req.long_lived,
            "host exec request received"
        );

        let mut cmd = tokio::process::Command::new(&resolved_command);
        cmd.args(&req.args)
            .current_dir(&resolved_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &req.env {
            cmd.env(k, v);
        }
        let lease = self.conceal_overlays(req, &resolved_dir).await?;
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                error!(
                    command = %resolved_command,
                    working_dir = %resolved_dir,
                    error = %e,
                    "failed to spawn process"
                );
                let mut message = format!("failed to spawn {resolved_command}: {e}");
                if let Some(lease) = lease
                    && let Err(reveal_err) = self.overlays.reveal_after(lease).await
                {
                    message.push_str(&format!("; restoring local overlays failed: {reveal_err}"));
                }
                return Err(Status::internal(message));
            }
        };

        let child_stdin = child.stdin.take().expect("stdin piped for short-lived");
        let (stdin_tx, stdin_rx) = mpsc::channel::<Vec<u8>>(32);
        tokio::spawn(Self::forward_stdin(child_stdin, stdin_rx));

        let (tx, rx) = mpsc::channel(32);
        match lease {
            None => ur_rpc::stream::spawn_child_output_stream(child, tx),
            Some(lease) => {
                let (child_tx, child_rx) = mpsc::channel(32);
                ur_rpc::stream::spawn_child_output_stream(child, child_tx);
                tokio::spawn(Self::forward_then_reveal(
                    child_rx,
                    tx,
                    self.overlays.clone(),
                    lease,
                ));
            }
        }

        let stream = ReceiverStream::new(rx);
        Ok((
            Response::new(Box::pin(stream) as CommandOutputStream),
            stdin_tx,
        ))
    }

    /// Conceal the local overlays of the pool slot `resolved_dir` lies in, when `req`
    /// runs a command that drives git. Returns `None` for other commands and for
    /// directories outside an overlaid slot.
    async fn conceal_overlays(
        &self,
        req: &BuilderExecRequest,
        resolved_dir: &str,
    ) -> Result<Option<OverlayLease>, Status> {
        if !OVERLAY_BRACKETED_COMMANDS.contains(&req.command.as_str()) {
            return Ok(None);
        }
        self.overlays
            .conceal_for(Path::new(resolved_dir))
            .await
            .map_err(|e| {
                error!(working_dir = %resolved_dir, error = %e, "failed to conceal local overlays");
                Status::failed_precondition(format!(
                    "failed to hide local overlay files from git: {e}"
                ))
            })
    }

    /// Forward a bracketed child's output to the caller, revealing the slot's overlays
    /// before the final exit-code frame. Keeps draining after the caller disconnects so
    /// the overlays come back only once the child has actually exited.
    async fn forward_then_reveal(
        mut child_rx: mpsc::Receiver<Result<CommandOutput, Status>>,
        tx: mpsc::Sender<Result<CommandOutput, Status>>,
        overlays: LocalOverlayManager,
        lease: OverlayLease,
    ) {
        let mut exit_frame = None;
        while let Some(msg) = child_rx.recv().await {
            if matches!(
                &msg,
                Ok(CommandOutput {
                    payload: Some(Payload::ExitCode(_))
                })
            ) {
                exit_frame = Some(msg);
                break;
            }
            let _ = tx.send(msg).await;
        }

        let final_frame = match overlays.reveal_after(lease).await {
            Ok(()) => exit_frame,
            Err(e) => {
                error!(error = %e, "failed to restore local overlays after git command");
                Some(Err(Status::internal(format!(
                    "command finished but restoring local overlay files failed: {e}"
                ))))
            }
        };
        if let Some(frame) = final_frame {
            let _ = tx.send(frame).await;
        }
    }

    /// Forward stdin frames from an inbound exec stream to a child's stdin.
    ///
    /// Returns when the inbound stream ends, dropping `stdin_tx` and thereby
    /// closing the child's stdin.
    fn spawn_stdin_forwarder(
        in_stream: tonic::Streaming<BuilderExecMessage>,
        stdin_tx: mpsc::Sender<Vec<u8>>,
    ) {
        tokio::spawn(Self::pump_stdin(in_stream, stdin_tx));
    }

    /// Drain stdin frames from `in_stream` into `stdin_tx` until the stream
    /// ends or the receiver goes away.
    async fn pump_stdin(
        mut in_stream: tonic::Streaming<BuilderExecMessage>,
        stdin_tx: mpsc::Sender<Vec<u8>>,
    ) {
        while let Some(Ok(msg)) = in_stream.next().await {
            let Some(ExecPayload::Stdin(data)) = msg.payload else {
                continue;
            };
            if stdin_tx.send(data).await.is_err() {
                break;
            }
        }
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

        // Both paths forward subsequent stdin frames from the bidi stream to
        // the child process.
        let (resp, stdin_tx) = if req.long_lived {
            self.spawn_long_lived(&req)?
        } else {
            self.spawn_command(&req).await?
        };

        Self::spawn_stdin_forwarder(in_stream, stdin_tx);

        Ok(resp)
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
            overlays: LocalOverlayManager {
                workspace: PathBuf::from("/nonexistent-pool-workspace"),
                config_dir: PathBuf::from("/nonexistent-config"),
                active: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            },
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

        let (resp, _stdin_tx) = handler.spawn_command(&req).await.unwrap();
        let (stdout_data, exit_code) = collect_stream(resp.into_inner()).await;

        assert_eq!(String::from_utf8_lossy(&stdout_data).trim(), "hello");
        assert_eq!(exit_code, Some(0));
    }

    #[tokio::test]
    async fn test_exec_git_in_overlaid_slot_hides_overlays() {
        let tmp = tempfile::tempdir().unwrap();
        let mut handler = handler_with_workspace(None);
        handler.overlays = LocalOverlayManager {
            workspace: tmp.path().join("workspace"),
            config_dir: tmp.path().join("config"),
            active: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        };
        let slot = handler.overlays.workspace.join("pool/proj/0");
        std::fs::create_dir_all(&slot).unwrap();
        let status = std::process::Command::new("git")
            .args(["init", "--initial-branch=master"])
            .current_dir(&slot)
            .output()
            .unwrap()
            .status;
        assert!(status.success());
        let local = handler.overlays.config_dir.join("projects/proj/local");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("overlay.txt"), "local").unwrap();
        handler.overlays.apply("proj", &slot).await.unwrap();

        let req = BuilderExecRequest {
            command: "git".into(),
            args: vec!["status".into(), "--porcelain".into()],
            working_dir: slot.display().to_string(),
            env: std::collections::HashMap::new(),
            long_lived: false,
        };
        let (resp, _stdin_tx) = handler.spawn_command(&req).await.unwrap();
        let (stdout_data, exit_code) = collect_stream(resp.into_inner()).await;

        assert_eq!(exit_code, Some(0));
        assert!(
            String::from_utf8_lossy(&stdout_data).trim().is_empty(),
            "overlay visible to git: {}",
            String::from_utf8_lossy(&stdout_data)
        );
        assert_eq!(
            std::fs::read_to_string(slot.join("overlay.txt")).unwrap(),
            "local",
            "overlay restored by the time the exit code arrives"
        );
    }

    #[tokio::test]
    async fn test_exec_short_lived_reads_forwarded_stdin() {
        let handler = handler_with_workspace(None);
        let req = BuilderExecRequest {
            command: "cat".into(),
            args: vec![],
            working_dir: "/tmp".into(),
            env: std::collections::HashMap::new(),
            long_lived: false,
        };

        let (resp, stdin_tx) = handler.spawn_command(&req).await.unwrap();
        stdin_tx.send(b"piped payload".to_vec()).await.unwrap();
        // Dropping the sender closes the child's stdin so `cat` sees EOF.
        drop(stdin_tx);

        let (stdout_data, exit_code) = collect_stream(resp.into_inner()).await;

        assert_eq!(String::from_utf8_lossy(&stdout_data), "piped payload");
        assert_eq!(exit_code, Some(0));
    }

    #[tokio::test]
    async fn test_exec_short_lived_stdin_closes_without_input() {
        let handler = handler_with_workspace(None);
        let req = BuilderExecRequest {
            command: "cat".into(),
            args: vec![],
            working_dir: "/tmp".into(),
            env: std::collections::HashMap::new(),
            long_lived: false,
        };

        // A non-bidi caller sends no stdin frames at all; the child must still
        // see EOF rather than hanging on an open pipe.
        let (resp, stdin_tx) = handler.spawn_command(&req).await.unwrap();
        drop(stdin_tx);

        let (stdout_data, exit_code) = collect_stream(resp.into_inner()).await;

        assert!(stdout_data.is_empty());
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

        let result = handler.spawn_command(&req).await;
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
