# ix

[![crates.io](https://img.shields.io/crates/v/moeix.svg)](https://crates.io/crates/moeix)
[![docs.rs](https://docs.rs/moeix/badge.svg)](https://docs.rs/moeix)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![CI](https://github.com/moeshawky/ix/actions/workflows/build.yml/badge.svg)](https://github.com/moeshawky/ix/actions/workflows/build.yml)

Sub-millisecond code search via sparse trigram indexing.

`ix` builds a compressed trigram index that is typically **2-3× the source
size** for pure code, and can be **smaller than the source** for
repetitive or binary-heavy repos (measured: 0.13× on a 1 GB mixed-content
repo). The compaction pipeline — delta encoding → protobuf varint →
ZSTD level 3 — achieves **88% reduction vs raw u32 storage** and **60%
additional savings on top of varint alone**. The CDX trigram table uses
a B-tree page architecture (block index → ZSTD-compressed 1024-entry
blocks) for **sub-50μs random access** into compressed data.

This eliminates the linear-scan bottleneck of traditional tools on large
codebases. Target hardware floor: 2015 CPU, 8 GB RAM.

## Documentation

| For | Read |
|-----|------|
| Getting started (tutorial) | [docs/QUICKSTART.md](docs/QUICKSTART.md) |
| CLI flag reference | `ix --help` |
| Running the daemon | [docs/DAEMON-RUNBOOK.md](docs/DAEMON-RUNBOOK.md) |
| `.ixd.toml` config | [docs/.ixd.toml.md](docs/.ixd.toml.md) |
| Socket API (tool builders) | [docs/SOCKET-API.md](docs/SOCKET-API.md) |
| Index delta format | [docs/DELTA-FORMAT.md](docs/DELTA-FORMAT.md) |
| Performance benchmarks | [docs/BENCHMARKS.md](docs/BENCHMARKS.md) |
| Contributing | [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md) |
| Release history | [CHANGELOG.md](CHANGELOG.md) |
| Upgrade from v0.7.x | [docs/v0.8.0-UPGRADE-GUIDE.md](docs/v0.8.0-UPGRADE-GUIDE.md) |

## Install

```bash
cargo install moeix
```

Installs two binaries:
- **`ix`** — CLI search tool
- **`ixd`** — background daemon (requires `notify` feature, enabled by default)

You only need `ix` for search. Install `ixd` if you want continuous indexing.

## Quick Start

```bash
# Build the index
ix --build /path/to/repo

# Literal search
ix "fn validate"

# Regex search
ix --regex "fn\s+\w+_handler"

# Context lines around each match
ix --context 3 "TODO"

# Show query statistics
ix --stats "struct Config"

# Only matching file paths
ix --files-only "error"

# Count matches only
ix --count "TODO"

# Filter by file extension
ix --type rs --type py "fn main"
```

## Daemon

`ixd` watches one or more directories for file changes and incrementally
updates the index:

```bash
# Single directory
ixd /path/to/repo

# Multiple directories (v0.9+)
ixd /project-a /project-b /project-c
```

Each directory runs on its own thread with independent index, watcher,
beacon, and Unix domain socket. Signal handling and memory monitoring
are shared.

### Service Management (Linux / systemd)

```bash
# Install as a user-level systemd service
ix service install /path/to/repo

# Start / stop / restart / status the service
ix service start
ix service stop
ix service restart
ix service status
```

The service auto-starts on login and survives reboots. See
[docs/DAEMON-RUNBOOK.md](docs/DAEMON-RUNBOOK.md) for full operation guide.

### Daemon Socket

The daemon exposes a Unix domain socket for external consumers (editors,
tooling):

```
$XDG_RUNTIME_DIR/ixd/{hash}.sock
```

Protocol is NDJSON — one JSON object per newline-terminated line. See
[docs/SOCKET-API.md](docs/SOCKET-API.md). The `ix` CLI reads the index
file directly, not through the socket.

### Index Statistics

Show what's inside an index — file count, trigram count, on-disk size:

```bash
# Human-readable
ix stats /path/to/repo

# JSON output
ix stats --json /path/to/repo
```

This is distinct from `ix --stats "pattern"` (the search flag that shows
per-query statistics). `ix stats` inspects the index itself.

### Configuring the Daemon

Scope what `ixd` watches and `ix --build` indexes with `.ixd.toml`:

```toml
# .ixd.toml
watch_roots = ["src", "lib"]
exclude_patterns = [".git", "node_modules", "target", "vendor"]
```

See [docs/.ixd.toml.md](docs/.ixd.toml.md) for full schema and examples.

## How It Works

1. **Extract** — `ix --build` walks the directory, extracts byte-level
   trigrams (skipping null bytes to nullify binary noise), and caps at
   64 offset samples per trigram for files >1 MB.
2. **Accumulate** — Trigrams are grouped into posting lists (one per
   unique trigram). An external sort with 500K-entry flush threshold
   keeps RAM constant regardless of repository size.
3. **Compress** — Posting lists and the trigram table use the same
   pipeline: delta-encode adjacent file IDs and offsets → protobuf
   varint → ZSTD level 3. The CDX trigram table is organized as a
   B-tree: a 12-byte-per-1024-entry block index for O(log N) lookup,
   then decompress one ~5 KB block to find the target.
4. **Plan** — On search, the query is decomposed into trigrams. The
   block index finds the target block, one ZSTD call decompresses it,
   and a linear scan finds the posting list offset.
5. **Verify** — Candidates are filtered through per-file bloom filters
   (256 B, 0.7% false-positive rate), then streamed through a regex
   matcher with constant memory usage.

### Compaction Pipeline (measured)

```
Raw u32 entries  →  delta-encode  →  varint  →  ZSTD level 3
  10.6 MB             2-3× smaller     60% more     88% total reduction
                                                      (1.3 MB final)
```

| Stage | What it catches | Typical savings |
|-------|----------------|-----------------|
| Null-byte skip | Binary files (30-80% null bytes) | near-zero trigram cost |
| Offset sampling | Repeated patterns in large files | 64 offsets max per trigram |
| Delta encoding | Sequential file IDs, clustered offsets | 2-3× vs raw u32 |
| Protobuf varint | Small values fit in 1 byte (<128) | dense trigrams stay compact |
| ZSTD level 3 | Byte-pattern redundancy in varint runs | **60%** on top of varint |


### Index Format (v1.3)

All integers little-endian, offsets absolute from file start, 8-byte aligned.

| Section | Size (example, 70 files) | Description |
|---------|---------------------------|-------------|
| Header | 256 B | magic `IX01`, version, flags, CRC, section offsets |
| File table | 3.4 KB | 48 B per file: path offset, content hash, size, mtime |
| Posting lists | 1,332 KB (90.1%) | Per-trigram file entries: delta+varint+ZSTD |
| CDX trigram table | 122 KB (8.3%) | 4.9 B/trigram (75% vs naive 20 B) |
| CDX block index | 312 B (0.02%) | 12 B per 1024-entry block, O(log N) binary search |
| Bloom filters | 18 KB (1.2%) | 256 B per file, 5 hashes, 0.7% FPR |
| String pool | 1.9 KB | Interned file paths |

CDX compression is always-on since v1.3. Not backward compatible with
v1.1/v1.2 — rebuild indexes after upgrading:

```bash
rm -rf .ix/
ix --build .
```

## Performance

Measured on a 2015-era CPU (Haswell equivalent), 8 GB RAM. All ratios
verified from actual indexes.

| Workload | Source | Index | Ratio |
|----------|--------|-------|-------|
| Source code (70 files) | 576 KB | 1,477 KB | **2.56×** |
| Mixed-content repo (426 files) | 1,069 MB | 138 MB | **0.13×** |

| Metric | Value | Notes |
|--------|-------|-------|
| Posting data vs raw u32 | **88% reduction** | 10.6 MB → 1.3 MB |
| ZSTD on varint buffer | **60% savings** | varint 3.3 MB → zstd 1.3 MB |
| CDX trigram table vs naive | **75% smaller** | 4.9 B vs 20 B per entry |
| Block index overhead | **0.02%** of index | 12 B per 1024 trigrams |
| CDX lookup latency | **<50 μs** | block index search + 1 ZSTD call |
| Build RAM peak | **<8 MB** | HashMap flushes at 500K entries |
| Safety ceiling | **60% RAM** | ResourceGuard (llmosafe), 80% fallback |
| Cold start | <3 s | From disk to first result |
| Selective query (10% match) | 40 ms | 10× fewer files than ripgrep |

`ix` wins when the trigram index eliminates most files from scanning.
On small repos or queries where every file matches, linear-scan tools
like ripgrep are faster.

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
moeix = "0.12"
```

```rust
use ix::reader::Reader;
use ix::executor::{Executor, QueryOptions};
use ix::planner::Planner;

let reader = Reader::open(".ix/shard.ix")?;
let plan = Planner::plan("struct Config", false);
let mut executor = Executor::new(&reader);
let (matches, stats) = executor.execute(&plan, &QueryOptions::default())?;
```

See [docs.rs/moeix](https://docs.rs/moeix) for the full API reference.

## Building

```bash
cargo build --all-features
cargo test --all-features
cargo clippy --all-features -- -D warnings
```

Requires Rust 1.85+.

## License

MIT
