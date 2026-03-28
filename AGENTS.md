# ix — Agent Contract

## What is ix

A Unix-native trigram code search tool. Two binaries:
- `ix` — CLI search tool (like grep, but indexed)
- `ixd` — background daemon that keeps the index fresh

## Design

Read `DESIGN.md` — 3,533 lines, 13 thoughts covering every component from byte-level format to release packaging. It was written with full guardrail screening (G-MEM-1, G-SEC-1, Compound Cascade checks on every thought).

## Architecture

```
src/
  lib/
    lib.rs          — public API
    format.rs       — index file format constants, header struct
    varint.rs       — varint encode/decode (protobuf-style)
    trigram.rs      — trigram extraction (byte-level, not UTF-8)
    builder.rs      — index builder (files -> .ix shard)
    reader.rs       — index reader (mmap, zero-copy)
    bloom.rs        — per-file bloom filters
    posting.rs      — posting list encode/decode (delta + varint)
    planner.rs      — query planner (literal/regex/fullscan)
    executor.rs     — query executor (intersect + verify)
    scanner.rs      — fallback scanner (no index, competitive with ripgrep)
    idle.rs         — idle detection (Linux/macOS/Windows)
    watcher.rs      — file system watcher (notify crate)
    string_pool.rs  — path string pool with prefix dedup
    config.rs       — configuration loading
    error.rs        — error types
  bin/
    ix.rs           — CLI entry point
    ixd.rs          — daemon entry point
tests/
    integration.rs  — end-to-end tests
benches/
    search.rs       — criterion benchmarks
```

## Build & Test

```bash
cargo build --release
cargo test
cargo bench

# Install system-wide
cargo install --path .

# Or manual
cp target/release/ix target/release/ixd /usr/local/bin/
```

## Key Design Decisions

1. **Byte trigrams, not UTF-8**: operates on raw bytes. UTF-8 self-synchronization makes this correct for Unicode for free. No charset detection needed.

2. **256MB fixed trigram table**: 16.7M slots x 16 bytes. Only accessed pages loaded via mmap. Simple indexing: trigram bytes → array index. No hash table.

3. **Varint posting lists**: delta-encoded file IDs + delta-encoded offsets. Compact. Fast to decode.

4. **Per-file bloom filters**: 256 bytes, 5 hashes, 0.7% FPR. Eliminates candidate files before decoding posting lists.

5. **Dormancy-aware indexing**: daemon checks system idle before heavy work. Queues changes during user activity. Drains queue on idle. Like Windows Search / macOS Spotlight.

6. **Staleness handling**: mtime check on search results. Stale files re-read from disk for verification. Always correct, even with outdated index.

7. **Index lives at `.ix/shard.ix`**: in the project root, next to `.git/`. Gitignored.

## Implementation Order

Build in this order — each step is independently testable:

```
1. format.rs + varint.rs + error.rs     — foundation, unit tests
2. trigram.rs                            — extraction, edge case tests
3. bloom.rs                              — bloom filters, FPR verification
4. posting.rs                            — encode/decode posting lists
5. string_pool.rs                        — path dedup
6. builder.rs                            — index building (produces .ix file)
7. reader.rs                             — index reading (mmap, validates CRC)
8. planner.rs                            — query decomposition
9. executor.rs                           — search execution
10. scanner.rs                           — fallback (no index)
11. ix.rs (CLI)                          — user-facing binary
12. config.rs                            — .ixconfig loading
13. idle.rs + watcher.rs                 — daemon support
14. ixd.rs                               — daemon binary
```

Steps 1-10 are the library. Steps 11-14 are the binaries.

## Quality Gates

- Zero `unsafe` except mmap (which is inherently unsafe)
- Zero clippy warnings
- Every offset validated before dereference (no UB on malformed index)
- CRC32C on header and sections
- Criterion benchmarks for: trigram extraction, posting decode, full search
- Integration test: build index, search, verify results match grep

## What This Replaces

`ix` replaces `grep`/`ripgrep` for indexed search. It does NOT replace:
- Tree-sitter parsing (that's codegraph's ingest binary)
- Entity graph (that's codegraph's SQLite)
- Community detection (that's codegraph's Leiden/Louvain)
- Semantic search (that's codegraph's embeddings)

`ix` is the byte-level search layer. Codegraph calls it like it calls ripgrep today — via subprocess.
