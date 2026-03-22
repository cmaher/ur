use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::data::DataPayload;

/// Events consumed by the main application loop.
#[derive(Debug)]
pub enum AppEvent {
    /// A keyboard event forwarded from the crossterm reader task.
    Key(KeyEvent),
    /// Periodic tick for background data refresh.
    Tick,
    /// A gRPC data-fetch task completed and delivered a payload.
    DataReady(DataPayload),
    /// Terminal was resized to (columns, rows).
    Resize(u16, u16),
}

const TICK_INTERVAL: Duration = Duration::from_secs(5);
const CROSSTERM_POLL_TIMEOUT: Duration = Duration::from_millis(100);

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
}

/// Spawns a blocking task that reads crossterm terminal events and forwards
/// `Key` and `Resize` variants through the channel.
fn spawn_crossterm_reader(tx: mpsc::UnboundedSender<AppEvent>) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        loop {
            // Poll with a short timeout so we notice when the channel closes.
            match event::poll(CROSSTERM_POLL_TIMEOUT) {
                Ok(true) => match event::read() {
                    Ok(Event::Key(key)) => {
                        if tx.send(AppEvent::Key(key)).is_err() {
                            break;
                        }
                    }
                    Ok(Event::Resize(cols, rows)) => {
                        if tx.send(AppEvent::Resize(cols, rows)).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {} // Mouse events, focus events — ignored.
                    Err(_) => break,
                },
                Ok(false) => {
                    // No event ready; check if receiver is gone.
                    if tx.is_closed() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
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

    #[tokio::test]
    async fn data_ready_round_trip() {
        let (manager, mut receiver) = EventManager::start();

        let sender = manager.sender();
        let payload = DataPayload::Tickets(Ok(vec![]));
        sender.send(AppEvent::DataReady(payload)).unwrap();

        // We should receive the DataReady event (possibly after a Tick; drain
        // until we find it).
        drop(manager);
        drop(sender);

        let mut found = false;
        while let Some(ev) = receiver.recv().await {
            if matches!(ev, AppEvent::DataReady(_)) {
                found = true;
                break;
            }
        }
        assert!(found, "expected DataReady event");
    }

    #[tokio::test]
    async fn receiver_closes_when_senders_dropped() {
        let (manager, mut receiver) = EventManager::start();
        drop(manager);

        // Drain until None — the channel should close once spawned tasks
        // notice the closed sender and exit.
        let mut count = 0;
        while let Some(_) = receiver.recv().await {
            count += 1;
            if count > 1000 {
                panic!("channel did not close");
            }
        }
    }
}
