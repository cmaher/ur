use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Initialize structured JSON logging to stdout.
///
/// ur-workerd runs inside a container where stdout is captured by the container
/// runtime (Docker/containerd). JSON format enables structured log aggregation
/// and parsing by Docker logging drivers.
pub fn init() {
    tracing_subscriber::registry()
        .with(fmt::layer().json().with_target(true).with_thread_ids(true))
        .init();
}
