use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use ur_utils::logging::LogWriter;

const LOG_PREFIX: &str = "ur.log";

/// Initialize structured JSON logging to `<logs_dir>/ur.log`.
///
/// Logs rotate at 20 MiB, retaining at most seven files (140 MiB). The CLI writes JSON
/// logs to a file (not stdout) so that user-facing output remains clean
/// while retaining machine-parseable diagnostics.
/// Returns a [`WorkerGuard`] that **must** be held for the lifetime of the
/// program -- dropping it flushes and stops the background writer.
pub fn init(logs_dir: &Path) -> WorkerGuard {
    let file_appender =
        LogWriter::new(logs_dir, LOG_PREFIX).expect("failed to create rolling file appender");
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
