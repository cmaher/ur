use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tonic::Status;
use tracing::info;
use ur_rpc::proto::core::CommandOutput;

/// Key for deduplicating long-lived processes: (command, working_dir).
pub type ProcessKey = (String, String);

/// Replaceable output sink for streaming child stdout/stderr to the current caller.
///
/// When a caller disconnects, the sender is cleared (`None`). When a new caller
/// reconnects, a fresh sender is swapped in. The background forwarder checks
/// this on each message — if `None` or closed, output is silently dropped.
pub type OutputSink = Arc<Mutex<Option<mpsc::Sender<Result<CommandOutput, Status>>>>>;

/// A registered long-lived process with an alive flag, stdin sender, and output sink.
pub struct RegisteredProcess {
    /// Shared flag set to `false` by the output-streaming task when the child exits.
    pub alive: Arc<AtomicBool>,
    /// Channel for forwarding stdin data to the child process.
    pub stdin_tx: mpsc::Sender<Vec<u8>>,
    /// Replaceable output sink — the current caller's output channel.
    pub output_sink: OutputSink,
}

/// Thread-safe registry of long-lived processes, keyed by (command, working_dir).
#[derive(Clone)]
pub struct ProcessRegistry {
    inner: Arc<Mutex<HashMap<ProcessKey, RegisteredProcess>>>,
}

