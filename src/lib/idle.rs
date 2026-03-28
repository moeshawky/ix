//! Idle/dormancy detection for the daemon.

use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonState {
    Active,
    Idle,
    Dormant,
}

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
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            last_query: now,
            last_change: now,
        }
    }

    pub fn record_query(&mut self) {
        self.last_query = Instant::now();
    }

    pub fn record_change(&mut self) {
        self.last_change = Instant::now();
    }

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
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_idle_tracker() {
        let mut tracker = IdleTracker::new();
        assert_eq!(tracker.state(), DaemonState::Active);

        // Simulate 6 minutes of inactivity
        tracker.last_query = Instant::now() - Duration::from_secs(6 * 60);
        tracker.last_change = Instant::now() - Duration::from_secs(6 * 60);
        assert_eq!(tracker.state(), DaemonState::Idle);

        // Simulate 31 minutes of inactivity
        tracker.last_query = Instant::now() - Duration::from_secs(31 * 60);
        tracker.last_change = Instant::now() - Duration::from_secs(31 * 60);
        assert_eq!(tracker.state(), DaemonState::Dormant);

        // Record activity and check if it's Active again
        tracker.record_query();
        assert_eq!(tracker.state(), DaemonState::Active);
    }
}
