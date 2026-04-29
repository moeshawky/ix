//! Idle/dormancy detection for the daemon.

use std::time::Instant;

/// Activity level of the indexing daemon, based on time since last query
/// or filesystem event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonState {
    /// Less than 5 minutes since the last query or change.
    Active,
    /// Between 5 and 30 minutes of inactivity.
    Idle,
    /// More than 30 minutes of inactivity.
    Dormant,
}

/// Tracks elapsed time since the last query and the last filesystem change
/// to determine the current [`DaemonState`].
pub struct IdleTracker {
    last_query: Instant,
    last_change: Instant,
}

impl Default for IdleTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl IdleTracker {
    /// Create a new tracker with the current time for both timestamps.
    #[must_use]
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            last_query: now,
            last_change: now,
        }
    }

    /// Mark that a search query was just received.
    pub fn record_query(&mut self) {
        self.last_query = Instant::now();
    }

    /// Mark that a filesystem change was just processed.
    pub fn record_change(&mut self) {
        self.last_change = Instant::now();
    }

    /// Return the current daemon state based on elapsed inactivity.
    #[must_use]
    pub fn state(&self) -> DaemonState {
        let last_activity = self.last_query.max(self.last_change);
        let elapsed = last_activity.elapsed().as_secs();

        if elapsed < 5 * 60 {
            DaemonState::Active
        } else if elapsed < 30 * 60 {
            DaemonState::Idle
        } else {
            DaemonState::Dormant
        }
    }
}

#[cfg(test)]
#[allow(clippy::as_conversions, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_idle_tracker() {
        let mut tracker = IdleTracker::new();
        assert_eq!(tracker.state(), DaemonState::Active);

        // Simulate 6 minutes of inactivity
        tracker.last_query = Instant::now().checked_sub(Duration::from_mins(6)).unwrap();
        tracker.last_change = Instant::now().checked_sub(Duration::from_mins(6)).unwrap();
        assert_eq!(tracker.state(), DaemonState::Idle);

        // Simulate 31 minutes of inactivity
        tracker.last_query = Instant::now().checked_sub(Duration::from_mins(31)).unwrap();
        tracker.last_change = Instant::now().checked_sub(Duration::from_mins(31)).unwrap();
        assert_eq!(tracker.state(), DaemonState::Dormant);

        // Record activity and check if it's Active again
        tracker.record_query();
        assert_eq!(tracker.state(), DaemonState::Active);
    }
}
