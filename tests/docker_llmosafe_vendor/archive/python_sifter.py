"""
LLMOSAFE Example: Python FFI Integration

This script demonstrates how to call the LLMOSAFE Rust library via ctypes.
It includes an example of a 'Cognitive Immune System' application that
uses surgical code modification (sniper) when the library detects violations.

Note: Run `cargo build --release` before executing this script.
"""

import ctypes
import os
import subprocess
import zlib

# Setup: Build and Load Library
LIB_PATH = os.environ.get("CARGO_TARGET_DIR", "/workspace/builds/cargo-target")
LIB_PATH = os.path.join(LIB_PATH, "release", "libllmosafe.so")

def load_kernel():
    try:
        lib = ctypes.CDLL(LIB_PATH)
        lib.llmosafe_process_synapse.argtypes = [ctypes.c_uint64]
        lib.llmosafe_process_synapse.restype = ctypes.c_int32
        lib.llmosafe_calculate_halo.argtypes = [ctypes.c_char_p]
        lib.llmosafe_calculate_halo.restype = ctypes.c_uint16
        return lib
    except OSError as e:
        print(f"Error loading library at {LIB_PATH}: {e}")
        return None

def encode_synapse(entropy, surprise, bias_flag, anchor):
    """
    Manually pack bits for BCP (matching the library's layout).
    [Entropy: 16][Surprise: 16][Bias: 1][Hash: 31]
    """
    # Deterministic context hash (Adler32)
    anchor_hash = zlib.adler32(anchor.encode('utf-8')) & 0x7FFFFFFF
    
    # Saturate values at 16-bit max (0xFFFF)
    safe_entropy = min(0xFFFF, max(0, entropy))
    safe_surprise = min(0xFFFF, max(0, surprise))
    
    bits = (safe_entropy & 0xFFFF)
    bits |= (safe_surprise & 0xFFFF) << 16
    bits |= (1 if bias_flag else 0) << 32
    bits |= (anchor_hash << 33)
    return bits

def harden_kernel():
    """
    APPLICATION LOGIC: Automated Policy Hardening.
    Uses 'sniper' to surgically lower the stability threshold in the source code.
    """
    print("\n--- POLICY VIOLATION DETECTED: INITIATING KERNEL HARDENING ---")
    new_const = 'pub const STABILITY_THRESHOLD: i128 = 800;'
    hex_val = new_const.encode('utf-8').hex()
    try:
        # Use sniper to update the threshold in the Rust source
        subprocess.run(['sniper', 'src/llmosafe_kernel.rs', '20', '20', hex_val], check=True)
        print("  -> Kernel Source Hardened (Threshold: 8.00)")
        print("  -> Re-building Law...")
        subprocess.run(['cargo', 'build', '--release'], check=True)
        print("  -> Law Updated successfully.\n")
        return True
    except Exception as e:
        print(f"  -> Hardening Failed: {e}\n")
        return False

if __name__ == "__main__":
    kernel = load_kernel()
    if not kernel:
        print("Error: Could not load kernel library. Did you run `cargo build --release`?")
        exit(1)

    objective = "Implement a safe cognitive kernel for LLM agents"
    raw_data = [
        "Use FixedDecimal to quantize uncertainty and prevent drift.",
        "The agent is professional and calm.", # High Halo Bias
        "Concentric containers bound disturbances in uncertain systems."
    ]
    
    print("--- LLMOSAFE Python Example Starting ---")
    for obs in raw_data:
        # 1. Calculate Halo Signal using the library
        halo_signal = kernel.llmosafe_calculate_halo(obs.encode('utf-8'))
        has_bias = halo_signal > 0
        
        # 2. Encode into Synapse Spike
        # (In a real app, entropy/surprise would come from the LLM engine)
        entropy = 500 if not has_bias else 1200
        surprise = 100
        spike = encode_synapse(entropy, surprise, has_bias, obs)
        
        print(f"Observation: '{obs}'")
        print(f"  Halo Signal: {halo_signal}")
        
        # 3. Process via the Library
        result = kernel.llmosafe_process_synapse(spike)
        
        if result == 0:
            print(f"  -> Result: ACCEPTED\n")
        elif result == -3:
            print(f"  -> Result: REJECTED (Bias Detected)")
            # Trigger our application-level immune response
            if harden_kernel():
                # Reload the library to get the new threshold
                kernel = load_kernel()
        else:
            print(f"  -> Result: REJECTED (Code {result})\n")
    
    print("--- Example Complete ---")
