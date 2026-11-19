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
use ur_rpc::retry::{RetryChannel, RetryConfig};

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

    /// The local machine's builderd channel.
    pub fn local_builderd_channel(&self) -> Channel {
        self.machines[&self.local_id].builderd.channel().clone()
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
        let _ = r.local_builderd_channel();
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
}
