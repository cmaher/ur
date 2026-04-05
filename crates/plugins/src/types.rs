use std::path::PathBuf;

/// Configuration applied to each worker container launched by a plugin.
pub struct WorkerConfig {
    /// Volume mounts as (host_path, container_path) pairs.
    pub volumes: Vec<(PathBuf, PathBuf)>,
    /// Environment variables as (key, value) pairs.
    pub env_vars: Vec<(String, String)>,
}

/// A database migration contributed by a plugin.
pub struct MigrationEntry {
    /// Name of the database this migration targets.
    pub database_name: String,
    /// Raw SQL migration content.
    pub migrations: Vec<String>,
}
