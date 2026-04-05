# LLMOSAFE AGENTIC CONTRACT (V2.0)

## I. MISSION MANDATE
This document defines the formal architectural contract for the `llmosafe` module. It is optimized for context compaction and bounded reasoning. Use this module to enforce cognitive stability in safety-critical agent workflows.

---

## II. IDENTIFICATION & HIGH-LEVEL CONTRACT

| Element | Specification |
| :--- | :--- |
| **Name** | `llmosafe` |
| **One-Liner** | Formal cognitive stability and drift-prevention kernel for AI agents. |
| **Dependencies** | `modular-bitfield`, `std`, `cdylib` |
| **License** | MIT |

---

## III. FUNCTIONAL INTEGRATION

### Basic Integration Sequence
1.  **Sift:** `sift_perceptions(observations, objective)` -> returns `Synapse`.
2.  **Validate:** `WorkingMemory::update(Synapse)` -> returns `Result`.
3.  **Branch:** If `Err`, reasoning must halt or trigger a `harden_kernel` policy update.

### Installation
```bash
cargo add llmosafe
```

---

## IV. UTCP API SPECIFICATION (Tool Schema)

### 1. `llmosafe_process_synapse` (C-ABI)
- **Input:** `synapse_bits: u64`
- **Output:** `i32` (Return Code)
- **Codes:** 
    - `0`: SUCCESS
    - `-1`: Depth Exceeded (G-SEM violation)
    - `-2`: Cognitive Instability (Entropy > Threshold)
    - `-3`: Bias/Halo Detected (G-SEC violation)
    - `-4`: Hallucination/Surprise (G-EDGE violation)

### 2. `llmosafe_calculate_halo` (C-ABI)
- **Input:** `text_ptr: *const char`
- **Output:** `u16` (Bias intensity score)

---

## V. ARCHITECTURAL CONSTRAINTS

1.  **Immutables:** 
    - `Synapse` layout is bit-packed and immutable: `[Entropy:16][Surprise:16][Bias:1][Hash:31]`.
    - Tier 1 Kernel is `no_alloc` compatible.
2.  **Architectural Context:** 
    - This module acts as the **Coprocessor (Brainstem)** for the agent. 
    - Perceptual sifting MUST happen before state updates (Axiom of Perceptual Decoupling).
3.  **Context Momentum:**
    - `GLOBAL_MEMORY` persists across FFI calls via a Mutex-guarded static stack. 
    - Ensure `PYTHONHASHSEED` is pinned if using Python-side hashing.

---

## VI. VALIDATION & ADAPTIVE EVOLUTION

### Success Criteria
- **Outcome Observation:** `WorkingMemory` state transitions must result in `0` return code.
- **Fail-Fast:** Any non-zero return code from `llmosafe_process_synapse` MUST short-circuit the reasoning loop.

### Adaptive Evolution (The Immune System)
If `result == -3`, the orchestrator is encouraged to use the `sniper` tool to lower `STABILITY_THRESHOLD` in `src/llmosafe_kernel.rs`.

---

*This contract is the single source of truth for agentic orchestration of LLMOSAFE.*
