use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use ur_utils::logging::LogWriter;

const LOG_FILE_PREFIX: &str = "workerd.log";
const LOGS_DIR_ENV: &str = "UR_LOGS_DIR";

/// Initialize structured JSON logging.
///
/// If `UR_LOGS_DIR` is set, writes to `<UR_LOGS_DIR>/workerd.log` with size-based
/// rotation (seven 20 MiB files) and returns a [`WorkerGuard`] that must be held for
/// the program lifetime.
///
/// Falls back to stdout logging when the env var is absent (backward compat
/// for containers launched without a logs mount).
pub fn init() -> Option<WorkerGuard> {
    match std::env::var(LOGS_DIR_ENV).ok().map(PathBuf::from) {
        Some(logs_dir) => {
            let file_appender = LogWriter::new(&logs_dir, LOG_FILE_PREFIX)
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

            Some(guard)
        }
        None => {
            tracing_subscriber::registry()
                .with(fmt::layer().json().with_target(true).with_thread_ids(true))
                .init();
            None
        }
    }
}
