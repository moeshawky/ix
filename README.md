# `ix` — Unix-native Trigram Code Search

`ix` is a high-performance, byte-level code search tool designed for the modern developer. It provides indexed search that is significantly faster than standard `grep` or `ripgrep` by leveraging a trigram-based index, while maintaining a Unix-native feel.

## Key Features

- **Trigram-Based Indexing**: Uses 3-byte sequences to build a compact, high-performance index.
- **Byte-Level Search**: Operates on raw bytes, making it inherently compatible with Unicode/UTF-8 without complex charset detection.
- **Unix-Native Integration**: Designed to be used in a pipeline, just like `grep`.
- **Background Daemon (`ixd`)**: (Planned) Keeps your index fresh during system idle time, ensuring searches are always up-to-date.
- **Zero-Copy Performance**: Uses `mmap` for fast, efficient index reading with minimal memory overhead.
- **Bloom Filter Optimization**: Employs per-file bloom filters to skip irrelevant files before decoding posting lists.
- **Graceful Fallback**: If no index is found, `ix` automatically falls back to a high-speed parallel scanner.

## Installation

```bash
# Build from source
cargo build --release

# Install to path
cargo install --path .
```

## Usage

### Building the Index

Before searching, you need to build an index for your project root:

```bash
ix --build
```
This will create a `.ix/shard.ix` file in the current directory.

### Searching

Search for a pattern (literal by default):

```bash
ix "ConnectionTimeout"
```

Search using a Regular Expression:

```bash
ix --regex "err(or|no).*timeout"
```

Force a full scan (ignoring the index):

```bash
ix --no-index "pattern"
```

## Architecture

`ix` is built with a focus on technical integrity and performance:

- **`builder.rs`**: The pipeline for creating the `.ix` index shards.
- **`reader.rs`**: Fast, `mmap`-based interface for querying the index.
- **`executor.rs`**: Orchestrates search execution, combining indexed lookups with content verification.
- **`planner.rs`**: Decomposes complex queries/regex into optimal trigram sets.
- **`bloom.rs`**: High-performance per-file bloom filters.
- **`string_pool.rs`**: Prefix-deduplicated path storage.

## Development

### Prerequisites
- Rust (2024 edition)
- `cargo`

### Running Tests
```bash
cargo test
```

### Benchmarking
```bash
cargo bench
```

## License

MIT - See [LICENSE](LICENSE) for details.
