# ix 🔍 

> **High-signal code retrieval for humans and AI agents.**  
> *Sub-millisecond search across multi-gigabyte codebases with constant memory overhead.*

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)]()
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](docs/CONTRIBUTING.md)

---

## ⚡️ The Hero Visual (Proof of Life)

```bash
# Instant search across a massive repo
$ time ix "ConnectionTimeout" src/
[ix] managed by ixd (Status: idle)
src/network/client.rs:42:1280:    pub timeout: ConnectionTimeout,
src/config/defaults.rs:15:450:    pub const DEFAULT_TIMEOUT: ConnectionTimeout = 30s;

real    0m0.004s
user    0m0.002s
sys     0m0.002s

# Streaming compressed logs without OOM
$ ix -z "ERROR" logs/2026-03-29.gz
logs/2026-03-29.gz:1042:0: [ERROR] Database connection failed
```

---

## 🎯 The Hook

`ix` is a high-performance, Unix-native search engine that eliminates the linear scan bottleneck of traditional tools. By leveraging a sparse trigram index and a **constant-memory streaming architecture**, `ix` delivers sub-millisecond retrieval across multi-gigabyte codebases with guaranteed resource safety.

Engineered for the 2026 agentic workflow, it introduces the **Beacon Protocol** to eliminate redundant re-indexing and provide authoritative state for AI agents.

---

## 🚀 Quick Start

### Installation
```bash
cargo install --path . --features full
```

### Build your first index
```bash
ix --build
```
*This creates a compact `.ix/shard.ix` file (typically <10% of source size).*

---

## 💎 Features

- **Universal Search**: Seamlessly query `.rs`, `.py`, `.gz`, `.zst`, `.zip`, and `.tar.gz` files.
- **Beacon Protocol**: Coordination plane between CLI and Daemon. Eliminates "stale index" confusion for AI agents.
- **Streaming Architecture**: Search arbitrarily large files with **constant memory overhead**.
- **Data Integrity**: Per-posting-list **CRC32C checksums** detect silent data corruption.
- **Logical Correctness**: Full support for CRLF (`\r\n`) and LF (`\n`) line endings with accurate byte offsets.
- **Agent-Native**: First-class support for `--json` output and context capping to respect LLM window limits.

---

## 🤖 Agentic Retrieval (LLM Usage)

`ix` follows the **UTCP Schema** for optimal AI agent integration:

| Command | Capability | Agent Output |
| :--- | :--- | :--- |
| `ix -c "p"` | **Existence Check** | Single integer (match count) |
| `ix -l "p"` | **Location** | List of unique file paths |
| `ix -C 3 "p"` | **Contextual Retrieval** | ±3 lines around every match |
| `ix --json "p"` | **Structured Extraction** | JSON Lines for machine parsing |
| `ix -r -U \"foo.*\\nbar\"` | **Multiline** | Cross-line Regex support |

---

## 🛠 Debugging & Development

For contributors and power users:
- **Telemetry**: Use `--stats` to see retrieval performance and index efficiency.
- **Integrity**: `ix` automatically verifies checksums during query execution.
- **Diagnostics**: `cargo test` runs the full suite, including **Negative Oracle** corruption tests and **Resource Boundary** memory tests.

---

## 🏛 Architecture

`ix` employs a tiered verification pipeline:
1. **Query Planner**: Analyzes Regex/Literal and expands trigram variants.
2. **Sparse Lookup**: Intersects posting lists from the trigram table.
3. **Beacon Gate**: Validates authority and coordinates with the background daemon (`ixd`).
4. **Streaming Verification**: Byte-level verification of candidate matches using constant-memory buffers.

---

## 📜 License & Provenance

---

> "Stop scanning. Start finding."
