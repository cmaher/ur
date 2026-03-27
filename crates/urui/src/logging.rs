use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const LOG_PREFIX: &str = "urui.log";
const MAX_LOG_FILES: usize = 7;

/// Initialize structured JSON logging to `<logs_dir>/urui.log`.
///
/// Logs are rotated daily, keeping at most 7 files. The TUI writes JSON
/// logs to a file (not stdout) so that terminal rendering remains clean
/// while retaining machine-parseable diagnostics.
/// Returns a [`WorkerGuard`] that **must** be held for the lifetime of the
/// program -- dropping it flushes and stops the background writer.
pub fn init(logs_dir: &Path) -> WorkerGuard {
    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_PREFIX)
        .max_log_files(MAX_LOG_FILES)
        .build(logs_dir)
        .expect("failed to create rolling file appender");
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
