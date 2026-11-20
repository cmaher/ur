//! `MachineRegistryManager`: the one place the server gets a machine's
//! builderd connection from.
//!
//! Every worker and slot row names its machine (`machine_id`). Code that
//! reaches builderd for a row asks the registry by that id, so an unknown id
//! is a visible error rather than being treated as local. For now the
//! registry holds exactly one machine: the server's own (`[machine] id`,
//! `UR_MACHINE_ID` in the server process) with the builderd address from
//! `UR_BUILDERD_ADDR` (default `http://host.docker.internal:<builderd_port>`).

use std::collections::HashMap;
use std::sync::Arc;

use tonic::transport::Channel;
use ur_rpc::proto::builder::BuilderdClient;
use ur_rpc::retry::{RetryChannel, RetryConfig};
use workflow_db::{MachineRowCount, WorkerRepo};

use crate::builder_container_client::BuilderContainerClient;
use crate::builder_local_worker_client::BuilderLocalWorkerClient;
use crate::builder_pool_client::BuilderPoolClient;

/// Why the registry could not produce a machine's builderd connection.
#[derive(Debug, thiserror::Error)]
pub enum MachineRegistryError {
    #[error("unknown machine '{machine_id}': no builderd is registered for it")]
    UnknownMachine { machine_id: String },
    #[error("invalid builderd address for machine '{machine_id}': {reason}")]
    Channel { machine_id: String, reason: String },
}

impl From<MachineRegistryError> for tonic::Status {
    fn from(e: MachineRegistryError) -> Self {
        match e {
            MachineRegistryError::UnknownMachine { .. } => {
                tonic::Status::failed_precondition(e.to_string())
            }
            MachineRegistryError::Channel { .. } => tonic::Status::internal(e.to_string()),
        }
    }
}

/// Startup refusal: worker/slot rows name machines the registry does not
/// hold (typically the machine id changed).
#[derive(Debug, thiserror::Error)]
pub enum UnregisteredRowsError {
    #[error("db error checking worker/slot machine ids: {source}")]
    Db {
        #[from]
        source: sqlx::Error,
    },
    #[error("{}", unregistered_message(local_id, foreign))]
    Unregistered {
        local_id: String,
        foreign: Vec<MachineRowCount>,
    },
}

fn unregistered_message(local_id: &str, foreign: &[MachineRowCount]) -> String {
    let listed: Vec<String> = foreign
        .iter()
        .map(|r| {
            format!(
                "'{}' ({} slot(s), {} active worker(s))",
                r.machine_id, r.slots, r.active_workers
            )
        })
        .collect();
    let old_id = match foreign {
        [only] => only.machine_id.as_str(),
        _ => "<old id>",
    };
    format!(
        "worker/slot rows name machine(s) this server does not know: {}; this machine is '{local_id}'. \
         set [machine] id = \"{old_id}\" in ur.toml to keep using these rows \
         (renaming a machine is not supported yet)",
        listed.join(", ")
    )
}

/// A registered machine's builderd connection.
struct MachineEntry {
    builderd_addr: String,
    builderd: RetryChannel,
}

/// Machine id → builderd connection. Channels are lazy (nothing connects
/// until the first RPC) and shared by every client built from them.
#[derive(Clone)]
pub struct MachineRegistryManager {
    local_id: String,
    machines: Arc<HashMap<String, MachineEntry>>,
}

impl MachineRegistryManager {
    /// A registry holding only the local machine `local_id`, whose builderd
    /// listens at `local_builderd_addr`.
    pub fn new(
        local_id: String,
        local_builderd_addr: String,
    ) -> Result<Self, MachineRegistryError> {
        let builderd =
            RetryChannel::new(&local_builderd_addr, RetryConfig::default()).map_err(|e| {
                MachineRegistryError::Channel {
                    machine_id: local_id.clone(),
                    reason: format!("{local_builderd_addr}: {e}"),
                }
            })?;
        let machines = HashMap::from([(
            local_id.clone(),
            MachineEntry {
                builderd_addr: local_builderd_addr,
                builderd,
            },
        )]);
        Ok(Self {
            local_id,
            machines: Arc::new(machines),
        })
    }

    /// The id of the machine this server runs on.
    pub fn local_id(&self) -> &str {
        &self.local_id
    }