impl ProcessRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check whether a process with the given key is already registered and still alive.
    /// Removes dead entries on access.
    pub fn is_running(&self, key: &ProcessKey) -> bool {
        let mut map = self.inner.lock().expect("registry lock poisoned");
        if let Some(entry) = map.get(key) {
            if entry.alive.load(Ordering::Relaxed) {
                return true;
            }
            info!(command = %key.0, working_dir = %key.1, "registered process exited, removing");
            map.remove(key);
        }
        false
    }

    /// Register a new long-lived process. Overwrites any existing entry for the key.
    pub fn register(&self, key: ProcessKey, process: RegisteredProcess) {
        let mut map = self.inner.lock().expect("registry lock poisoned");
        info!(command = %key.0, working_dir = %key.1, "registering long-lived process");
        map.insert(key, process);
    }

    /// Get a clone of the stdin sender for a registered process, if it exists and is still alive.
    pub fn get_stdin_tx(&self, key: &ProcessKey) -> Option<mpsc::Sender<Vec<u8>>> {
        let mut map = self.inner.lock().expect("registry lock poisoned");
        if let Some(entry) = map.get(key) {
            if entry.alive.load(Ordering::Relaxed) {
                return Some(entry.stdin_tx.clone());
            }
            map.remove(key);
        }
        None
    }

    /// Replace the output sink for a registered process, returning the old one (if any).
    /// This wires a new caller's output channel to the existing child process.
    pub fn replace_output_sink(
        &self,
        key: &ProcessKey,
        new_tx: mpsc::Sender<Result<CommandOutput, Status>>,
    ) -> Option<OutputSink> {
        let map = self.inner.lock().expect("registry lock poisoned");
        if let Some(entry) = map.get(key)
            && entry.alive.load(Ordering::Relaxed)
        {
            let mut sink = entry.output_sink.lock().expect("output sink lock poisoned");
            *sink = Some(new_tx);
            return Some(entry.output_sink.clone());
        }
        None
    }

    /// Reap exited processes from the registry. Returns the number of entries removed.
    pub fn reap(&self) -> usize {
        let mut map = self.inner.lock().expect("registry lock poisoned");
        let before = map.len();
        map.retain(|key, entry| {
            if entry.alive.load(Ordering::Relaxed) {
                true
            } else {
                info!(command = %key.0, working_dir = %key.1, "reaping exited process");
                false
            }
        });
        before - map.len()
    }

    /// Spawn a background task that periodically reaps exited processes.
    pub fn spawn_reap_task(&self) {
        let registry = self.clone();
        tokio::spawn(Self::reap_loop(registry));
    }

    async fn reap_loop(registry: Self) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            interval.tick().await;
            let removed = registry.reap();
            if removed > 0 {
                info!(removed, "reap task cleaned up exited processes");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(cmd: &str, dir: &str) -> ProcessKey {
        (cmd.to_string(), dir.to_string())
    }

    fn make_output_sink() -> OutputSink {
        Arc::new(Mutex::new(None))
    }

    #[test]
    fn test_register_and_check_running() {
        let registry = ProcessRegistry::new();
        let key = make_key("sleep", "/tmp");
        let alive = Arc::new(AtomicBool::new(true));
        let (tx, _rx) = mpsc::channel(1);

        registry.register(
            key.clone(),
            RegisteredProcess {
                alive: alive.clone(),
                stdin_tx: tx,
                output_sink: make_output_sink(),
            },
        );

        assert!(registry.is_running(&key));
    }

    #[test]
    fn test_exited_process_is_removed() {
        let registry = ProcessRegistry::new();
        let key = make_key("true", "/tmp");
        let alive = Arc::new(AtomicBool::new(true));
        let (tx, _rx) = mpsc::channel(1);

        registry.register(
            key.clone(),
            RegisteredProcess {
                alive: alive.clone(),
                stdin_tx: tx,
                output_sink: make_output_sink(),
            },
        );

        // Simulate process exit
        alive.store(false, Ordering::Relaxed);

        assert!(!registry.is_running(&key));
    }

    #[test]
    fn test_reap_removes_exited() {
        let registry = ProcessRegistry::new();
        let key = make_key("true", "/tmp");
        let alive = Arc::new(AtomicBool::new(true));
        let (tx, _rx) = mpsc::channel(1);

        registry.register(
            key.clone(),
            RegisteredProcess {
                alive: alive.clone(),
                stdin_tx: tx,
                output_sink: make_output_sink(),
            },
        );

        alive.store(false, Ordering::Relaxed);

        let removed = registry.reap();
        assert_eq!(removed, 1);
        assert_eq!(registry.reap(), 0);
    }

    #[test]
    fn test_not_registered_is_not_running() {
        let registry = ProcessRegistry::new();
        let key = make_key("nonexistent", "/tmp");
        assert!(!registry.is_running(&key));
    }

    #[test]
    fn test_get_stdin_tx_alive() {
        let registry = ProcessRegistry::new();
        let key = make_key("cmd", "/tmp");
        let alive = Arc::new(AtomicBool::new(true));
        let (tx, _rx) = mpsc::channel(1);

        registry.register(
            key.clone(),
            RegisteredProcess {
                alive: alive.clone(),
                stdin_tx: tx,
                output_sink: make_output_sink(),
            },
        );

        assert!(registry.get_stdin_tx(&key).is_some());
    }

    #[test]
    fn test_get_stdin_tx_dead() {
        let registry = ProcessRegistry::new();
        let key = make_key("cmd", "/tmp");
        let alive = Arc::new(AtomicBool::new(false));
        let (tx, _rx) = mpsc::channel(1);

        registry.register(
            key.clone(),
            RegisteredProcess {
                alive: alive.clone(),
                stdin_tx: tx,
                output_sink: make_output_sink(),
            },
        );

        assert!(registry.get_stdin_tx(&key).is_none());
    }

    #[test]
    fn test_replace_output_sink() {
        let registry = ProcessRegistry::new();
        let key = make_key("cmd", "/tmp");
        let alive = Arc::new(AtomicBool::new(true));
        let (tx, _rx) = mpsc::channel(1);

        registry.register(
            key.clone(),
            RegisteredProcess {
                alive: alive.clone(),
                stdin_tx: tx,
                output_sink: make_output_sink(),
            },
        );

        let (new_tx, _new_rx) = mpsc::channel(1);
        let result = registry.replace_output_sink(&key, new_tx);
        assert!(result.is_some());
    }

    #[test]
    fn test_replace_output_sink_dead_process() {
        let registry = ProcessRegistry::new();
        let key = make_key("cmd", "/tmp");
        let alive = Arc::new(AtomicBool::new(false));
        let (tx, _rx) = mpsc::channel(1);

        registry.register(
            key.clone(),
            RegisteredProcess {
                alive: alive.clone(),
                stdin_tx: tx,
                output_sink: make_output_sink(),
            },
        );

        let (new_tx, _new_rx) = mpsc::channel(1);
        let result = registry.replace_output_sink(&key, new_tx);
        assert!(result.is_none());
    }
}
