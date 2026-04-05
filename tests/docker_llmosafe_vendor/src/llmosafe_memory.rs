//! LLMOSAFE Tier 2 Cognitive Working Memory
//! 
//! This module implements the "Memory Integrity" Axiom.
//! It uses the Seshat principle of "Tangible Ratios" (Sekel)
//! to maintain a fixed-size cognitive state without heap allocation.
//!
//! Research Grounds:
//! - Titans: Surprise-based gating with momentum.
//! - TransformerFAM: Feedback-loop working memory.
//! - Infini-attention: Compressive associative memory.

use crate::llmosafe_kernel::{CognitiveEntropy, KernelError, Synapse, Privilege, ActionRequest};

/// NegativeSelectionLedger (The CRISPR Layer).
/// Remembers failed reasoning paths to prevent wasted compute.
pub struct NegativeSelectionLedger {
    buffer: [u64; 16],
    head: usize,
}

impl NegativeSelectionLedger {
    pub fn new() -> Self {
        Self {
            buffer: [u64::MAX; 16],
            head: 0,
        }
    }

    pub fn record(&mut self, hash: u64) {
        self.buffer[self.head] = hash;
        self.head = (self.head + 1) % 16;
    }

    pub fn contains(&self, hash: u64) -> bool {
        for val in self.buffer.iter() {
            if *val == hash { return true; }
        }
        false
    }
}

impl Default for NegativeSelectionLedger {
    fn default() -> Self {
        Self::new()
    }
}

/// Ratio: 64 "Palms" (anchors) for persistent reasoning.
pub struct WorkingMemory<const SIZE: usize = 64> {
    state: [CognitiveEntropy<28, 2>; SIZE],
    current_index: usize,
    surprise_threshold: i128,
    pub capability_mask: u64,
    pub ledger: NegativeSelectionLedger,
}

impl<const SIZE: usize> WorkingMemory<SIZE> {
    pub fn new(threshold: i128) -> Self {
        Self {
            state: [CognitiveEntropy::new(0); SIZE],
            current_index: 0,
            surprise_threshold: threshold,
            capability_mask: Privilege::CAP_SANDBOX,
            ledger: NegativeSelectionLedger::new(),
        }
    }


    /// Updated: Uses the Synapse protocol for state transitions.
    ///
    /// # Examples
    ///
    /// ```
    /// use llmosafe::{WorkingMemory, Synapse};
    /// let mut memory = WorkingMemory::<64>::new(1000);
    /// let synapse = Synapse::from_raw_u64(0);
    /// assert!(memory.update(synapse).is_ok());
    /// ```
    pub fn authorize(&mut self, action: ActionRequest) -> Result<(), KernelError> {
        let required = action.required_privilege();
        if (self.capability_mask & required) == required {
            Ok(())
        } else {
            // Environment variable bypass for backward compatibility/testing
            let ok = match action {
                ActionRequest::IndexRoot if std::env::var("LLMOSAFE_AUTH_ROOT").is_ok() => true,
                ActionRequest::ExternalApiCall if std::env::var("LLMOSAFE_AUTH_NETWORK").is_ok() => true,
                _ => false,
            };

            if ok {
                Ok(())
            } else {
                // IMMUNE MEMORY: Record the failed intent
                self.ledger.record(required); 
                Err(KernelError::PermissionDenied)
            }
        }
    }

    /// Support recursive privilege restriction (Viral Bitmask Economy).
    pub fn narrow(&mut self, restriction_mask: u64) {
        self.capability_mask &= restriction_mask;
    }

