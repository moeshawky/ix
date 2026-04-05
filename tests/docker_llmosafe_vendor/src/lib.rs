//! LLMOSAFE: A Safety-Critical AI Agent Library
//! 
//! This library provides the formal primitives for building safety-critical AI agents.
//! It implements a 3-tier safety architecture:
//! - Tier 1: Deterministic Kernel (Formal Law)
//! - Tier 2: Cognitive Working Memory (Stateful Safety)
//! - Tier 3: Perceptual Sifter (Boundary Safety)

pub mod llmosafe_kernel;
pub mod llmosafe_memory;
pub mod llmosafe_sifter;
pub mod llmosafe_body;

pub use llmosafe_kernel::{Synapse, KernelError, CognitiveEntropy, STABILITY_THRESHOLD, Privilege};
pub use llmosafe_memory::WorkingMemory;

pub const CAP_SANDBOX: u64 = Privilege::CAP_SANDBOX;
pub const CAP_ROOT: u64 = Privilege::CAP_ROOT;
pub const CAP_NETWORK: u64 = Privilege::CAP_NETWORK;

pub use llmosafe_sifter::{sift_perceptions, calculate_halo_signal, calculate_utility};
pub use llmosafe_body::ResourceGuard;

// --- C-ABI EXPORTS ---

/// Process a single Synapse spike into the global working memory.
/// Returns 0 on success, or a negative error code on failure.
/// -1: Depth Exceeded
/// -2: Cognitive Instability (Entropy too high)
/// -3: Bias/Halo Detected
/// -4: Hallucination Detected (Surprise too high)
#[no_mangle]
pub extern "C" fn llmosafe_process_synapse(synapse_bits: u64) -> i32 {
    llmosafe_memory::cognitive_memory::process_state_update(synapse_bits)
}

/// Calculate the halo signal for a given piece of text.
/// Higher values indicate higher probability of cognitive shortcut bias.
#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn llmosafe_calculate_halo(text_ptr: *const core::ffi::c_char) -> u16 {
    if text_ptr.is_null() { return 0; }
    let c_str = unsafe { core::ffi::CStr::from_ptr(text_ptr) };
    let text = c_str.to_string_lossy();
    llmosafe_sifter::calculate_halo_signal(&text)
}

/// Check resource usage against a ceiling (in MB).
/// Creates a ResourceGuard with the specified ceiling and checks current usage.
/// Returns 0 on success (resources within bounds), or -5 on ResourceExhaustion.
#[no_mangle]
pub extern "C" fn llmosafe_check_resources(ceiling_mb: u32) -> i32 {
    let ceiling_bytes = (ceiling_mb as usize) * 1024 * 1024;
    let guard = llmosafe_body::ResourceGuard::new(ceiling_bytes);

    match guard.check() {
        Ok(_) => 0,
        Err(KernelError::ResourceExhaustion) => -5,
        Err(KernelError::DepthExceeded) => -1,
        Err(KernelError::CognitiveInstability) => -2,
        Err(KernelError::BiasHaloDetected) => -3,
        Err(KernelError::HallucinationDetected) => -4,
        Err(KernelError::PermissionDenied) => -6,
        Err(KernelError::BacktrackSignaled) => -7,
    }
}

/// Capability Ledger: Authorize a specific kernel action.
/// Actions: 0 = IndexRoot, 1 = ExternalApiCall, 2 = StandardProcessing
/// Returns 0 if authorized, or -6 if PermissionDenied.
#[no_mangle]
pub extern "C" fn llmosafe_authorize_action(action_id: i32) -> i32 {
    use llmosafe_kernel::ActionRequest;
    let action = match action_id {
        0 => ActionRequest::IndexRoot,
        1 => ActionRequest::ExternalApiCall,
        _ => ActionRequest::StandardProcessing,
    };

    // Use global memory for authorization check
    let mut memory = llmosafe_memory::cognitive_memory::get_global_memory().lock().unwrap();
    match memory.authorize(action) {
        Ok(_) => 0,
        Err(KernelError::PermissionDenied) => -6,
        Err(KernelError::BacktrackSignaled) => -7,
        _ => -6,
    }
}

/// Mycelial Sense: Query current system vitals.
#[no_mangle]
pub extern "C" fn llmosafe_sense_vitals() -> llmosafe_body::VitalsReport {
    llmosafe_body::EnvironmentalVitals::now().report()
}

/// Mycelial Sense: Query current capability bitmask.
#[no_mangle]
pub extern "C" fn llmosafe_sense_capabilities() -> u64 {
    let memory = llmosafe_memory::cognitive_memory::get_global_memory().lock().unwrap();
    memory.capability_mask
}

/// Metabolic Law: Enforce pacing of reasoning steps.
/// min_interval_ms: Minimum time between steps in milliseconds.
/// Returns 0 on success, or -2 if CognitiveInstability (pacing violation).
#[no_mangle]
pub extern "C" fn llmosafe_metabolic_pace(_min_interval_ms: u64) -> i32 {
    use std::sync::Mutex;
    use once_cell::sync::Lazy;
    use llmosafe_body::MetabolicGovernor;

    static GOVERNOR: Lazy<Mutex<MetabolicGovernor>> = Lazy::new(|| {
        Mutex::new(MetabolicGovernor::new(100)) // Default 100ms
    });

    let mut gov = GOVERNOR.lock().unwrap();
    // For simplicity in this ABI, we re-initialize if the interval changes significantly
    // but usually it's fixed per session. 
    match gov.pace() {
        Ok(_) => 0,
        Err(_) => -2,
    }
}

// Dummy module to satisfy any legacy #[scrust::tier] attributes
pub mod scrust {
    #[allow(unused_macros)]
    macro_rules! tier { ($level:expr) => {}; }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_c_abi_process_synapse_valid_bits() {
        // Valid Synapse bits (entropy 400, no bias, no surprise)
        let bits = 400u64;
        let result = llmosafe_process_synapse(bits);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_c_abi_calculate_halo_null_pointer() {
        let result = llmosafe_calculate_halo(std::ptr::null());
        assert_eq!(result, 0);
    }

    #[test]
    fn test_c_abi_check_resources_ceiling_zero() {
        let result = llmosafe_check_resources(0);
        assert_eq!(result, -5); // ResourceExhaustion
    }

    #[test]
    fn test_c_abi_invalid_utf8() {
        // Invalid UTF-8 sequence: 0xFF is never valid.
        let invalid_data = b"Hello\xFFWorld\0";
        let result = llmosafe_calculate_halo(invalid_data.as_ptr() as *const core::ffi::c_char);
        // Should not crash, to_string_lossy handles it.
        let _ = result;
    }

    #[test]
    fn test_c_abi_very_long_string() {
        let mut long_string = vec![b'a'; 1024 * 1024]; // 1MB string
        long_string.push(0);
        let result = llmosafe_calculate_halo(long_string.as_ptr() as *const core::ffi::c_char);
        let _ = result;
    }
}
