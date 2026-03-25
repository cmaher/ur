use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::page::TabId;

/// Cooldown duration between batched UI event fetches.
const COOLDOWN: Duration = Duration::from_millis(200);

/// Tracks which tab pages have dirty data from UI events and manages a
/// cooldown window so that rapid-fire events are batched into periodic
/// fetches rather than triggering one fetch per event.
pub struct PageThrottle {
    /// Tabs whose data has changed since the last flush.
    dirty: HashSet<TabId>,
    /// When the current cooldown window started, if one is active.
    cooldown_start: Option<Instant>,
}

impl PageThrottle {
    /// Create a new throttle with no dirty pages and no active cooldown.
    pub fn new() -> Self {
        Self {
            dirty: HashSet::new(),
            cooldown_start: None,
        }
    }

    /// Mark the given tabs as dirty (their data has changed).
    pub fn mark_dirty(&mut self, tabs: impl IntoIterator<Item = TabId>) {
        self.dirty.extend(tabs);
    }

    /// Returns true if the cooldown has elapsed and there are dirty pages
    /// waiting to be flushed.
    pub fn should_flush(&self) -> bool {
        if self.dirty.is_empty() {
            return false;
        }
        match self.cooldown_start {
            None => true,
            Some(start) => start.elapsed() >= COOLDOWN,
        }
    }

    /// Drain all dirty tabs and restart the cooldown timer.
    ///
    /// Returns the set of tabs that were dirty. The caller is responsible
    /// for fetching data for the active tab and marking others stale.
    pub fn flush(&mut self) -> HashSet<TabId> {
        let tabs = std::mem::take(&mut self.dirty);
        if !tabs.is_empty() {
            self.cooldown_start = Some(Instant::now());
        }
        tabs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_throttle_has_no_dirty_pages() {
        let throttle = PageThrottle::new();
        assert!(throttle.dirty.is_empty());
        assert!(throttle.cooldown_start.is_none());
    }

    #[test]
    fn mark_dirty_adds_tabs() {
        let mut throttle = PageThrottle::new();
        throttle.mark_dirty([TabId::Tickets, TabId::Flows]);
        assert!(throttle.dirty.contains(&TabId::Tickets));
        assert!(throttle.dirty.contains(&TabId::Flows));
    }

    #[test]
    fn should_flush_true_when_dirty_and_no_cooldown() {
        let mut throttle = PageThrottle::new();
        throttle.mark_dirty([TabId::Tickets]);
        assert!(throttle.should_flush());
    }

    #[test]
    fn should_flush_false_when_empty() {
        let throttle = PageThrottle::new();
        assert!(!throttle.should_flush());
    }

    #[test]
    fn should_flush_false_during_cooldown() {
        let mut throttle = PageThrottle::new();
        throttle.mark_dirty([TabId::Tickets]);
        // Simulate an active cooldown that just started.
        throttle.cooldown_start = Some(Instant::now());
        assert!(!throttle.should_flush());
    }

    #[test]
    fn should_flush_true_after_cooldown_elapsed() {
        let mut throttle = PageThrottle::new();
        throttle.mark_dirty([TabId::Tickets]);
        // Set cooldown to a point far in the past.
        throttle.cooldown_start = Some(Instant::now() - Duration::from_millis(300));
        assert!(throttle.should_flush());
    }

    #[test]
    fn flush_returns_dirty_set_and_clears() {
        let mut throttle = PageThrottle::new();
        throttle.mark_dirty([TabId::Tickets, TabId::Workers]);
        let flushed = throttle.flush();
        assert!(flushed.contains(&TabId::Tickets));
        assert!(flushed.contains(&TabId::Workers));
        assert!(throttle.dirty.is_empty());
        assert!(throttle.cooldown_start.is_some());
    }

    #[test]
    fn flush_empty_does_not_start_cooldown() {
        let mut throttle = PageThrottle::new();
        let flushed = throttle.flush();
        assert!(flushed.is_empty());
        assert!(throttle.cooldown_start.is_none());
    }

    #[test]
    fn events_during_cooldown_accumulate() {
        let mut throttle = PageThrottle::new();
        throttle.mark_dirty([TabId::Tickets]);
        let _ = throttle.flush();

        // During cooldown, mark more dirty.
        throttle.mark_dirty([TabId::Flows]);
        assert!(!throttle.should_flush()); // Still in cooldown.
        assert!(throttle.dirty.contains(&TabId::Flows));
    }
}
