use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::data::{ActionResult, DataPayload};

/// Events consumed by the main application loop.
#[derive(Debug)]
pub enum AppEvent {
    /// A keyboard event forwarded from the crossterm reader task.
    Key(KeyEvent),
    /// Periodic tick for background data refresh.
    Tick,
    /// A gRPC data-fetch task completed and delivered a payload.
    DataReady(DataPayload),
    /// An async action (dispatch, etc.) completed with a result.
    ActionResult(ActionResult),
    /// Terminal was resized to (columns, rows).
    Resize(u16, u16),
}

const TICK_INTERVAL: Duration = Duration::from_secs(5);
const CROSSTERM_POLL_TIMEOUT: Duration = Duration::from_millis(10);

/// Channel-based event manager.
///
/// Spawns two background tasks (crossterm reader, tick timer) and exposes a
/// sender so that external data-fetch tasks can push `DataReady` events.
/// The main loop calls [`EventManager::recv`] to consume events without
/// blocking on any single producer.
///
/// Implements `Clone` — cloning shares the sender but not the receiver.
#[derive(Clone)]
pub struct EventManager {
    sender: mpsc::UnboundedSender<AppEvent>,
}

/// Owned receiver half, consumed by the main application loop.
pub struct EventReceiver {
    receiver: mpsc::UnboundedReceiver<AppEvent>,
    /// Handles kept alive so tasks are cancelled on drop.
    _crossterm_handle: JoinHandle<()>,
    _tick_handle: JoinHandle<()>,
}

impl EventManager {
    /// Create a new `EventManager`, spawning the crossterm reader and tick
    /// timer tasks. Returns the manager (clonable sender) and the receiver
    /// that the main loop should `select!` on.
    pub fn start() -> (Self, EventReceiver) {
        let (tx, rx) = mpsc::unbounded_channel();

        let crossterm_handle = spawn_crossterm_reader(tx.clone());
        let tick_handle = spawn_tick_timer(tx.clone());

        let manager = Self { sender: tx };

        let receiver = EventReceiver {
            receiver: rx,
            _crossterm_handle: crossterm_handle,
            _tick_handle: tick_handle,
        };

        (manager, receiver)
    }

    /// Get a sender that data-fetch tasks can use to push `DataReady` events.
    pub fn sender(&self) -> mpsc::UnboundedSender<AppEvent> {
        self.sender.clone()
    }
}

impl EventReceiver {
    /// Wait for the next event from any producer.
    ///
    /// Returns `None` when all senders have been dropped (clean shutdown).
    pub async fn recv(&mut self) -> Option<AppEvent> {
        self.receiver.recv().await
    }

    /// Non-blocking receive: returns `Ok(event)` if one is ready, or
    /// `Err` if the channel is empty.
    pub fn try_recv(&mut self) -> Result<AppEvent, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }
}

/// Spawns a blocking task that reads crossterm terminal events and forwards
/// `Key` and `Resize` variants through the channel.
fn spawn_crossterm_reader(tx: mpsc::UnboundedSender<AppEvent>) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || while read_and_forward_event(&tx) {})
}

/// Poll for one crossterm event and forward it. Returns `false` to stop the loop.
fn read_and_forward_event(tx: &mpsc::UnboundedSender<AppEvent>) -> bool {
    match event::poll(CROSSTERM_POLL_TIMEOUT) {
        Ok(true) => forward_event(tx),
        Ok(false) => !tx.is_closed(),
        Err(_) => false,
    }
}

/// Read a single crossterm event and send it. Returns `false` if the channel is gone.
fn forward_event(tx: &mpsc::UnboundedSender<AppEvent>) -> bool {
    match event::read() {
        Ok(Event::Key(key)) => tx.send(AppEvent::Key(key)).is_ok(),
        Ok(Event::Resize(cols, rows)) => tx.send(AppEvent::Resize(cols, rows)).is_ok(),
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Spawns an async task that sends `Tick` at a fixed interval.
fn spawn_tick_timer(tx: mpsc::UnboundedSender<AppEvent>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK_INTERVAL);
        // The first tick fires immediately; skip it so the UI gets a full
        // interval before the first refresh.
        interval.tick().await;

        loop {
            interval.tick().await;
            if tx.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test the channel plumbing without spawning real crossterm/tick tasks,
    /// which hang in non-TTY CI environments.
    #[tokio::test]
    async fn data_ready_round_trip() {
        let (tx, mut rx) = mpsc::unbounded_channel();

        let payload = DataPayload::Tickets(Ok(vec![]));
        tx.send(AppEvent::DataReady(payload)).unwrap();
        drop(tx);

        let found = find_data_ready(&mut rx).await;
        assert!(found, "expected DataReady event");
    }

    async fn find_data_ready(rx: &mut mpsc::UnboundedReceiver<AppEvent>) -> bool {
        while let Some(ev) = rx.recv().await {
            if matches!(ev, AppEvent::DataReady(_)) {
                return true;
            }
        }
        false
    }

    #[tokio::test]
    async fn channel_closes_when_senders_dropped() {
        let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
        tx.send(AppEvent::Tick).unwrap();
        drop(tx);

        // Should receive the one event, then None.
        assert!(rx.recv().await.is_some());
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn action_result_round_trip() {
        let (tx, mut rx) = mpsc::unbounded_channel();

        let result = ActionResult {
            result: Ok("Dispatched ur-123".into()),
            silent_on_success: false,
        };
        tx.send(AppEvent::ActionResult(result)).unwrap();
        drop(tx);

        let ev = rx.recv().await.unwrap();
        assert!(matches!(ev, AppEvent::ActionResult(_)));
    }

    #[test]
    fn event_manager_sender_is_clone() {
        fn assert_clone<T: Clone>() {}
        assert_clone::<EventManager>();
    }
}
