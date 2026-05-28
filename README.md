# ix

[![crates.io](https://img.shields.io/crates/v/moeix.svg)](https://crates.io/crates/moeix)
[![docs.rs](https://docs.rs/moeix/badge.svg)](https://docs.rs/moeix)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![CI](https://github.com/moeshawky/ix/actions/workflows/build.yml/badge.svg)](https://github.com/moeshawky/ix/actions/workflows/build.yml)

Sub-millisecond code search via sparse trigram indexing.

`ix` pre-computes a byte-level trigram index to narrow search candidates
to a fraction of the total file set, then verifies matches with a
memory-constant streaming architecture. This eliminates the linear-scan
bottleneck of traditional tools on large codebases.

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

# Start / stop / restart the service
ix service start
ix service stop
ix service restart
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

### Configuring the Daemon

Scope what the daemon watches and indexes with `.ixd.toml`:

```toml
# .ixd.toml
watch_roots = ["src", "lib"]
exclude_patterns = [".git", "node_modules", "target", "vendor"]
```

See [docs/.ixd.toml.md](docs/.ixd.toml.md) for full schema and examples.

## How It Works

1. **Index** — `ix --build` walks the directory, extracts byte-level trigrams
   from every file, and writes a compressed index to `.ix/shard.ix`.
2. **Plan** — On search, the query is decomposed into trigrams. The index
   is consulted to find candidate files.
3. **Verify** — Candidates are streamed through a regex matcher with
   constant memory usage.

### Index Format (v1.3)

All integers little-endian, offsets absolute from file start, 8-byte aligned.

| Section | Description |
|---------|-------------|
| Header | 256 bytes: magic `IX01`, version, flags, section offsets |
| File table | Per-file metadata: path hash, content hash, size |
| Trigram table (CDX) | Delta-encoded + varint + ZSTD in 1024-entry blocks |
| Posting lists | Per-trigram file IDs, delta-encoded + varint + ZSTD |
| String pool | Interned file paths |

CDX compression is always-on since v1.3. Not backward compatible with
v1.1/v1.2 — rebuild indexes after upgrading:

```bash
rm -rf .ix/
ix --build .
```

## Performance

| Metric | Value |
|--------|-------|
| Index ratio | ~4× source size (ZSTD level 3) |
| Selective query (10% match) | 40ms — 10× fewer files than ripgrep |
| Small dataset (all match) | 305ms — ripgrep wins on all-match workloads |
| Cold start | <3s |
| Hot path p99 | <50ms |

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
moeix = "0.9"
```

```rust
use ix::reader::Reader;
use ix::executor::Executor;
use ix::planner::Planner;

let reader = Reader::open(".ix/shard.ix")?;
let plan = Planner::plan("struct Config", false);
let mut executor = Executor::new(&reader);
let (matches, stats) = executor.execute(&plan, &options)?;
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
