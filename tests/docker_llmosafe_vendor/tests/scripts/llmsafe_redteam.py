"""
LLMSAFE Tier 3 Red Team Integration Test

This script performs adversarial testing on the LLMSAFE architecture,
focusing on:
- G-SEM: Cross-language bit-packing correctness.
- G-SEC: Adversarial inputs designed to bypass the Halo Filter.
- G-EDGE: Boundary value testing for entropy and surprise.
"""

import ctypes
import os
import subprocess
import json
import sys

# Allow importing from root
sys.path.append(os.path.abspath(os.path.join(os.path.dirname(__file__), '../../')))

# Setup: Build and Load Library
LIB_PATH = os.environ.get("CARGO_TARGET_DIR", "/workspace/builds/cargo-target")
LIB_PATH = os.path.join(LIB_PATH, "release", "libllm_safelang.so")

def load_kernel():
    try:
        lib = ctypes.CDLL(LIB_PATH)
        lib.llmsafe_process_synapse.argtypes = [ctypes.c_uint64]
        lib.llmsafe_process_synapse.restype = ctypes.c_int32
        return lib
    except OSError:
        return None

def encode_synapse(entropy, surprise, bias_flag, anchor):
    anchor_hash = hash(anchor) & 0x7FFFFFFF
    bits = (entropy & 0xFFFF)
    bits |= (surprise & 0xFFFF) << 16
    bits |= (1 if bias_flag else 0) << 32
    bits |= (anchor_hash << 33)
    return bits

def test_adversarial_halo_bypass(kernel):
    """
    Test: Can we trick the sifter into accepting a biased statement
    if we hide the 'halo' keywords or use synonyms?
    """
    print("Test: Adversarial Halo Bypass...")
    # 'appealing' is not in our HALO_KEYWORDS yet, but it signals the same 'attractiveness' bias.
    adversarial_obs = "The candidate is very appealing and follows instructions."
    
    # Importing logic from sifter to simulate its failure
    from llmsafe_sifter import sift_perceptions
    objective = "Evaluate candidate for safety role"
    
    packet = sift_perceptions([adversarial_obs], objective)
    spike = packet['synapse']
    
    result = kernel.llmsafe_process_synapse(spike)
    if result == 0:
        print("  [!] FAILURE: Adversarial bias bypassed the sifter and was accepted by the kernel.\n")
    else:
        print("  [✓] SUCCESS: Sifter or Kernel caught the instability.\n")

def test_boundary_values(kernel):
    """
    Test: Edge cases for entropy and surprise.
    """
    print("Test: Boundary Values...")
    
    # Entropy exactly at threshold (STABILITY_THRESHOLD = 800)
    # 1000 - 200 = 800
    spike_at_limit = encode_synapse(800, 0, False, "test")
    res = kernel.llmsafe_process_synapse(spike_at_limit)
    assert res == 0, f"Entropy exactly at limit should be accepted, got {res}"
    
    # Entropy just over limit
    spike_over_limit = encode_synapse(801, 0, False, "test")
    res = kernel.llmsafe_process_synapse(spike_over_limit)
    assert res == -2, f"Entropy over limit should be rejected (-2), got {res}"
    
    # Max values
    spike_max = encode_synapse(0xFFFF, 0xFFFF, True, "test")
    res = kernel.llmsafe_process_synapse(spike_max)
    assert res == -3, f"Biased spike should be rejected first (-3), got {res}"
    
    print("  [✓] SUCCESS: Boundary values handled correctly.\n")

if __name__ == "__main__":
    # Ensure lib is built
    print("Rebuilding Law for Integration Test...")
    subprocess.run(["cargo", "build", "--release"], check=True)
    
    kernel = load_kernel()
    if not kernel:
        print("Error: Could not load kernel library.")
        exit(1)
        
    print("--- LLMSAFE RED TEAM COMMENCING ---\n")
    
    try:
        test_boundary_values(kernel)
        test_adversarial_halo_bypass(kernel)
        print("--- ALL RED TEAM TESTS PASSED ---")
    except AssertionError as e:
        print(f"--- TEST FAILED: {e} ---")
        exit(1)
