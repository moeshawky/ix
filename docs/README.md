# Project Documentation Hub

- **Source:** Project root
- **Last Verified:** 2026-11-22 (v0.14.2)
- **Verification:** `cargo test --workspace --all-features && ./scripts/verify-docs.sh`

## Quick Start

### Prerequisites
- Rust 1.85+ (see [rust-toolchain.toml](../rust-toolchain.toml))
- Python 3.10+ & `uv` / `maturin` (optional, for `ix-py`)

### Build
```bash
# Build all workspace Rust targets
cargo build --workspace --all-targets --all-features --exclude ix-py --verbose
```

### Test
```bash
# Core library unit tests
cargo test --lib
# Expected: 222 passed (with --all-features)

# Full workspace suite (unit + integration + property + streaming)
cargo test --workspace --all-features
# Expected: 287 passed; 0 failed; 1 ignored

# Python bindings suite (inside ix-py/.venv)
ix-py/.venv/bin/pytest ix-py/tests/
# Expected: 34 passed
```

### Formatting
```bash
# Check formatting
cargo fmt --check --all

# Check Python formatting & lints
ix-py/.venv/bin/ruff check --config ix-py/pyproject.toml ix-py/
ix-py/.venv/bin/mypy --config-file ix-py/pyproject.toml ix-py/python/ix
```

### Linting
```bash
# Standard workspace lint
cargo clippy --workspace --all-targets --all-features -- -D warnings

# Tier 1 strict CI lints (.github/workflows/tier3.yml:41)
cargo clippy --workspace --all-targets --all-features --exclude ix-py -- \
  -D clippy::dbg_macro \
  -D clippy::todo \
  -D clippy::wildcard_imports \
  -D clippy::panic_in_result_fn \
  -D clippy::missing_const_for_fn \
  -D clippy::large_enum_variant

# Minimal & optional feature matrix checks
cargo check --no-default-features
cargo check --no-default-features --features archive
cargo check --manifest-path ix-py/Cargo.toml --no-default-features
```

### Security
```bash
# Supply chain, license, and ban audit (requires cargo-deny)
cargo deny check licenses advisories bans
```

### Benchmarks
```bash
# Criterion benchmarks
cargo bench
```

## Workspace Layout

### Crate Structure
- `src/lib/` — Core `ix` library (`src/lib/lib.rs` + 27 modules)
- `src/bin/ix/` — CLI binary modules (`main.rs`, `args.rs`, `commands.rs`, `output.rs`, `service.rs`)
- `src/bin/ixd.rs` — Daemon binary (requires `notify` feature on Unix)
- `ix-py/` — Python native extension crate (`_ix`, PyPI package `moeix`, Python module `ix`)
- `fuzz/` — Fuzz targets (`ix-fuzz`)
- `benches/` — Criterion benchmarks (`search`)

### Module Map
```
src/lib/
├── lib.rs — Crate root, lint config, feature-gated exports
├── api.rs — Public API surface
├── archive.rs — Archive support (zip, tar, flate2; optional feature: archive)
├── bloom.rs — Bloom filter
├── builder.rs — Index builder and path admission
├── cache_policy.rs — Adaptive cache policy (ResourceGuard 60% ceiling)
├── config.rs — Search configuration types + .ixd.toml loading
├── daemon.rs — Multi-root daemon loop (cfg(all(feature = "notify", unix)))
├── daemon_sock/ — Unix domain socket IPC (mod, client, resolve, search, server, types)
├── decompress.rs — Decompression (gz/zst/bz2/xz; optional feature: decompress)
├── error.rs — Error types (thiserror)
├── executor.rs — Execution engine
├── file_types.rs — Shared --type alias expansion (CLI + daemon + ix-py)
├── format.rs — Index format constants, header, Beacon
├── idle.rs — Idle tracker for daemon (optional feature: notify)
├── neg_cache.rs — Negative result cache
├── planner.rs — Query planner
├── posting.rs — Posting list encoding/decoding
├── posting_cache.rs — LRU posting list cache
├── reader.rs — Shard & delta reader
├── regex_pool.rs — Compiled regex cache
├── scanner.rs — Fallback linear scanner (.gitignore + .ixignore)
├── streaming.rs — Memory-constant stream verification
├── string_pool.rs — String interning
├── trigram.rs — Trigram extraction
├── varint.rs — Varint encoding/decoding
└── watcher.rs — File watcher (optional feature: notify)
```

