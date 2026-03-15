use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const LOG_FILE: &str = "builderd.log";

/// Initialize structured JSON logging to `<config_dir>/builderd.log`.
///
/// Returns a [`WorkerGuard`] that **must** be held for the lifetime of the
/// program — dropping it flushes and stops the background writer.
pub fn init(config_dir: &Path) -> WorkerGuard {
    let file_appender = tracing_appender::rolling::never(config_dir, LOG_FILE);
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .json()
                .with_target(true)
                .with_thread_ids(true)
                .with_writer(non_blocking),
        )
        .init();

    guard
}
