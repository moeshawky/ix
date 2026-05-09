# ix

[![crates.io](https://img.shields.io/crates/v/moeix.svg)](https://crates.io/crates/moeix)
[![docs.rs](https://docs.rs/moeix/badge.svg)](https://docs.rs/moeix)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![CI](https://github.com/moeshawky/ix/actions/workflows/build.yml/badge.svg)](https://github.com/moeshawky/ix/actions/workflows/build.yml)

Sub-millisecond code search via sparse trigram indexing.

`ix` pre-computes a byte-level trigram index to narrow search candidates to a fraction of the total file set, then verifies matches with a memory-constant streaming architecture. This eliminates the linear-scan bottleneck that slows `grep` and `ripgrep` on large codebases.

## Install

```bash
cargo install moeix
```

This installs two binaries:
- **`ix`** — CLI search tool
- **`ixd`** — background daemon for continuous indexing (requires `notify` feature, enabled by default)

## Quick Start

```bash
# Index a directory (defaults to current directory if no path given)
ix --build
ix --build /path/to/repo

# Search
ix "fn validate"

# Search with regex
ix --regex "fn\s+\w+_handler"

# Search with context lines
ix --context 3 "TODO"

# Negation filter (exclude matches)
ix "error" --negate "test"

# Show query statistics
ix --stats "struct Config"
```

## How It Works

1. **Index** — `ix --build` walks the directory, extracts byte-level trigrams from every file, and writes a compressed index to `.ix/shard.ix`.
2. **Plan** — On search, the query is decomposed into trigrams. The index is consulted to find candidate files that contain all required trigrams.
3. **Verify** — Candidates are streamed through a regex matcher with constant memory usage, producing precise line-level results.

### Index Format (v1.3)

All integers are little-endian, all offsets absolute from file start, 8-byte aligned sections.

| Section | Description |
|---------|-------------|
| Header | 256 bytes: magic `IX01`, version, flags, section offsets |
| File table | Per-file metadata: path hash, content hash, size, posting offset |
| Trigram table (CDX) | Delta-encoded + varint + ZSTD compressed in 1024-entry blocks |
| Block index | `(u32 first_key, u64 block_offset)` × N + sentinel |
| Posting lists | Per-trigram file IDs, delta-encoded + varint + ZSTD |
| String pool | Interned file paths |

CDX compression is always-on since v1.3. The reader does a two-level search: block index → decompress block → linear scan.

**Not backward compatible** with v1.1 or v1.2. Rebuild indexes after upgrading:

```bash
rm -rf .ix/
ix --build .
```

## Daemon

`ixd` watches a directory for file changes and incrementally updates the index:

```bash
ixd /path/to/repo
```

The daemon exposes a Unix domain socket for external consumers (editors, tooling):

```
$XDG_RUNTIME_DIR/ixd/{hash}.sock
```

Protocol is NDJSON — one JSON object per newline-terminated line. Push notifications for file changes and status updates; query/response for history and status queries.

`ix` CLI does **not** use the socket — it reads the index file directly.

## Performance

| Metric | Value |
|--------|-------|
| Index ratio | ~4× source size (ZSTD level 3) |
| Selective query (10% match) | 40ms — scans 10× fewer files than ripgrep |
| Small dataset (all match) | 305ms — ripgrep wins on small/all-match workloads |
| Cold start | <3s |
| Hot path p99 | <50ms |

`ix` wins when the trigram index can eliminate most files from scanning. On small repos or queries where every file matches, linear-scan tools like ripgrep are faster.

## Feature Flags

| Flag | Default | Description |
|------|---------|-------------|
| `notify` | yes | File watcher + daemon (`ixd`) |
| `decompress` | no | gz/zst/bz2/xz decompression |
| `archive` | no | zip/tar archive support |
| `full` | no | All optional features |

## Library

`ix` is also a library (`moeix` on crates.io, `ix` as the crate name):

```toml
[dependencies]
moeix = "0.5"
```

```rust
use ix::{Reader, Executor};

let reader = Reader::open(".ix/shard.ix")?;
let mut executor = Executor::new(&reader);
let matches = executor.execute(/* query */);
```

See [docs.rs/moeix](https://docs.rs/moeix) for full API documentation.

## Building

```bash
cargo build --all-features
cargo test --all-features
cargo clippy --all-features -- -D warnings
```

Requires Rust 1.85+.

## License

MIT

### Clean-Before-Build

The daemon uses a "clean-before-build" pattern to prevent stale file descriptor bugs:

1. Old temp files are cleaned at the start of each build (not at the end)
2. Fresh writers are initialized for each build
3. No temp file accumulation across consecutive builds
4. Prevents inode exhaustion on Linux

This fixes the critical bug where incremental rebuilds failed with "I/O: No such file or directory (os error 2)" after the first successful build.
