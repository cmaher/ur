//! Per-worker ordering of Launch and Stop (ur-j5puq).
//!
//! Stop is a barrier: [`WorkerBarrier::begin_stop`] tombstones the id first,
//! then waits for any in-flight launch of it; a launch that starts later sees
//! the tombstone under the id's lock and is refused. So once a Stop returns
//! Ok, the id is not running in this builderd and never will be. Tombstones
//! live in memory only and expire after `ttl` (any launch RPC older than that
//! is long dead). Tombstones do not survive a builderd restart, and the
//! server's launch task may resend a Launch to the new process after its
//! stopper's Stop returned Ok from the old one; that launch then runs. The
//! server covers this itself: a launch whose finalize finds its row already
//! stopped issues its own Stop for what it launched.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::OwnedMutexGuard;
use tonic::Status;

/// Production tombstone lifetime.
pub const TOMBSTONE_TTL: Duration = Duration::from_secs(3600);

#[derive(Default)]
struct State {
    locks: HashMap<String, Arc<tokio::sync::Mutex<()>>>,
    tombstones: HashMap<String, Instant>,
    /// Container launched per id (container service only): its name, from
    /// before `docker run`, then its ID once the launch succeeds. A Stop
    /// stops it whatever container id the stopper read from its row.
    launched: HashMap<String, String>,
}

/// Per-id Launch/Stop serialization plus stop tombstones.
#[derive(Clone)]
pub struct WorkerBarrier {
    state: Arc<Mutex<State>>,
    ttl: Duration,
}

impl WorkerBarrier {
    pub fn new(ttl: Duration) -> Self {
        WorkerBarrier {
            state: Arc::default(),
            ttl,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let ttl = self.ttl;
        state.tombstones.retain(|_, at| at.elapsed() < ttl);
        // Locks nobody holds or waits on can go.
        state.locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        state
    }

    fn lock_for(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.state().locks.entry(id.to_owned()).or_default().clone()
    }

    /// Hold `id`'s lock for a launch, or refuse it if `id` was stopped.
    pub async fn begin_launch(&self, id: &str) -> Result<OwnedMutexGuard<()>, Status> {
        let guard = self.lock_for(id).lock_owned().await;
        if self.state().tombstones.contains_key(id) {
            return Err(Status::failed_precondition(format!(
                "worker '{id}' was stopped; refusing to launch it"
            )));
        }
        Ok(guard)
    }

    /// Tombstone `id`, then wait for any in-flight launch of it to finish.
    pub async fn begin_stop(&self, id: &str) -> OwnedMutexGuard<()> {
        self.state()
            .tombstones
            .insert(id.to_owned(), Instant::now());
        self.lock_for(id).lock_owned().await
    }

    /// Record the container a launch of `id` started.
    pub fn record_launched(&self, id: &str, container: &str) {
        self.state()
            .launched
            .insert(id.to_owned(), container.to_owned());
    }

    /// The container a launch of `id` started.
    pub fn launched(&self, id: &str) -> Option<String> {
        self.state().launched.get(id).cloned()
    }

    /// Forget `id`'s launched container once it is stopped.
    pub fn forget_launched(&self, id: &str) {
        self.state().launched.remove(id);
    }

    #[cfg(test)]
    fn is_tombstoned(&self, id: &str) -> bool {
        self.state().tombstones.contains_key(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stop_before_launch_refuses_the_launch() {
        let b = WorkerBarrier::new(TOMBSTONE_TTL);
        drop(b.begin_stop("w1").await);
        let err = b.begin_launch("w1").await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
        b.begin_launch("w2").await.unwrap();
    }

    #[tokio::test]
    async fn stop_waits_for_an_in_flight_launch() {
        let b = WorkerBarrier::new(TOMBSTONE_TTL);
        let launch = b.begin_launch("w1").await.unwrap();
        let stopper = b.clone();
        let stop = tokio::spawn(async move { drop(stopper.begin_stop("w1").await) });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!stop.is_finished(), "stop must wait for the launch");
        assert!(b.is_tombstoned("w1"), "tombstoned before waiting");
        drop(launch);
        stop.await.unwrap();
        assert!(b.begin_launch("w1").await.is_err());
    }

    #[tokio::test]
    async fn tombstones_expire_and_idle_locks_are_dropped() {
        let b = WorkerBarrier::new(Duration::from_millis(20));
        drop(b.begin_stop("w1").await);
        assert!(b.is_tombstoned("w1"));
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(!b.is_tombstoned("w1"), "pruned after the ttl");
        drop(b.begin_launch("w1").await.unwrap());
        assert!(b.state().locks.is_empty(), "idle locks pruned");
    }
}
