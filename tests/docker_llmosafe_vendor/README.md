# llmosafe (v0.3.0 Behavioral Substrate)

[![Crates.io](https://img.shields.io/crates/v/llmosafe.svg)](https://crates.io/crates/llmosafe)
[![Documentation](https://docs.rs/llmosafe/badge.svg)](https://docs.rs/llmosafe)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A 3-tier safety-critical library for AI agents, providing formal primitives for entropy tracking, surprise gating, and perceptual sifting.

`llmosafe` implements a deterministic safety kernel designed to bound the reasoning processes of LLM-based agents, ensuring cognitive stability and preventing hallucinations through formal invariants.

## v0.3.0 Behavioral Substrate

The v0.3.0 update introduces the **Behavioral Substrate**, codifying the relationship between cognitive entropy and physical resources.

### Metabolic Governor (The Metabolic Law)
The kernel now enforces the **Metabolic Law**: AI reasoning is no longer unbounded in time. The `MetabolicGovernor` enforces strict pacing (e.g., 100ms minimum interval) between reasoning steps to prevent "Cognitive Thermal Runaway" and ensure observability.

### Capability Ledger
The `Capability Ledger` provides fine-grained authorization for kernel actions. Agents must explicitly possess privileges (Sandbox, Network, Root) to perform restricted operations, preventing unauthorized escalation during reasoning loops.

## Architecture

```text
┌───────────────────────────────────────────────────────────┐
│                    PERCEPTUAL SIFTER (Tier 3)             │
│  (Sifts raw observations into high-signal Synapse spikes) │
└───────────────┬───────────────────────────────────────────┘
                │
                ▼
┌───────────────────────────────────────────────────────────┐
│                COGNITIVE WORKING MEMORY (Tier 2)          │
│  (Fixed-size state with surprise-based update gating)     │
│  (Capability Ledger enforcement)                          │
└───────────────┬───────────────────────────────────────────┘
                │
                ▼
┌───────────────────────────────────────────────────────────┐
│                    DETERMINISTIC KERNEL (Tier 1)          │
│  (Hard stability gates and reasoning loop depth bounds)   │
│  (Metabolic Governor pacing enforcement)                  │
└───────────────────────────────────────────────────────────┘
```

## Features

- **Tier 1 (Kernel):** Cognitive Entropy tracking, Reasoning Loop depth enforcement, and **Metabolic Pacing**.
- **Tier 2 (Memory):** Fixed-size Working Memory with surprise-gated state transitions and **Capability-based Authorization**.
- **Tier 3 (Sifter):** Halo/Bias signal detection and utility-based perception ranking.
- **C-ABI:** Direct integration with C, Python (ctypes), and other languages via stable binary interface.

## Usage

### C Integration (FFI)

The library generates a C header `include/llmosafe.h`. v0.3.0 introduces new entry points for pacing and authorization.

```c
#include "llmosafe.h"

// Enforce Metabolic Law (100ms pacing)
if (llmosafe_metabolic_pace(100) < 0) {
    // Pacing violation!
}

// Check Capability Ledger
if (llmosafe_authorize_action(1 /* ExternalApiCall */) < 0) {
    // Unauthorized action!
}

uint64_t synapse_bits = ...;
int32_t result = llmosafe_process_synapse(synapse_bits);
```

## Safety Invariants

- **Metabolic Law:** Mandatory pacing to ensure system-wide stability.
- **Capability Isolation:** Actions are restricted by the Capability Ledger.
- **Deterministic Arithmetic:** Uses fixed-precision entropy tracking.
- **Memory Safety:** `no_alloc` compatible core (Tier 1/2) ensures predictable resource usage.
