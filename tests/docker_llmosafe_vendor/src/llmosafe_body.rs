//! llmosafe_body — Physical resource guard (the autonomic nervous system).
//!
//! Monitors RSS memory, maps to CognitiveEntropy, triggers KernelError
//! when physical resource thresholds are crossed.

use crate::llmosafe_kernel::{KernelError, Synapse};

/// ResourceGuard monitors physical resource consumption and triggers safety halts.
/// Maps physical metrics (RAM, CPU) to the CognitiveEntropy/Synapse system.
#[derive(Debug, Clone)]
pub struct ResourceGuard {
    memory_ceiling_bytes: usize,
}

impl ResourceGuard {
    /// Creates a new ResourceGuard with a specified memory ceiling.
    pub fn new(memory_ceiling_bytes: usize) -> Self {
        Self { memory_ceiling_bytes }
    }

    /// Automatically create a guard based on a fraction of system memory.
    pub fn auto(fraction: f64) -> Self {
        let system_mem = Self::system_memory_bytes();
        let ceiling = (system_mem as f64 * fraction) as usize;
        Self::new(ceiling)
    }

    /// Checks current resource usage and returns a Synapse with mapped entropy.
    pub fn check(&self) -> Result<Synapse, KernelError> {
        let current_rss = Self::current_rss_bytes();
        let memory_ratio = if self.memory_ceiling_bytes > 0 {
            current_rss as f64 / self.memory_ceiling_bytes as f64
        } else {
            1.0
        };

        if memory_ratio >= 1.0 {
            return Err(KernelError::ResourceExhaustion);
        }

        let vitals = EnvironmentalVitals::now();
        let iowait_entropy = vitals.iowait_entropy();
        let load_entropy = vitals.load_entropy();
        
        // Final entropy is weighted: 50% Memory, 25% IO Wait, 25% Load
        let total_entropy = (memory_ratio * 500.0) + (iowait_entropy as f64 * 0.25) + (load_entropy as f64 * 0.25);
        let entropy_u16 = total_entropy.min(u16::MAX as f64) as u16;

        let mut synapse = Synapse::new();
        synapse.set_raw_entropy(entropy_u16);
        synapse.set_raw_surprise(0);
        synapse.set_has_bias(false);
        synapse.set_anchor_hash(0);

        Ok(synapse)
    }

    /// Returns current RSS memory usage in bytes.
    #[cfg(unix)]
    pub fn current_rss_bytes() -> usize {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
        if ret == 0 {
            (usage.ru_maxrss as usize) * 1024
        } else {
            Self::read_rss_from_proc()
        }
    }

    #[cfg(not(unix))]
    pub fn current_rss_bytes() -> usize { 0 }

    #[cfg(target_os = "linux")]
    fn read_rss_from_proc() -> usize {
        use std::fs;
        if let Ok(status) = fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if line.starts_with("VmRSS:") {
                    if let Some(size_str) = line.split_whitespace().nth(1) {
                        if let Ok(kb) = size_str.parse::<usize>() {
                            return kb * 1024;
                        }
                    }
                }
            }
        }
        0
    }

    #[cfg(not(target_os = "linux"))]
    fn read_rss_from_proc() -> usize { 0 }

    #[cfg(target_os = "linux")]
    pub fn system_memory_bytes() -> usize {
        use std::fs;
        if let Ok(meminfo) = fs::read_to_string("/proc/meminfo") {
            for line in meminfo.lines() {
                if line.starts_with("MemTotal:") {
                    if let Some(size_str) = line.split_whitespace().nth(1) {
                        if let Ok(kb) = size_str.parse::<usize>() {
                            return kb * 1024;
                        }
                    }
                }
            }
        }
        0
    }

    #[cfg(not(target_os = "linux"))]
    pub fn system_memory_bytes() -> usize { 0 }
}

/// EnvironmentalVitals tracks system-level pressure.
#[derive(Debug, Clone, Copy)]
pub struct EnvironmentalVitals {
    pub iowait_percent: f64,
    pub load_avg_1min: f64,
}

/// VitalsReport (C-compatible) for the Mycelial Introspection ABI.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VitalsReport {
    pub iowait_percent: f64,
    pub load_avg_1min: f64,
}

impl EnvironmentalVitals {
    pub fn now() -> Self {
        Self {
            iowait_percent: Self::read_iowait().unwrap_or(0.0),
            load_avg_1min: Self::read_loadavg().unwrap_or(0.0),
        }
    }

    pub fn report(&self) -> VitalsReport {
        VitalsReport {
            iowait_percent: self.iowait_percent,
            load_avg_1min: self.load_avg_1min,
        }
    }

    fn iowait_entropy(&self) -> u16 {
        // Capped at 20% iowait = 1000 entropy
        (self.iowait_percent * 50.0).min(1500.0) as u16
    }

    fn load_entropy(&self) -> u16 {
        // Capped at load 10.0 = 1000 entropy
        (self.load_avg_1min * 100.0).min(1500.0) as u16
    }

    #[cfg(target_os = "linux")]
    fn read_iowait() -> Option<f64> {
        use std::fs;
        let stat = fs::read_to_string("/proc/stat").ok()?;
        let first_line = stat.lines().next()?;
        let parts: Vec<&str> = first_line.split_whitespace().collect();
        if parts.len() > 5 {
            let iowait: f64 = parts[5].parse().ok()?;
            let mut total: f64 = 0.0;
            for part in parts.iter().skip(1) {
                total += part.parse::<f64>().unwrap_or(0.0);
            }
            if total > 0.0 {
                return Some((iowait / total) * 100.0);
            }
        }
        None
    }

    #[cfg(not(target_os = "linux"))]
    fn read_iowait() -> Option<f64> { None }

    #[cfg(target_os = "linux")]
    fn read_loadavg() -> Option<f64> {
        use std::fs;
        let load = fs::read_to_string("/proc/loadavg").ok()?;
        load.split_whitespace().next()?.parse().ok()
    }

    #[cfg(not(target_os = "linux"))]
    fn read_loadavg() -> Option<f64> { None }
}

/// MetabolicGovernor enforces pacing (The Metabolic Law).
pub struct MetabolicGovernor {
    last_step: std::time::Instant,
    min_interval: std::time::Duration,
}

impl MetabolicGovernor {
    pub fn new(min_interval_ms: u64) -> Self {
        Self {
            last_step: std::time::Instant::now() - std::time::Duration::from_millis(min_interval_ms),
            min_interval: std::time::Duration::from_millis(min_interval_ms),
        }
    }

    pub fn pace(&mut self) -> Result<(), KernelError> {
        let now = std::time::Instant::now();
        if now.duration_since(self.last_step) < self.min_interval {
            return Err(KernelError::CognitiveInstability);
        }
        self.last_step = now;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_guard_new_large_ceiling() {
        let guard = ResourceGuard::new(1024 * 1024 * 1024 * 100);
        let result = guard.check();
        assert!(result.is_ok());
        let synapse = result.unwrap();
        assert!(synapse.raw_entropy() < 500);
    }

    #[test]
    fn test_resource_guard_tiny_ceiling_exhaustion() {
        let guard = ResourceGuard::new(1);
        let result = guard.check();
        assert_eq!(result, Err(KernelError::ResourceExhaustion));
    }
}