### Dependencies (`Cargo.toml:76-108` & `Cargo.lock`)
| Crate | Version | Status |
|-------|---------|--------|
| llmosafe | 0.7.5 (`Cargo.toml`, 0.7.7 locked) | Direct (memory safety) |
| memmap2 | 0.9 | Direct (mmap I/O) |
| crc32c | 0.6 | Direct (checksums) |
| xxhash-rust | 0.8 | Direct (hashing, CDX block index) |
| zstd | 0.13 (0.13.5 locked) | Direct (posting + CDX compression) |
| regex | 1 | Direct (pattern matching) |
| regex-syntax | 0.8 | Direct (query planning) |
| ignore | 0.4 | Direct (scanner, .gitignore) |
| clap | 4 | Direct (CLI args) |
| serde | 1 | Direct (Serialization) |
| serde_json | 1 | Direct (NDJSON socket protocol) |
| toml | 0.8 | Direct (`.ixd.toml` parsing) |
| thiserror | 2 | Direct (Error types) |
| crossbeam-channel | 0.5 | Direct (Concurrency) |
| rayon | 1 | Direct (Parallel verification) |
| notify | 7 | Optional (`notify` feature: watcher, inotify/kqueue) |
| nix | 0.29 (`fs`, `process`, `signal`) | Direct (Unix daemon management) |
| ctrlc | 3.4 | Direct (Daemon signal handling) |
| tracing | 0.1 | Direct (Structured logging) |
| chrono | 0.4 | Direct (Timestamps) |
| flate2 | 1 | Optional (`decompress` / `archive`) |
| bzip2 | 0.5 | Optional (`decompress`) |
| xz2 | 0.1 | Optional (`decompress`) |
| zip | 2 | Optional (`archive`) |
| tar | 0.4 | Optional (`archive`) |

### Feature Flags (`Cargo.toml:69-74`)
- `notify` — File watcher + daemon (`ixd`) + Unix domain socket IPC (default)
- `decompress` — Archive decompression (`flate2`, `bzip2`, `xz2`; optional)
- `archive` — Zip/tar/tar.gz archive indexing (`zip`, `tar`, `flate2`; optional)
- `full` — All optional features (`decompress`, `notify`, `archive`)

## Performance (Measured)

### Latency
- Cold start: <3s
- Hot path p99: <50ms
- Index build: ~2-5s per 100k files

### Memory
- Working set: ~250MB typical
- Peak during build: <8 MB (500K-entry flush threshold), 60% RAM safety ceiling

### Throughput
- Search: ~1000 req/s sustained
- Indexing: ~500 docs/s

## CI/CD

### Build & Test (`/.github/workflows/build.yml`)
- Cross-platform (`ubuntu-latest`, `macos-latest`, `windows-latest`): build, test, minimal feature matrix checks (`--no-default-features`, `--features archive`, `ix-py --no-default-features`), `clippy -D warnings`, `cargo fmt --check`

### Tier-3 Quality Gates (`/.github/workflows/tier3.yml`)
- Strict formatting, Clippy `-D warnings` + feature matrix + Tier-1 Clippy lints (`missing_const_for_fn`, etc.)
- `cargo deny check licenses advisories bans`
- `cargo llvm-cov` (`lcov` upload to Codecov on `main`) + `cargo check --manifest-path fuzz/Cargo.toml` + Miri nightly

### Python Wheels (`/.github/workflows/wheels.yml`)
- Linux (`x86_64`, `aarch64` `manylinux`) and Windows (`x86_64`, `i686`) wheel builds via `PyO3/maturin-action@v1`
- `verify-python`: `.venv`-isolated `ruff`, `mypy`, `maturin develop --release`, and `pytest` gating PyPI upload (`moeix`) on `v*` tags

### Release Binaries (`/.github/workflows/release.yml`)
- `cargo-dist v0.31.0` (`x86_64-unknown-linux-gnu` shell installer + GitHub Release) on `v*` tags

## Contributing

### Adding New Documentation
1. Measure first (run the actual code)
2. Cite source (file:line)
3. Add verification commands
4. Store in this `docs/` directory
5. Run `./scripts/verify-docs.sh`

### Updating Existing Docs
1. Verify current behavior
2. Update measurement
3. Bump `Last Verified` timestamp
4. Run verification script

## Scripts

- `./scripts/verify-docs.sh` — Verify all documentation claims
- `./scripts/detect-doc-drift.sh` — Detect outdated docs
- `./fuzz/fuzz_targets/` — Fuzz targets for parsing robustness

## Release Checklist
- [ ] All Tier 1 lints pass
- [ ] `cargo audit` = 0 vulnerabilities
- [ ] `cargo deny` = passes
- [ ] Coverage report updated
- [ ] Benchmarks recorded
- [ ] Docs verified with `verify-docs.sh`
- [ ] MSRV confirmed (1.85)
