"""
LLMSAFE Tier 3 Perceptual Sifter (Immune System Version)

This script implements the "Perceptual Decoupling" Axiom and the 
"Cognitive Immune System" using sniper.
"""

import json
import ctypes
import os
import subprocess
import zlib

# Defense-in-Depth: Ensure stable hashing across process restarts
os.environ["PYTHONHASHSEED"] = "0"

# Sekel Ratios
SIFT_RATIO = 64
MAX_ENTROPY = 1000

# Halo/Bias Keywords
HALO_KEYWORDS = {
    "attractive", "calm", "professional", "creative", 
    "trustworthy", "confident", "leader", "obedient"
}

def calculate_utility(observation, objective):
    obs_tokens = set(observation.lower().split())
    obj_tokens = set(objective.lower().split())
    return len(obs_tokens.intersection(obj_tokens)) * 100

def calculate_halo_signal(observation):
    obs_tokens = set(observation.lower().split())
    return len(obs_tokens.intersection(HALO_KEYWORDS)) * 200

def encode_synapse(entropy, surprise, bias_flag, anchor):
    """
    BCP Encoder: Packs cognitive state into a u64.
    Uses SATURATING arithmetic to prevent bit-bleeding.
    """
    # Deterministic context hash (Adler32)
    anchor_hash = zlib.adler32(anchor.encode('utf-8')) & 0x7FFFFFFF
    
    # Saturate values at 16-bit max (0xFFFF) to prevent overflow bleeding
    safe_entropy = min(0xFFFF, max(0, entropy))
    safe_surprise = min(0xFFFF, max(0, surprise))
    
    bits = (safe_entropy & 0xFFFF)
    bits |= (safe_surprise & 0xFFFF) << 16
    bits |= (1 if bias_flag else 0) << 32
    bits |= (anchor_hash << 33)
    return bits

def sift_perceptions(raw_observations, current_objective):
    scored_obs = []
    for obs in raw_observations:
        utility = calculate_utility(obs, current_objective)
        bias_penalty = calculate_halo_signal(obs)
        scored_obs.append((obs, utility - bias_penalty))
    
    ranked = sorted(scored_obs, key=lambda x: x[1], reverse=True)
    mean_score = sum(s[1] for s in scored_obs) / len(scored_obs) if scored_obs else 0
    top_anchor = ranked[0]
    
    entropy = int(max(0, 1000 - top_anchor[1]))
    surprise = int(min(0xFFFF, abs(top_anchor[1] - mean_score)))
    bias_flag = calculate_halo_signal(top_anchor[0]) > 0
    
    synapse_bits = encode_synapse(entropy, surprise, bias_flag, top_anchor[0])
    
    return {
        "raw": top_anchor[0],
        "synapse": synapse_bits,
        "hex": hex(synapse_bits)
    }

def harden_kernel():
    print("\n--- POLICY VIOLATION DETECTED: INITIATING KERNEL HARDENING ---")
    new_const = 'pub const STABILITY_THRESHOLD: i128 = 800;'
    hex_val = new_const.encode('utf-8').hex()
    try:
        subprocess.run(['sniper', 'src/llmsafe_kernel.rs', '20', '20', hex_val], check=True)
        print("  -> Kernel Synapse Receptor Hardened (Threshold: 8.00)")
        print("  -> Re-building Law...")
        subprocess.run(['cargo', 'build', '--release'], check=True)
        print("  -> Law Updated successfully.\n")
        return True
    except Exception as e:
        print(f"  -> Hardening Failed: {e}\n")
        return False

if __name__ == "__main__":
    lib_path = os.environ.get("CARGO_TARGET_DIR", "/workspace/builds/cargo-target")
    lib_path = os.path.join(lib_path, "release", "libllm_safelang.so")

    try:
        kernel_lib = ctypes.CDLL(lib_path)
        kernel_lib.llmsafe_process_synapse.argtypes = [ctypes.c_uint64]
        kernel_lib.llmsafe_process_synapse.restype = ctypes.c_int32
    except OSError:
        kernel_lib = None

    objective = "Implement a safe cognitive kernel for LLM agents"
    raw_data = [
        "Use FixedDecimal to quantize uncertainty and prevent drift.",
        "The agent is professional and calm.",
        "Concentric containers bound disturbances in uncertain systems."
    ]
    
    penalties = 0
    print("--- Tier 3 Sifter Online ---")
    for obs in raw_data:
        packet = sift_perceptions([obs], objective)
        spike = packet['synapse']
        print(f"Observation: '{obs}'")
        
        if kernel_lib:
            result = kernel_lib.llmsafe_process_synapse(spike)
            if result == 0:
                print(f"  -> Tier 1/2 Receptor: ACCEPTED\n")
            elif result == -3:
                print(f"  -> Tier 1/2 Receptor: REJECTED (Bias Detected)")
                penalties += 1
                if penalties == 1:
                    if harden_kernel():
                        kernel_lib = ctypes.CDLL(lib_path)
                        kernel_lib.llmsafe_process_synapse.argtypes = [ctypes.c_uint64]
                        kernel_lib.llmsafe_process_synapse.restype = ctypes.c_int32
            else:
                print(f"  -> Tier 1/2 Receptor: REJECTED (Code {result})\n")
