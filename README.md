# `ix` 🔍

> Accelerates code search for developers by providing a byte-level trigram index for Unix-native pipelines.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](docs/CONTRIBUTING.md)

## Table of Contents
- [About](#about)
- [Getting Started](#getting-started)
- [Usage](#usage)
- [Architecture](#architecture)
- [Contributing](#contributing)
- [License](#license)

## About
`ix` is a high-performance, byte-level code search tool. It provides indexed search that is significantly faster than standard linear scanners by leveraging a trigram-based index, while maintaining a Unix-native feel.

## Getting Started

### Installation
```bash
cargo install --path .
```

### Initializing the Index
```bash
ix --build
```

## Usage
Search for a pattern:
```bash
ix "ConnectionTimeout"
```

Use Regular Expressions:
```bash
ix --regex "err(or|no).*timeout"
```

## Architecture
`ix` uses a byte-level trigram index designed for zero-copy access via `mmap`. It includes per-file Bloom filters to minimize unnecessary I/O.

## Contributing
Contributions are welcome! Please see [CONTRIBUTING.md](docs/CONTRIBUTING.md) for details.

## License
Distributed under the MIT License. See `LICENSE` for more information.

## Acknowledgements
- Inspired by the need for faster-than-grep search in massive codebases.
- Built with Rust 2024.
