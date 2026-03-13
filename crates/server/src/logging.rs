use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Initialize structured JSON logging to stdout.
///
/// ur-server runs inside a container where stdout is captured by the container
/// runtime (Docker/containerd). JSON format enables structured log aggregation
/// and parsing by Docker logging drivers.
///
/// Defaults to `info` level; override via `RUST_LOG` env var.
pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().json().with_target(true).with_thread_ids(true))
        .init();
}
