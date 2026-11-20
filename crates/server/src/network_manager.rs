use anyhow::{Result, bail};

use ur_rpc::proto::builder_container::InspectNetworkRequest;

use crate::MachineRegistryManager;

/// Verifies the Docker network that ur worker containers join.
///
/// Networks are owned by docker-compose (the worker network uses `internal: true`
/// for isolation). This manager only checks existence; it never creates or removes
/// networks.
#[derive(Clone)]
pub struct NetworkManager {
    /// Source of each machine's builderd, which inspects its Docker networks.
    machine_registry: MachineRegistryManager,
    /// Name of the Docker network to verify.
    network_name: String,
}

impl NetworkManager {
    pub fn new(machine_registry: MachineRegistryManager, network_name: String) -> Self {
        Self {
            machine_registry,
            network_name,
        }
    }

    /// Return the network name managed by this instance.
    pub fn network_name(&self) -> &str {
        &self.network_name
    }

    /// Verify the Docker network exists on `machine_id` (created by docker compose).
    ///
    /// Networks are owned by docker-compose — the worker network uses
    /// `internal: true` for isolation, which `docker network create` cannot
    /// express. This method only checks; it never creates.
    pub async fn ensure(&self, machine_id: &str) -> Result<()> {
        let request = InspectNetworkRequest {
            name: self.network_name.clone(),
        };
        let response = self
            .machine_registry
            .container_client(machine_id)?
            .inspect_network(request)
            .await
            .map_err(|s| anyhow::anyhow!("inspect_network gRPC error: {s}"))?;
        if !response.exists {
            bail!(
                "Docker network '{}' does not exist — is docker compose running?",
                self.network_name
            );
        }
        Ok(())
    }
}
