//! Idle detection (Linux/macOS/Windows).

pub struct IdleDetector;

impl Default for IdleDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl IdleDetector {
    pub fn new() -> Self {
        Self
    }

    /// Check if the system is currently "idle" enough for background indexing.
    pub fn is_idle(&self) -> bool {
        // For now, always return true as a stub.
        // REAL implementation would check CPU/IO/input as per DESIGN.md.
        true
    }
}
