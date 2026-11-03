//! `WorkerdAddressResolver`: the one place the server decides how to reach a
//! worker's daemon (`WorkerDaemonService`).

use workflow_db::WorkerRepo;
use workflow_db::model::{Worker, WorkerRuntime};

use crate::WorkerdClient;

/// Resolves a worker row to the address (and routing metadata) of its daemon.
///
/// - `container`: the worker container's own `workerd`, via Docker DNS —
///   `http://<worker_prefix><process_id>:WORKERD_GRPC_PORT`.
/// - `local`: builderd hosts every local worker's daemon on its own port, so
///   the client dials `builderd_addr` and sends `ur-worker-id` on every call.
///
/// Injected from `main`; never construct one inside a handler.
#[derive(Clone)]
pub struct WorkerdAddressResolver {
    builderd_addr: String,
    worker_prefix: String,
}

impl WorkerdAddressResolver {
    pub fn new(builderd_addr: String, worker_prefix: String) -> Self {
        Self {
            builderd_addr,
            worker_prefix,
        }
    }

    /// The gRPC address of `worker`'s daemon.
    pub fn address_for(&self, worker: &Worker) -> String {
        match worker.runtime {
            WorkerRuntime::Container => format!(
                "http://{}{}:{}",
                self.worker_prefix,
                worker.process_id,
                ur_config::WORKERD_GRPC_PORT
            ),
            WorkerRuntime::Local => self.builderd_addr.clone(),
        }
    }

    /// A client for `worker`'s daemon with no agent-status tracking.
    pub fn client_for(&self, worker: &Worker) -> WorkerdClient {
        self.with_routing(worker, WorkerdClient::new(self.address_for(worker)))
    }

    /// A client that marks `worker` Working after each successful dispatch.
    pub fn tracked_client_for(&self, worker: &Worker, worker_repo: WorkerRepo) -> WorkerdClient {
        let client = WorkerdClient::with_status_tracking(
            self.address_for(worker),
            worker_repo,
            worker.worker_id.clone(),
        );
        self.with_routing(worker, client)
    }

    fn with_routing(&self, worker: &Worker, client: WorkerdClient) -> WorkerdClient {
        match worker.runtime {
            WorkerRuntime::Container => client,
            WorkerRuntime::Local => client.with_routing_worker_id(worker.worker_id.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(runtime: WorkerRuntime) -> Worker {
        Worker {
            worker_id: "ur-abc12-x1y2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            container_id: "cid".into(),
            worker_secret: "secret".into(),
            strategy: "code".into(),
            agent_type: "claude".into(),
            container_status: "running".into(),
            agent_status: "idle".into(),
            workspace_path: None,
            created_at: String::new(),
            updated_at: String::new(),
            idle_redispatch_count: 0,
            brain_path: None,
            runtime,
        }
    }

    fn resolver() -> WorkerdAddressResolver {
        WorkerdAddressResolver::new(
            "http://host.docker.internal:12323".into(),
            ur_config::DEFAULT_WORKER_PREFIX.into(),
        )
    }

    #[tokio::test]
    async fn container_worker_dials_its_container_without_routing_header() {
        let w = worker(WorkerRuntime::Container);
        assert_eq!(
            resolver().address_for(&w),
            format!(
                "http://{}ur-abc12:{}",
                ur_config::DEFAULT_WORKER_PREFIX,
                ur_config::WORKERD_GRPC_PORT
            )
        );
        let request = resolver().client_for(&w).request(()).unwrap();
        assert!(
            request
                .metadata()
                .get(ur_config::WORKER_ID_HEADER)
                .is_none()
        );
    }

    #[tokio::test]
    async fn local_worker_dials_builderd_with_worker_id_header() {
        let w = worker(WorkerRuntime::Local);
        assert_eq!(
            resolver().address_for(&w),
            "http://host.docker.internal:12323"
        );
        let request = resolver().client_for(&w).request(()).unwrap();
        assert_eq!(
            request.metadata().get(ur_config::WORKER_ID_HEADER).unwrap(),
            "ur-abc12-x1y2"
        );
    }

    #[tokio::test]
    async fn tracked_local_client_also_sends_worker_id_header() {
        let pool = sqlx::PgPool::connect_lazy("postgres://ur:ur@localhost:1/none").unwrap();
        let w = worker(WorkerRuntime::Local);
        let client = resolver().tracked_client_for(&w, WorkerRepo::new(pool));
        let request = client.request(()).unwrap();
        assert_eq!(
            request.metadata().get(ur_config::WORKER_ID_HEADER).unwrap(),
            "ur-abc12-x1y2"
        );
    }
}
