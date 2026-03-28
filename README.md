# `ix` 🔍

> High-performance, byte-level code search using a sparse trigram index. Optimized for developers and LLM agents.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](docs/CONTRIBUTING.md)
[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)]()

## Table of Contents
- [About](#about)
- [Key Features](#key-features)
- [Getting Started](#getting-started)
- [Usage](#usage)
- [LLM Agent Usage](#llm-agent-usage)
- [Architecture](#architecture)
- [Contributing](#contributing)
- [License](#license)

## About
`ix` is a Unix-native code search tool that bridges the gap between linear scanners like `grep` and full-text search engines. By leveraging a **sparse trigram index**, `ix` delivers sub-millisecond search latency on multi-gigabyte codebases while maintaining a minimal disk footprint.

## Key Features
- **Sparse Trigram Index**: Only stores non-empty slots, reducing index size by up to 80% compared to fixed-size tables.
- **LLM-Native**: Specialized flags (`--json`, `--context`, `--max-results`) designed for programmatic consumption by AI agents.
- **Zero-Copy Performance**: Utilizes `mmap` for instant index loading and high-speed querying.
- **Robustness**: Safe UTF-8 handling and binary file detection prevent crashes in complex environments.
- **Unix-Native**: Fits perfectly into CLI pipelines with standard output formatting.

## Getting Started

### Installation

#### From Source
```bash
cargo install --path .
```

#### Pre-built Binaries (Linux & macOS)
```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/moeshawky/ix/releases/latest/download/ix-installer.sh | sh
```

#### Pre-built Binaries (Windows)
```powershell
powershell -c "irm https://github.com/moeshawky/ix/releases/latest/download/ix-installer.ps1 | iex"
```

### Initializing the Index
Build the index once for your project root:
```bash
ix --build
```
*This creates a compact `.ix/shard.ix` file (typically <10% of source size).*

## Usage

### Basic Search
```bash
ix "ConnectionTimeout"
```

### Regex Search
```bash
ix --regex "err(or|no).*timeout"
```

### Scoped Search (by file type)
```bash
ix -t rs -t py "fn main"
```

## LLM Agent Usage
`ix` is designed to be the primary search tool for AI agents (Claude Code, OpenCode, etc.) to prevent context flooding and ensure high-precision retrieval.

| Pattern | Command | Output |
|:---|:---|:---|
| **Existence Check** | `ix -c "pattern"` | Single integer (count) |
| **Location** | `ix -l "pattern"` | List of unique file paths |
| **Context** | `ix -C 3 "pattern"` | ±3 lines around matches |
| **Safe Default** | `ix "pattern"` | Max 100 results (prevents flooding) |
| **Machine Read** | `ix --json "pattern"` | JSON Lines format |

## Architecture
`ix` uses a byte-level trigram index addressing a sorted array of posting lists.
1. **Query Planning**: Decomposes patterns into required trigram sets.
2. **Sparse Lookup**: Binary searches the trigram table (`O(log n)`).
3. **Bloom Filtering**: Per-file filters eliminate 99%+ of false positive candidates.
4. **Verification**: SIMD-accelerated content verification on final candidates.

## Contributing
Contributions are welcome! Please see [CONTRIBUTING.md](docs/CONTRIBUTING.md) for details.

## License
Distributed under the MIT License. See `LICENSE` for more information.

## Acknowledgements
- Inspired by the need for faster-than-grep search in massive codebases.
- Built with Rust 2024.
