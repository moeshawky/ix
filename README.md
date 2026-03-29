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

`ix` is a Unix-native search engine designed for the 2026 developer workflow. While `grep` and `ripgrep` are excellent linear scanners, `ix` bridges the gap to full-text search by leveraging a **sparse trigram index**. 

It is specifically engineered to provide **high-signal retrieval** for AI agents, preventing context flooding by delivering precise code primitives rather than overwhelming chunks of text.

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
- **Parallel Execution**: Multi-threaded verification powered by `rayon`.
- **Streaming Architecture**: Search arbitrarily large files with **constant memory overhead**.
- **Agent-Native**: First-class support for `--json` output and context capping to respect LLM window limits.
- **Daemon Mode**: `ixd` monitors your filesystem and keeps indices fresh while your system is idle.

---

## 🤖 Agentic Retrieval (LLM Usage)

`ix` follows the **UTCP Schema** for optimal AI agent integration:

| Command | Capability | Agent Output |
| :--- | :--- | :--- |
| `ix -c "p"` | **Existence Check** | Single integer (match count) |
| `ix -l "p"` | **Location** | List of unique file paths |
| `ix -C 3 "p"` | **Contextual Retrieval** | ±3 lines around every match |
| `ix --json "p"` | **Structured Extraction** | JSON Lines for machine parsing |
| `ix -r -U "p"` | **Multiline** | Cross-line Regex (dot matches newline) |

---

## 🛠 Debugging & Development

For contributors and power users:
- **Telemetry**: Use `--stats` to see retrieval performance and index efficiency.
- **Diagnostics**: `cargo test --all-features` runs the full integration suite (unit, robustness, and streaming).
- **Binary Bypass**: Use `--binary` to force search through non-text files.

---

## 🏛 Architecture

`ix` employs a tiered verification pipeline:
1. **Query Planner**: Analyzes Regex/Literal and expands trigram variants.
2. **Sparse Lookup**: Intersects posting lists from the trigram table.
3. **Bloom Filter Gate**: Probabilistic skip of false positives before I/O.
4. **SIMD Verification**: Final byte-level verification of candidate matches.

---

## 📜 License & Provenance

- **License**: MIT  
- **Created By**: AI-assisted development (Gemini CLI)  
- **Human Auditor**: @moeshawky  
- **Evolution**: Built with Rust 2024 (and future-proofed for 2026).

---

> "Stop scanning. Start finding."