    pub fn update(&mut self, mut synapse: Synapse) -> Result<(), KernelError> {
        // IMMUNE MEMORY: Check if the current intent was previously blocked
        let intent_hash = synapse.anchor_hash() as u64; 
        if self.ledger.contains(intent_hash) {
            synapse.set_backtrack_requested(true);
        }

        synapse.validate()?;

        if synapse.surprise() > self.surprise_threshold {
            return Err(KernelError::HallucinationDetected);
        }

        self.state[self.current_index] = synapse.entropy();
        self.current_index = (self.current_index + 1) % SIZE;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llmosafe_kernel::Synapse;

    #[test]
    fn test_memory_update_gating() {
        let mut memory = WorkingMemory::<4>::new(500); // Threshold 5.00

        // 1. Valid update (Low surprise, no bias, stable entropy)
        let mut synapse = Synapse::new();
        synapse.set_raw_entropy(400);
        synapse.set_raw_surprise(100);
        synapse.set_has_bias(false);
        assert!(memory.update(synapse).is_ok());

        // 2. Invalid update: Surprise too high (Hallucination)
        let mut synapse = Synapse::new();
        synapse.set_raw_entropy(400);
        synapse.set_raw_surprise(600);
        synapse.set_has_bias(false);
        assert_eq!(memory.update(synapse).unwrap_err(), KernelError::HallucinationDetected);

        // 3. Invalid update: Bias detected
        let mut synapse = Synapse::new();
        synapse.set_raw_entropy(400);
        synapse.set_raw_surprise(100);
        synapse.set_has_bias(true);
        assert_eq!(memory.update(synapse).unwrap_err(), KernelError::BiasHaloDetected);

        // 4. Invalid update: Cognitive Instability
        let mut synapse = Synapse::new();
        synapse.set_raw_entropy(1100);
        synapse.set_raw_surprise(100);
        synapse.set_has_bias(false);
        assert_eq!(memory.update(synapse).unwrap_err(), KernelError::CognitiveInstability);
    }

    #[test]
    fn test_working_memory_size_1() {
        let mut memory = WorkingMemory::<1>::new(1000);
        let mut s1 = Synapse::new();
        s1.set_raw_entropy(100);
        // Ensure anchor hash is not the default u64::MAX used in ledger
        s1.set_anchor_hash(1);
        let mut s2 = Synapse::new();
        s2.set_raw_entropy(200);
        s2.set_anchor_hash(2);
        
        memory.update(s1).unwrap();
        assert!(memory.state[0].is_stable(100));
        
        memory.update(s2).unwrap();
        assert!(memory.state[0].is_stable(200));
        assert_eq!(memory.current_index, 0);
    }

    #[test]
    fn test_memory_new_max_threshold() {
        let memory = WorkingMemory::<64>::new(i128::MAX);
        assert_eq!(memory.surprise_threshold, i128::MAX);
    }

    #[test]
    fn test_memory_zero_threshold() {
        let mut memory = WorkingMemory::<64>::new(0);
        let mut synapse = Synapse::new();
        synapse.set_raw_surprise(1);
        synapse.set_anchor_hash(123);
        // Any surprise > 0 should fail
        assert_eq!(memory.update(synapse).unwrap_err(), KernelError::HallucinationDetected);
    }

    #[test]
    fn test_memory_negative_threshold() {
        let mut memory = WorkingMemory::<64>::new(-1);
        let mut synapse = Synapse::new();
        synapse.set_anchor_hash(456);
        // Even surprise 0 > -1, so it should fail
        assert_eq!(memory.update(synapse).unwrap_err(), KernelError::HallucinationDetected);
    }

    #[test]
    fn test_viral_bitmask_economy() {
        let mut memory = WorkingMemory::<64>::new(1000);

        // Default should be CAP_SANDBOX
        assert!(memory.authorize(ActionRequest::StandardProcessing).is_ok());
        assert!(memory.authorize(ActionRequest::IndexRoot).is_err());

        // Grant Root
        memory.capability_mask |= Privilege::CAP_ROOT;
        assert!(memory.authorize(ActionRequest::IndexRoot).is_ok());

        // Narrow to just Sandbox (removing Root)
        memory.narrow(Privilege::CAP_SANDBOX);
        assert_eq!(memory.capability_mask, Privilege::CAP_SANDBOX);
        assert!(memory.authorize(ActionRequest::StandardProcessing).is_ok());
        assert!(memory.authorize(ActionRequest::IndexRoot).is_err());
    }

}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;
    use crate::llmosafe_kernel::Synapse;

    proptest! {
        #[test]
        fn test_working_memory_random_synapse_sequence(
            entropies in prop::collection::vec(0u16..800u16, 1..200)
        ) {
            let mut memory = WorkingMemory::<64>::new(1000);
            for e in entropies {
                let mut synapse = Synapse::new();
                synapse.set_raw_entropy(e);
                // Ensure hash doesn't trigger immune memory false positive
                synapse.set_anchor_hash(0x12345678);
                prop_assert!(memory.update(synapse).is_ok());
            }
        }
    }
}

pub mod cognitive_memory {
    use super::*;
use once_cell::sync::Lazy;
    use std::sync::Mutex;

    /// Global singleton for the Cognitive Working Memory.
    /// Uses a Mutex for thread-safety in distributed hive environments.
    static GLOBAL_MEMORY: Lazy<Mutex<WorkingMemory<64>>> = Lazy::new(|| Mutex::new(WorkingMemory::<64>::new(500))); // Threshold = 5.00

    pub fn get_global_memory() -> &'static Mutex<WorkingMemory<64>> {
        &GLOBAL_MEMORY
    }

    /// Bridges the working memory to the reasoning core via Synapse.
    /// Returns 0 on success, or a negative error code.
    pub fn process_state_update(synapse_bits: u64) -> i32 {
        let synapse = Synapse::from_raw_u64(synapse_bits);

        let mut memory = GLOBAL_MEMORY.lock().unwrap();

        match memory.update(synapse) {
            Ok(_) => 0,
            Err(KernelError::DepthExceeded) => -1,
            Err(KernelError::CognitiveInstability) => -2,
            Err(KernelError::BiasHaloDetected) => -3,
            Err(KernelError::HallucinationDetected) => -4,
            Err(KernelError::ResourceExhaustion) => -5,
            Err(KernelError::PermissionDenied) => -6,
            Err(KernelError::BacktrackSignaled) => -7,
        }
    }
}
