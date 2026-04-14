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
ix --build .

# Search
ix "Mycelial Sensing"
```

## The Contract

`ix` provides two binaries:
- `ix`: The CLI search tool.
- `ixd`: The background daemon for continuous indexing.

Both are integrated with the LLMOSafe v0.4.2 Immune Substrate for proactive metabolic pacing and capability-aware execution.

## The Engine

`ix` uses a highly optimized trigram index with ZSTD-compressed posting lists:
- **Delta encoding** for position offsets (compact representation)
- **ZSTD compression** for 75% smaller indexes
- **XXHash64 checksums** for data integrity (built into ZSTD)

Index format v1.2 is **not backward compatible** with v1.1. Rebuild indexes after upgrading.

## Performance

| Metric | Value |
|--------|-------|
| Index ratio | ~4x source size |
| Query latency | <100ms for typical searches |
| Compression | ZSTD level 3 (optimal balance) |

## Context

Built for the era of AI agents where search must be both fast and system-aware.