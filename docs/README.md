# Project Documentation Hub

**Source:** Project root  
**Last Verified:** 2026-04-29  
**Verification:** `./scripts/verify-docs.sh`

## Quick Start

### Prerequisites
- Rust 1.85+ (see [rust-toolchain](../rust-toolchain))
- Docker + Docker Compose

### Build
```bash
# Build all targets (includes fuzz targets, benchmarks)
cargo build --all-targets --all-features --verbose
# Expected: Completes in ~30s on M1/modern laptop
```

### Test
```bash
# All tests (71 unit tests)
cargo test --lib
# Expected: 71 passed
```

### Formatting
```bash
# Check formatting
cargo fmt -- --check
```

### Linting
```bash
# Tier 1: Critical lints (fail CI)
cargo clippy --all-targets --all-features \
  -D clippy::as_conversions \
  -D clippy::unwrap_used \
  -D clippy::indexing_slicing \
  -D clippy::expect_used \
  -D clippy::panic_in_result_fn \
  -D clippy::missing_const_for_fn \
  -D clippy::large_enum_variant

# Tier 2: Production hygiene
cargo clippy --all-targets --all-features \
  -D clippy::dbg_macro \
  -D clippy::todo \
  -D clippy::wildcard_imports

# Full lint
cargo clippy --all-targets --all-features -D warnings
```

### Security
```bash
# Supply chain audit (requires cargo-audit)
cargo audit

# License compliance (requires cargo-deny)
cargo deny check licenses advisories bans
```

### Coverage
```bash
# Generate coverage report (requires cargo-llvm-cov)
cargo llvm-cov --all-features --workspace --lcov --output-path lcov.info
cargo llvm-cov --all-features --workspace --summary-only
# Expected coverage: 47.14% total (see lcov.info for detail)
```

### Benchmarks
```bash
# Criterion benchmarks (requires nightly for some features)
cargo bench
```

## Architecture

### Crate Structure
- `src/lib/` — Core library (lib.rs + modules)
- `src/bin/ix` — CLI binary
- `src/bin/ixd.rs` — Daemon binary (optional feature: notify)
- `fuzz/` — Fuzz targets
- `benches/` — Criterion benchmarks

### Module Map
```
src/lib/
├── lib.rs — Crate root, lint config
├── api.rs — Public API surface
├── builder.rs — Index builder
├── reader.rs — Shard reader
├── executor.rs — Execution engine
├── planner.rs — Query planner
├── scanner.rs — File scanner (optional: ignore)
├── format.rs — Index format constants, header, Beacon
├── trigram.rs — Trigram extraction
├── varint.rs — Varint encoding/decoding
├── bloom.rs — Bloom filter
├── posting.rs — Posting list encoding/decoding
├── string_pool.rs — String interning
├── streaming.rs — Memory-constant stream verification
├── config.rs — Search configuration types
├── error.rs — Error types (thiserror)
├── archive.rs — Archive support (zip, tar; optional)
├── decompress.rs — Decompression (gz/zst/bz2/xz; optional)
├── daemon_sock.rs — Unix domain socket IPC (optional: notify)
├── watcher.rs — File watcher (optional: notify)
├── idle.rs — Idle tracker for daemon (optional: notify)
├── cache_policy.rs — Adaptive cache policy
├── posting_cache.rs — LRU posting list cache
├── neg_cache.rs — Negative result cache
└── regex_pool.rs — Compiled regex cache
```

### Dependencies
| Crate | Version | Status |
|-------|---------|--------|
| llmosafe | 0.5 | Direct (memory safety) |
| memmap2 | 0.9 | Direct (mmap I/O) |
| crc32c | 0.6 | Direct (checksums) |
| xxhash-rust | 0.8 | Direct (hashing, CDX block index) |
| zstd | 0.13 | Direct (posting + CDX compression) |
| regex | 1 | Direct (pattern matching) |
| regex-syntax | 0.8 | Direct (query planning) |
| ignore | 0.4 | Optional (scanner, .gitignore) |
| notify | 7 | Optional (watcher, inotify/kqueue) |
| clap | 4 | CLI args |
| serde | 1 | Serialization |
| serde_json | 1 | NDJSON socket protocol |
| thiserror | 2 | Error types |
| crossbeam-channel | 0.5 | Concurrency |
| rayon | 1 | Parallel verification |
| nix | 0.29 | Daemon process management |
| ctrlc | 3.4 | Daemon signal handling |
| tracing | 0.1 | Structured logging |
| chrono | 0.4 | Timestamps |
| flate2 | 1 | Optional (gz decompression) |
| bzip2 | 0.5 | Optional (bz2 decompression) |
| xz2 | 0.1 | Optional (xz decompression) |
| zip | 2 | Optional (zip archive) |
| tar | 0.4 | Optional (tar archive) |

### Feature Flags
- `notify` — File watcher support (default)
- `decompress` — Archive decompression (optional)
- `archive` — Zip/tar support (optional)
- `full` — All optional features

## Performance (Measured)

### Latency
- Cold start: <3s
- Hot path p99: <50ms
- Index build: ~2-5s per 100k files

### Memory
- Working set: ~250MB typical
- Peak during build: ~1GB

### Throughput
- Search: ~1000 req/s sustained
- Indexing: ~500 docs/s

## CI/CD

### GitHub Actions (`/.github/workflows/build.yml`)
```yaml
jobs:
  lint:
    # Tier 1 + Tier 2 checks
  build:
    # Cross-platform: ubuntu, macos, windows
  test:
    # All features, all targets
```

### Coverage CI (`/.github/workflows/tier3.yml`)
- `cargo llvm-cov` (lcov output)
- `codecov` upload on main
- Fuzz target integration
- Miri nightly (UB detection)

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
