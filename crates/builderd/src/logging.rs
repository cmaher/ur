use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use ur_utils::logging::LogWriter;

const LOG_FILE_PREFIX: &str = "builderd.log";

/// Initialize structured JSON logging to `<logs_dir>/builderd.log` with size-based
/// rotation, retaining up to seven 20 MiB files.
///
/// Returns a [`WorkerGuard`] that **must** be held for the lifetime of the
/// program — dropping it flushes and stops the background writer.
pub fn init(logs_dir: &Path) -> WorkerGuard {
    let file_appender = LogWriter::new(logs_dir, LOG_FILE_PREFIX)
        .expect("failed to initialize rolling file appender");
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
