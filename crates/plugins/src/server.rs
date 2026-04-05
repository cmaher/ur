use anyhow::Result;

use crate::types::{MigrationEntry, WorkerConfig};

/// Plugin interface for server-side extensions.
///
/// Implement this trait to modify worker configuration, contribute database
/// migrations, or register additional gRPC services.
pub trait ServerPlugin: Send + Sync {
    /// Unique name identifying this plugin.
    fn name(&self) -> &str;

    /// Called once at startup with the plugin's TOML configuration table.
    /// Default: no-op.
    fn configure(&mut self, _config: &toml::Table) -> Result<()> {
        Ok(())
    }

    /// Modify the worker container configuration (volumes, env vars).
    /// Default: no-op.
    fn modify_worker(&self, _worker_config: &mut WorkerConfig) -> Result<()> {
        Ok(())
    }

    /// Return database migrations contributed by this plugin.
    /// Default: empty.
    fn migrations(&self) -> Vec<MigrationEntry> {
        Vec::new()
    }

    /// Register additional gRPC services on the server router.
    /// Default: pass-through.
    fn register_grpc(
        &self,
        router: tonic::transport::server::Router,
    ) -> tonic::transport::server::Router {
        router
    }
}

/// Registry holding server plugins and providing batch-apply methods.
pub struct ServerRegistry {
    plugins: Vec<Box<dyn ServerPlugin>>,
}

impl ServerRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Register a plugin.
    pub fn register(&mut self, plugin: Box<dyn ServerPlugin>) {
        self.plugins.push(plugin);
    }

    /// Apply `modify_worker` from every registered plugin, in order.
    pub fn apply_worker_config(&self, worker_config: &mut WorkerConfig) -> Result<()> {
        for plugin in &self.plugins {
            plugin.modify_worker(worker_config)?;
        }
        Ok(())
    }

    /// Collect all migrations from registered plugins.
    pub fn collect_migrations(&self) -> Vec<MigrationEntry> {
        self.plugins.iter().flat_map(|p| p.migrations()).collect()
    }

    /// Apply `register_grpc` from every registered plugin, in order.
    pub fn apply_grpc(
        &self,
        mut router: tonic::transport::server::Router,
    ) -> tonic::transport::server::Router {
        for plugin in &self.plugins {
            router = plugin.register_grpc(router);
        }
        router
    }

    /// Iterate over registered plugins.
    pub fn plugins(&self) -> &[Box<dyn ServerPlugin>] {
        &self.plugins
    }
}

impl Default for ServerRegistry {
    fn default() -> Self {
        Self::new()
    }
}
