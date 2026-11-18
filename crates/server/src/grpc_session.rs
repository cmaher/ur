//! Host-facing session-control RPCs (`CaptureWorkerPane`, `ReadWorkerTranscript`,
//! `SendWorkerKeys`, `InterruptWorker`, `WaitWorkerIdle`).
//!
//! [`CoreServiceHandler`]'s trait methods delegate here. Each request names a
//! running worker by its CLI-facing process id, is routed to that worker's
//! daemon exactly like `SendWorkerMessage`, and keeps the daemon's gRPC code
//! (prefixed with the operation and worker) so the CLI can show why it failed.
//! The worker-facing `WorkerCoreServiceHandler` answers all of these with
//! `Unimplemented`.

use tonic::Status;
use tracing::info;
use ur_rpc::proto::core::{
    CaptureWorkerPaneRequest, CaptureWorkerPaneResponse, InterruptWorkerRequest,
    ReadWorkerTranscriptRequest, ReadWorkerTranscriptResponse, SendWorkerKeysRequest,
    WaitWorkerIdleRequest, WaitWorkerIdleResponse, WorkerKey,
};

use crate::grpc::{CoreError, CoreServiceHandler};
use crate::workerd_client::WorkerdClient;

/// Turns returned when the request asks for 0.
pub const DEFAULT_TRANSCRIPT_TURNS: u32 = 10;
/// Wait used when the request asks for 0 seconds.
pub const DEFAULT_WAIT_IDLE_SECS: u32 = 600;
/// Longest accepted wait.
pub const MAX_WAIT_IDLE_SECS: u32 = 3600;
/// Screen lines returned on a timed-out wait when the request asks for 0.
pub const DEFAULT_SCREEN_TAIL_LINES: u32 = 40;
/// Most scrollback lines a capture may ask for.
pub const MAX_CAPTURE_LINES: u32 = 50_000;

/// Rewrap a daemon `Status`, keeping its code.
fn daemon_status(op: &str, worker_id: &str, status: Status) -> Status {
    Status::new(
        status.code(),
        format!("{op} on worker {worker_id}: {}", status.message()),
    )
}

impl CoreServiceHandler {
    /// The daemon client of the running worker whose process id is `worker_id`.
    async fn running_worker_client(&self, worker_id: &str) -> Result<WorkerdClient, Status> {
        let workers = self
            .worker_repo
            .list_workers_by_container_status("running")
            .await
            .map_err(|e| Status::internal(format!("db error: {e}")))?;
        let worker = workers
            .into_iter()
            .find(|w| w.process_id == worker_id)
            .ok_or_else(|| CoreError::WorkerNotFound {
                worker_id: worker_id.to_owned(),
            })?;
        Ok(self
            .workerd_resolver
            .tracked_client_for(&worker, self.worker_repo.clone()))
    }

    pub(crate) async fn capture_worker_pane_impl(
        &self,
        req: CaptureWorkerPaneRequest,
    ) -> Result<CaptureWorkerPaneResponse, Status> {
        info!(
            worker_id = req.worker_id,
            lines = req.lines,
            "capture_worker_pane"
        );
        if req.lines > MAX_CAPTURE_LINES {
            return Err(Status::invalid_argument(format!(
                "lines must be at most {MAX_CAPTURE_LINES}"
            )));
        }
        let client = self.running_worker_client(&req.worker_id).await?;
        let text = client
            .capture_pane(req.lines)
            .await
            .map_err(|s| daemon_status("capture pane", &req.worker_id, s))?;
        Ok(CaptureWorkerPaneResponse { text })
    }

    pub(crate) async fn read_worker_transcript_impl(
        &self,
        req: ReadWorkerTranscriptRequest,
    ) -> Result<ReadWorkerTranscriptResponse, Status> {
        info!(
            worker_id = req.worker_id,
            turns = req.turns,
            "read_worker_transcript"
        );
        let turns = if req.turns == 0 {
            DEFAULT_TRANSCRIPT_TURNS
        } else {
            req.turns
        };
        let client = self.running_worker_client(&req.worker_id).await?;
        let resp = client
            .read_transcript(turns)
            .await
            .map_err(|s| daemon_status("read transcript", &req.worker_id, s))?;
        Ok(ReadWorkerTranscriptResponse {
            turns: resp.turns,
            source_path: resp.source_path,
        })
    }

    pub(crate) async fn send_worker_keys_impl(
        &self,
        req: SendWorkerKeysRequest,
    ) -> Result<(), Status> {
        info!(worker_id = req.worker_id, keys = ?req.keys, "send_worker_keys");
        if req.keys.is_empty() {
            return Err(Status::invalid_argument("no keys given"));
        }
        let keys = req
            .keys
            .iter()
            .map(|k| match WorkerKey::try_from(*k) {
                Ok(WorkerKey::Unspecified) | Err(_) => {
                    Err(Status::invalid_argument(format!("unknown key {k}")))
                }
                Ok(key) => Ok(key),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let client = self.running_worker_client(&req.worker_id).await?;
        client
            .send_keys(&keys)
            .await
            .map_err(|s| daemon_status("send keys", &req.worker_id, s))
    }

    pub(crate) async fn interrupt_worker_impl(
        &self,
        req: InterruptWorkerRequest,
    ) -> Result<(), Status> {
        info!(worker_id = req.worker_id, "interrupt_worker");
        let client = self.running_worker_client(&req.worker_id).await?;
        client
            .interrupt()
            .await
            .map_err(|s| daemon_status("interrupt", &req.worker_id, s))
    }

    pub(crate) async fn wait_worker_idle_impl(
        &self,
        req: WaitWorkerIdleRequest,
    ) -> Result<WaitWorkerIdleResponse, Status> {
        info!(
            worker_id = req.worker_id,
            timeout_secs = req.timeout_secs,
            "wait_worker_idle"
        );
        if req.timeout_secs > MAX_WAIT_IDLE_SECS {
            return Err(Status::invalid_argument(format!(
                "timeout must be at most {MAX_WAIT_IDLE_SECS} seconds"
            )));
        }
        let timeout = match req.timeout_secs {
            0 => DEFAULT_WAIT_IDLE_SECS,
            t => t,
        };
        let lines = match req.screen_lines {
            0 => DEFAULT_SCREEN_TAIL_LINES,
            n => n,
        };
        let client = self.running_worker_client(&req.worker_id).await?;
        let resp = client
            .wait_idle(timeout, lines)
            .await
            .map_err(|s| daemon_status("wait for idle", &req.worker_id, s))?;
        Ok(WaitWorkerIdleResponse {
            timed_out: resp.timed_out,
            screen_tail: resp.screen_tail,
        })
    }
}
