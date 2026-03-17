use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tracing::info;

/// Key for deduplicating long-lived processes: (command, working_dir).
pub type ProcessKey = (String, String);

/// A registered long-lived process with an alive flag and stdin sender.
pub struct RegisteredProcess {
    /// Shared flag set to `false` by the output-streaming task when the child exits.
    pub alive: Arc<AtomicBool>,
    /// Channel for forwarding stdin data to the child process.
    pub stdin_tx: mpsc::Sender<Vec<u8>>,
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
            },
        );

        assert!(registry.get_stdin_tx(&key).is_none());
    }
}
