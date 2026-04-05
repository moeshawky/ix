# ix ⚡

> Lightning-fast, safety-aware code search and indexing tool.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)]()

## See It In Action

```bash
$ ix search "fn validate"
src/llmosafe_kernel.rs:42: pub fn validate(&self) -> Result<(), KernelError>
```

## Quick Start

```bash
# Install
cargo install --path .

# Index current directory
ix index .

# Search
ix search "Mycelial Sensing"
```

## The Contract

`ix` provides two binaries:
- `ix`: The CLI search tool.
- `ixd`: The background daemon for continuous indexing.

Both are integrated with the LLMOSafe v0.4.0 Immune Substrate for proactive metabolic pacing and capability-aware execution.

## The Engine
`ix` uses a highly optimized trigram index and sparse tables. It relies on `llmosafe` for recursive filesystem traversal safety (`CAP_ROOT` checks) and back-pressure handling (Error -7).

## Context
Built for the era of AI agents where search must be both fast and system-aware.