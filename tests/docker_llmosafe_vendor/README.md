# LLMOSafe 🛡️

> Active Immune Substrate providing real-time sensing, metabolic awareness, and persistent failure memory for autonomous agents.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)]()

## See It In Action

```c
#include "llmosafe.h"

// 1. Sense the environment
VitalsReport vitals = llmosafe_sense_vitals();
if (vitals.iowait_percent > 15.0) {
    usleep(500000); // Proactive metabolic pacing
}
```

## Quick Start

```bash
# Add to Cargo.toml
[dependencies]
llmosafe = "0.4.0-alpha"
```

## The Contract
LLMOSafe provides three core pillars:
1. **Viral Bitmask Economy:** O(1) authorization checks (e.g., `CAP_ROOT`, `CAP_NETWORK`).
2. **Mycelial Sensing:** Real-time metabolic and capability introspection via `llmosafe_sense_vitals()`.
3. **CRISPR Memory:** `NegativeSelectionLedger` tracks failures and issues Backtrack signals (-7) to prune dead-ends.

## The Engine
Implemented in Rust, offering zero-cost abstractions, thread-safe memory with `once_cell`, and a seamless C-ABI for FFI integrations.

## Context
Designed to fix the "Safety of Lost Effort" loop in agentic systems, shifting from passive guardrails to proactive immune sensing.