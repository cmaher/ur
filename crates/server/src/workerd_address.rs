//! `WorkerdAddressResolver`: the one place the server decides how to reach a
//! worker's daemon (`WorkerDaemonService`).

use workflow_db::WorkerRepo;
use workflow_db::model::{Worker, WorkerRuntime};

use crate::WorkerdClient;
use crate::machine_registry::{MachineRegistryError, MachineRegistryManager};

/// Resolves a worker row to the address (and routing metadata) of its daemon.
///
/// - `container`: the worker container's own `workerd`, via Docker DNS —
///   `http://<worker_prefix><worker_id>:WORKERD_GRPC_PORT` (a container from
///   before ur-ezhdb keeps its `<worker_prefix><process_id>` name).
/// - `local`: builderd hosts every local worker's daemon on its own port, so
///   the client uses the builderd channel of the row's `machine_id` (from the
///   [`MachineRegistryManager`]) and sends `ur-worker-id` on every call.
///
/// A row whose `machine_id` the registry does not know is an error for
/// either runtime (`failed_precondition` naming the id), never treated as
/// local.
///
/// Injected from `main`; never construct one inside a handler.
#[derive(Clone)]
pub struct WorkerdAddressResolver {
    machine_registry: MachineRegistryManager,
    worker_prefix: String,
}

impl WorkerdAddressResolver {
    pub fn new(machine_registry: MachineRegistryManager, worker_prefix: String) -> Self {
        Self {
            machine_registry,
            worker_prefix,
        }
    }

    /// The gRPC address of `worker`'s daemon (for a local worker, its
    /// machine's builderd).
    pub fn address_for(&self, worker: &Worker) -> Result<String, MachineRegistryError> {
        match worker.runtime {
            WorkerRuntime::Container => self.container_address(worker),
            WorkerRuntime::Local => self
                .machine_registry
                .builderd_addr(&worker.machine_id)
                .map(str::to_owned),
        }
    }

    /// A client for `worker`'s daemon with no agent-status tracking.
    pub fn client_for(&self, worker: &Worker) -> Result<WorkerdClient, MachineRegistryError> {
        match worker.runtime {
            WorkerRuntime::Container => Ok(WorkerdClient::new(self.container_address(worker)?)),
            WorkerRuntime::Local => Ok(WorkerdClient::from_channel(
                self.machine_registry.builderd_channel(&worker.machine_id)?,
            )
            .with_routing_worker_id(worker.worker_id.clone())),
        }
    }

    /// A client that marks `worker` Working after each successful dispatch.
    pub fn tracked_client_for(
        &self,
        worker: &Worker,
        worker_repo: WorkerRepo,
    ) -> Result<WorkerdClient, MachineRegistryError> {
        Ok(self
            .client_for(worker)?
            .with_status_tracking(worker_repo, worker.worker_id.clone()))
    }

    /// Docker DNS address of a container worker's daemon. The machine must
    /// be registered: the name only resolves on that machine's network.
    fn container_address(&self, worker: &Worker) -> Result<String, MachineRegistryError> {
        self.machine_registry.builderd_addr(&worker.machine_id)?;
        // Pre-ur-ezhdb containers kept their process-id name.
        let name = match crate::worker::legacy_container_id(&self.worker_prefix, worker) {
            Some(_) => format!("{}{}", self.worker_prefix, worker.process_id),
            None => format!("{}{}", self.worker_prefix, worker.worker_id),
        };
        Ok(format!("http://{name}:{}", ur_config::WORKERD_GRPC_PORT))
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
            container_id: format!("{}ur-abc12-x1y2", ur_config::DEFAULT_WORKER_PREFIX),
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
            machine_id: "test-machine".into(),
        }
    }

    fn resolver() -> WorkerdAddressResolver {
        WorkerdAddressResolver::new(
            MachineRegistryManager::new(
                "test-machine".into(),
                "http://host.docker.internal:12323".into(),
            )
            .unwrap(),
            ur_config::DEFAULT_WORKER_PREFIX.into(),
        )
    }

    /// The container's Docker DNS name derives from the worker id; a
    /// container from before ur-ezhdb (its row recorded a Docker ID) keeps
    /// its process-id name.
    #[tokio::test]
    async fn container_worker_dials_its_container_without_routing_header() {
        let w = worker(WorkerRuntime::Container);
        assert_eq!(
            resolver().address_for(&w).unwrap(),
            format!(
                "http://{}ur-abc12-x1y2:{}",
                ur_config::DEFAULT_WORKER_PREFIX,
                ur_config::WORKERD_GRPC_PORT
            )
        );
        let legacy = Worker {
            container_id: "0123abcd".into(),
            ..worker(WorkerRuntime::Container)
        };
        assert_eq!(
            resolver().address_for(&legacy).unwrap(),
            format!(
                "http://{}ur-abc12:{}",
                ur_config::DEFAULT_WORKER_PREFIX,
                ur_config::WORKERD_GRPC_PORT
            )
        );
        let request = resolver().client_for(&w).unwrap().request(()).unwrap();
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
            resolver().address_for(&w).unwrap(),
            "http://host.docker.internal:12323"
        );
        let request = resolver().client_for(&w).unwrap().request(()).unwrap();
        assert_eq!(
            request.metadata().get(ur_config::WORKER_ID_HEADER).unwrap(),
            "ur-abc12-x1y2"
        );
    }

    #[tokio::test]
    async fn tracked_local_client_also_sends_worker_id_header() {
        let pool = sqlx::PgPool::connect_lazy("postgres://ur:ur@localhost:1/none").unwrap();
        let w = worker(WorkerRuntime::Local);
        let client = resolver()
            .tracked_client_for(&w, WorkerRepo::new(pool))
            .unwrap();
        let request = client.request(()).unwrap();
        assert_eq!(
            request.metadata().get(ur_config::WORKER_ID_HEADER).unwrap(),
            "ur-abc12-x1y2"
        );
    }

    /// A worker (either runtime) on a machine the registry does not know is
    /// an error naming the machine, never treated as local.
    #[tokio::test]
    async fn worker_on_unknown_machine_is_failed_precondition() {
        for runtime in [WorkerRuntime::Local, WorkerRuntime::Container] {
            let w = Worker {
                machine_id: "studio".into(),
                ..worker(runtime)
            };
            assert_names_studio(resolver().address_for(&w).unwrap_err());
            assert_names_studio(resolver().client_for(&w).err().unwrap());
        }
    }

    fn assert_names_studio(err: MachineRegistryError) {
        let status = tonic::Status::from(err);
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert!(status.message().contains("studio"), "{}", status.message());
    }
}