    /// The builderd channel of `machine_id`.
    pub fn builderd_channel(&self, machine_id: &str) -> Result<Channel, MachineRegistryError> {
        Ok(self.entry(machine_id)?.builderd.channel().clone())
    }

    /// The builderd address of `machine_id` (for logs).
    pub fn builderd_addr(&self, machine_id: &str) -> Result<&str, MachineRegistryError> {
        Ok(&self.entry(machine_id)?.builderd_addr)
    }

    /// Every registered machine id, sorted (the local one included).
    pub fn machine_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.machines.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// `machine_id`'s builderd host-exec client (git, gh, hooks).
    pub fn builderd_client(
        &self,
        machine_id: &str,
    ) -> Result<BuilderdClient, MachineRegistryError> {
        Ok(BuilderdClient::new(self.builderd_channel(machine_id)?))
    }

    /// The local machine's builderd host-exec client, for work tied to no
    /// worker or slot (GitHub calls, host-CLI hostexec).
    pub fn local_builderd_client(&self) -> BuilderdClient {
        BuilderdClient::new(self.local_builderd_channel())
    }

    /// `machine_id`'s builderd container client.
    pub fn container_client(
        &self,
        machine_id: &str,
    ) -> Result<BuilderContainerClient, MachineRegistryError> {
        Ok(BuilderContainerClient::new(
            self.builderd_channel(machine_id)?,
        ))
    }

    /// `machine_id`'s builderd local-worker client.
    pub fn local_worker_client(
        &self,
        machine_id: &str,
    ) -> Result<BuilderLocalWorkerClient, MachineRegistryError> {
        Ok(BuilderLocalWorkerClient::new(
            self.builderd_channel(machine_id)?,
        ))
    }

    /// `machine_id`'s builderd pool client.
    pub fn pool_client(&self, machine_id: &str) -> Result<BuilderPoolClient, MachineRegistryError> {
        Ok(BuilderPoolClient::new(self.builderd_channel(machine_id)?))
    }

    /// The local machine's builderd channel.
    fn local_builderd_channel(&self) -> Channel {
        self.machines[&self.local_id].builderd.channel().clone()
    }

    /// Fail closed when any slot row or non-`stopped` worker row names a
    /// machine the registry does not hold: such rows would otherwise be
    /// orphaned (or misrouted) silently after a machine id change. Stopped
    /// workers are history and never block. Runs at startup, right after
    /// the machine_id backfill.
    pub async fn ensure_rows_registered(
        &self,
        worker_repo: &WorkerRepo,
    ) -> Result<(), UnregisteredRowsError> {
        let foreign: Vec<MachineRowCount> = worker_repo
            .machine_row_counts()
            .await?
            .into_iter()
            .filter(|r| !self.machines.contains_key(&r.machine_id))
            .collect();
        if foreign.is_empty() {
            return Ok(());
        }
        Err(UnregisteredRowsError::Unregistered {
            local_id: self.local_id.clone(),
            foreign,
        })
    }

