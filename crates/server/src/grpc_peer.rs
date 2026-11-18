//! Worker-port access to other workers: the one resolver every worker-callable
//! session-control verb and the worker port's `WorkerStop`/`WorkerLaunch` go
//! through (policy: `docs/plans/ur-o15mo-session-control.md`).
//!
//! [`WorkerCoreServiceHandler::peer_access`] loads the caller's row by the
//! `ur-worker-id` header (the auth interceptor already checked its secret) and
//! refuses callers that may not drive other workers; [`PeerAccess`] then
//! resolves targets in the caller's project only, so another project's worker
//! is indistinguishable from a missing one. Every resolved call is logged with
//! caller, target and verb. This is a guardrail for well-behaved agents, not a
//! security boundary.

use tonic::Status;
use tonic::metadata::MetadataMap;
use tracing::info;
use ur_rpc::proto::core::{WorkerLaunchRequest, WorkerSummary};
use workflow_db::model::Worker;

use crate::WorkerStrategy;
use crate::grpc::{CoreError, WorkerCoreServiceHandler, worker_summaries};
use crate::grpc_session::SessionTargets;
use crate::workerd_client::WorkerdClient;

/// `PermissionDenied` message for callers that may not drive other workers.
pub const DRIVER_ONLY_MESSAGE: &str = "only manual workers may drive other workers";

/// What a worker asks to do to another worker (audit log and policy key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PeerVerb {
    List,
    ReadScreen,
    ReadTranscript,
    Send,
    Keys,
    Interrupt,
    WaitIdle,
    Stop,
    Launch,
}

impl PeerVerb {
    fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::ReadScreen => "read_screen",
            Self::ReadTranscript => "read_transcript",
            Self::Send => "send",
            Self::Keys => "keys",
            Self::Interrupt => "interrupt",
            Self::WaitIdle => "wait_idle",
            Self::Stop => "stop",
            Self::Launch => "launch",
        }
    }
}

/// Whether a caller whose stored strategy is `strategy` may use `verb` on a
/// worker other than itself.
fn may_drive(strategy: Option<WorkerStrategy>, verb: PeerVerb) -> bool {
    match strategy {
        Some(WorkerStrategy::Manual) => true,
        // Design workers' `ur-workertools workflow dispatch` reaches the
        // server as workerd's WorkerLaunch; the default design mode depends on it.
        Some(WorkerStrategy::Design) => verb == PeerVerb::Launch,
        _ => false,
    }
}

/// A caller cleared to use one verb; resolves its targets.
pub(crate) struct PeerAccess<'a> {
    handler: &'a WorkerCoreServiceHandler,
    caller: Worker,
    verb: PeerVerb,
}

impl WorkerCoreServiceHandler {
    /// Load the caller and check it may use `verb`. `target` is the worker id
    /// the request names, if any: a worker may always stop itself.
    pub(crate) async fn peer_access(
        &self,
        metadata: &MetadataMap,
        verb: PeerVerb,
        target: Option<&str>,
    ) -> Result<PeerAccess<'_>, Status> {
        let caller_id = metadata
            .get(ur_config::WORKER_ID_HEADER)
            .ok_or_else(|| Status::unauthenticated("missing ur-worker-id header"))?
            .to_str()
            .map_err(|_| Status::unauthenticated("invalid ur-worker-id header encoding"))?;
        let caller = self
            .worker_repo
            .get_worker(caller_id)
            .await
            .map_err(|e| Status::internal(format!("db error: {e}")))?
            .ok_or_else(|| Status::unauthenticated(format!("unknown worker {caller_id}")))?;
        let access = PeerAccess {
            handler: self,
            caller,
            verb,
        };
        let self_stop = verb == PeerVerb::Stop && target.is_some_and(|t| access.is_caller(t));
        let strategy = WorkerStrategy::from_name(&access.caller.strategy).ok();
        if !self_stop && !may_drive(strategy, verb) {
            info!(
                caller = access.caller.worker_id,
                target = target.unwrap_or_default(),
                verb = verb.as_str(),
                strategy = access.caller.strategy,
                "worker peer access denied"
            );
            return Err(Status::permission_denied(DRIVER_ONLY_MESSAGE));
        }
        Ok(access)
    }
}