    fn entry(&self, machine_id: &str) -> Result<&MachineEntry, MachineRegistryError> {
        self.machines
            .get(machine_id)
            .ok_or_else(|| MachineRegistryError::UnknownMachine {
                machine_id: machine_id.to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> MachineRegistryManager {
        MachineRegistryManager::new("mbp".into(), "http://127.0.0.1:12323".into()).unwrap()
    }

    #[tokio::test]
    async fn holds_only_the_local_machine() {
        let r = registry();
        assert_eq!(r.local_id(), "mbp");
        assert!(r.builderd_channel("mbp").is_ok());
        assert_eq!(r.builderd_addr("mbp").unwrap(), "http://127.0.0.1:12323");
        assert_eq!(r.machine_ids(), vec!["mbp".to_owned()]);
        let _ = r.local_builderd_client();
        assert!(r.container_client("mbp").is_ok());
        assert!(r.local_worker_client("mbp").is_ok());
        assert!(r.pool_client("mbp").is_ok());
        assert!(r.builderd_client("mbp").is_ok());
    }

    #[tokio::test]
    async fn unknown_machine_is_failed_precondition_naming_it() {
        let err = registry().builderd_channel("studio").unwrap_err();
        assert!(
            matches!(&err, MachineRegistryError::UnknownMachine { machine_id } if machine_id == "studio")
        );
        let status = tonic::Status::from(err);
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert!(status.message().contains("studio"), "{}", status.message());

        let status = tonic::Status::from(registry().builderd_addr("").unwrap_err());
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);

        // Every typed client fails closed the same way.
        let r = registry();
        for err in [
            r.builderd_client("studio").err(),
            r.container_client("studio").err(),
            r.local_worker_client("studio").err(),
            r.pool_client("studio").err(),
        ] {
            let status = tonic::Status::from(err.expect("unknown machine must error"));
            assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        }
    }

    #[test]
    fn invalid_address_is_a_channel_error() {
        let Err(err) = MachineRegistryManager::new("mbp".into(), "not a uri".into()) else {
            panic!("an unparseable builderd address must be rejected");
        };
        assert!(
            matches!(&err, MachineRegistryError::Channel { machine_id, .. } if machine_id == "mbp")
        );
        assert_eq!(tonic::Status::from(err).code(), tonic::Code::Internal);
    }

    fn row_worker(worker_id: &str, machine_id: &str, status: &str) -> workflow_db::Worker {
        let now = chrono::Utc::now().to_rfc3339();
        workflow_db::Worker {
            worker_id: worker_id.into(),
            process_id: worker_id.into(),
            project_key: "proj".into(),
            container_id: String::new(),
            worker_secret: "secret".into(),
            strategy: "code".into(),
            agent_type: "claude".into(),
            container_status: status.into(),
            agent_status: "idle".into(),
            workspace_path: None,
            created_at: now.clone(),
            updated_at: now,
            idle_redispatch_count: 0,
            brain_path: None,
            runtime: workflow_db::model::WorkerRuntime::Container,
            machine_id: machine_id.into(),
        }
    }

    fn registry_b() -> MachineRegistryManager {
        MachineRegistryManager::new("b".into(), "http://127.0.0.1:1".into()).unwrap()
    }

    /// Rows of a machine the registry does not hold (here: the machine id
    /// changed from "a" to "b") stop startup with an error naming the old
    /// id, its counts, the local id and the remedy.
    #[tokio::test]
    async fn rows_of_an_unregistered_machine_block_startup() {
        let db = ur_db_test::TestDb::new().await;
        let repo = WorkerRepo::new(db.workflow_pool().clone());
        let now = chrono::Utc::now().to_rfc3339();
        repo.insert_slot(&workflow_db::Slot {
            id: "s0".into(),
            project_key: "proj".into(),
            slot_name: "0".into(),
            host_path: "/pool/proj/0".into(),
            created_at: now.clone(),
            updated_at: now,
            machine_id: "a".into(),
        })
        .await
        .unwrap();
        repo.insert_worker(&row_worker("w-run", "a", "running"))
            .await
            .unwrap();
        repo.insert_worker(&row_worker("w-local", "b", "running"))
            .await
            .unwrap();

        let err = registry_b()
            .ensure_rows_registered(&repo)
            .await
            .unwrap_err();
        let UnregisteredRowsError::Unregistered { local_id, foreign } = &err else {
            panic!("expected Unregistered, got {err:?}");
        };
        assert_eq!(local_id, "b");
        assert_eq!(
            foreign,
            &[MachineRowCount {
                machine_id: "a".into(),
                slots: 1,
                active_workers: 1,
            }]
        );
        let msg = err.to_string();
        assert!(msg.contains("'a' (1 slot(s), 1 active worker(s))"), "{msg}");
        assert!(msg.contains("this machine is 'b'"), "{msg}");
        assert!(msg.contains("set [machine] id = \"a\" in ur.toml"), "{msg}");
    }

    /// Stopped workers of another machine are history: they do not block.
    #[tokio::test]
    async fn stopped_workers_of_another_machine_do_not_block() {
        let db = ur_db_test::TestDb::new().await;
        let repo = WorkerRepo::new(db.workflow_pool().clone());
        repo.insert_worker(&row_worker("w-old", "a", "stopped"))
            .await
            .unwrap();
        repo.insert_worker(&row_worker("w-local", "b", "running"))
            .await
            .unwrap();
        registry_b().ensure_rows_registered(&repo).await.unwrap();
    }
}