impl PeerAccess<'_> {
    fn is_caller(&self, id: &str) -> bool {
        id == self.caller.worker_id || id == self.caller.process_id
    }

    /// A worker the caller can see: same non-empty project.
    fn visible(&self, project_key: &str) -> bool {
        !self.caller.project_key.is_empty() && project_key == self.caller.project_key
    }

    fn audit(&self, target: &str) {
        info!(
            caller = self.caller.worker_id,
            target,
            verb = self.verb.as_str(),
            "worker peer access"
        );
    }

    /// The running same-project worker `id` names (process id or full id).
    pub(crate) async fn running_target(&self, id: &str) -> Result<Worker, Status> {
        let workers = self
            .handler
            .worker_repo
            .list_workers_by_container_status("running")
            .await
            .map_err(|e| Status::internal(format!("db error: {e}")))?;
        let target = workers
            .into_iter()
            .filter(|w| self.visible(&w.project_key))
            .find(|w| w.worker_id == id || w.process_id == id)
            .ok_or_else(|| CoreError::WorkerNotFound {
                worker_id: id.to_owned(),
            })?;
        self.audit(&target.worker_id);
        Ok(target)
    }

    /// Running workers in the caller's project, for `WorkerList`.
    pub(crate) async fn peers(&self) -> Vec<WorkerSummary> {
        self.audit("*");
        let summaries = self
            .handler
            .launch_manager
            .worker_manager
            .list()
            .await
            .into_iter()
            .filter(|s| self.visible(&s.project_key))
            .collect();
        worker_summaries(
            summaries,
            &self.handler.ticket_repo,
            &self.handler.workflow_repo,
        )
        .await
    }

    /// The id to hand `LaunchManager::stop`: the caller itself, or an active
    /// (provisioning/running/stopping) same-project worker.
    pub(crate) async fn stop_target(&self, id: &str) -> Result<String, Status> {
        if self.is_caller(id) {
            self.audit(id);
            return Ok(id.to_owned());
        }
        let not_found = || CoreError::WorkerNotFound {
            worker_id: id.to_owned(),
        };
        let target = self
            .handler
            .launch_manager
            .worker_manager
            .find_active_worker(id)
            .await
            .map_err(|_| not_found())?;
        if !self.visible(&target.project_key) {
            return Err(not_found().into());
        }
        self.audit(&target.worker_id);
        Ok(target.worker_id)
    }

    /// A launch must land in the caller's project.
    pub(crate) fn authorize_launch(&self, req: &WorkerLaunchRequest) -> Result<(), Status> {
        let project_key = self
            .handler
            .launch_manager
            .resolve_effective_project_key(req);
        if !self.visible(&project_key) {
            let shown = if project_key.is_empty() {
                "(none)"
            } else {
                &project_key
            };
            return Err(Status::not_found(format!(
                "launch {}: project not found: {shown}",
                req.worker_id
            )));
        }
        self.audit(&req.worker_id);
        Ok(())
    }
}

#[tonic::async_trait]
impl SessionTargets for PeerAccess<'_> {
    async fn target_client(&self, worker_id: &str) -> Result<WorkerdClient, Status> {
        let target = self.running_target(worker_id).await?;
        Ok(self
            .handler
            .launch_manager
            .workerd_resolver
            .tracked_client_for(&target, self.handler.worker_repo.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_manual_drives_and_design_keeps_dispatch_launch() {
        use PeerVerb::*;
        let all = [
            List,
            ReadScreen,
            ReadTranscript,
            Send,
            Keys,
            Interrupt,
            WaitIdle,
            Stop,
            Launch,
        ];
        for verb in all {
            assert!(may_drive(Some(WorkerStrategy::Manual), verb), "{verb:?}");
            assert!(!may_drive(Some(WorkerStrategy::Code), verb), "{verb:?}");
            assert!(!may_drive(None, verb), "{verb:?}");
            assert_eq!(
                may_drive(Some(WorkerStrategy::Design), verb),
                verb == Launch,
                "{verb:?}"
            );
        }
    }
}
