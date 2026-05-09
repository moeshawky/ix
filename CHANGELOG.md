## [0.6.0] - 2026-05-09

### Added
- **Clean-before-build pattern** — Fixed critical stale file descriptor bug where incremental rebuilds failed with "I/O: No such file or directory (os error 2)" after first successful build.
  - `Builder` struct fields (`files_writer`, `blooms_writer`, `strings_writer`) now wrapped in `Option<BufWriter<File>>`
  - New `init_writers()` method creates fresh temp files at build start
  - New `cleanup_old_temp_files()` method releases old temp files before initialization
  - Temp files are build artifacts, not struct state — created fresh each build, cleaned at next build start
  - Prevents inode exhaustion on Linux where deleted files with open FDs keep inodes alive
- **CLI ergonomics** — `ix --build` without path argument now defaults to current working directory
  - Changed from `default_value = "."` to `num_args = 0..=1, default_missing_value = "."`
  - All three modes work: `ix --build`, `ix --build /path`, `ix "pattern"`
- **Enterprise-grade error handling** — Zero `.unwrap()` or `.expect()` in library code
  - Added `get_writer()` helper with `.ok_or_else(|| Error::Io(...))` pattern
  - All 19 writer accesses now return proper errors instead of panicking
  - Error messages include context ("files_writer not initialized", etc.)

### Fixed
- **Stale FD bug** — `serialize()` no longer deletes temp files mid-lifecycle; cleanup happens at next build start
- **Disk accumulation** — No temp file accumulation across consecutive builds
- **Daemon incremental rebuild** — `ixd` can now perform multiple consecutive rebuilds without failure

### Changed
- `Builder::new()` no longer creates temp files — initialization deferred to `build()`
- `Builder::build()` now calls `cleanup_old_temp_files()` + `init_writers()` at start
- Temp file deletion responsibility moved from `serialize()` to `cleanup_old_temp_files()`

### Technical Details
- Index format: unchanged (v1.3)
- Backward compatible: yes (no index format change)
- Breaking changes: no
- Migration: none required (existing indexes remain valid)

### Security
- No `.unwrap()` in library code — all errors properly propagated
- Clean-before-build prevents inode exhaustion attacks via repeated builds

# Changelog

All notable changes to this project will be documented in this file.

## [0.5.4] - 2026-05-03

### Added
- **Multi-directory daemon support**: `ix --daemon dir1 dir2 dir3` now watches multiple directories simultaneously.

### Fixed
- **Format violations**: All code now passes `cargo fmt --check`.
- **Ctrl-C handler error handling**: Replaced `expect()` with proper error handling in daemon.
- **Dependency advisory**: Added exception for RUSTSEC-2024-0375 (transitive via cbindgen build dependency).

## [0.5.3] - 2026-05-03

### Added
- **DaemonStatus typed enum** — Replaces raw status strings with structured typed state (`Idle`, `Indexing`, `Deferred`, `Escalated`, `Warned`, `SafetyHalt`, `SafetyExit`). Wire format backward compatible — legacy `status` field preserved as plain string.
- **daemon_status field** — New optional field in `QueryResult` and `Status` messages carrying structured typed state (`{"state":"indexing","entropy":42}`).
- **last_rebuild_at** — New optional `u64` field in `QueryResult` response — Unix timestamp of the last successful rebuild completion.

### Fixed
- **StatusQuery id echo back** — StatusQuery now echoes back the client-provided `id` instead of returning the daemon PID.
- **SafetyDecision cooldowns** — All `Halt` and `Escalate` cooldowns now non-zero via llmosafe 0.5.5 dependency.
- **Disk space check moved** — Now performed at start of `build()` before any temp file I/O.
- **Builder Drop impl** — Cleans up orphaned temp files on build failure.

### Changed
- `ClientMessage::StatusQuery` now accepts an optional `id: u64` field (serde default) for correlation.

## [0.5.2] - 2026-04-29

### Fixed
- **Symlink attack prevention on daemon socket bind** — `DaemonServer::new()` now checks if a file or symlink exists at the socket path before binding. Rejects with a clear error message instead of silently removing stale sockets (which could be exploited in a symlink attack).
- **Client receive timeout** — `DaemonClient::connect()` now sets a 5-second read timeout. `recv()` returns a `TimedOut` error instead of blocking indefinitely if the server becomes unresponsive.
- **More specific error types** — `recv()` now returns distinct error kinds for timeout (`TimedOut`), clean disconnect (`UnexpectedEof`), and malformed JSON (`InvalidData`).

### Removed
- Stale socket cleanup on server start — This was a security risk (symlink race). Users must manually remove the socket file if restarting the daemon.

## [0.5.1] - 2026-04-29

### Fixed
- **CRITICAL**: `PostingList::encode()` no longer falls back to raw data on ZSTD failure — the reader always calls `zstd::decode_all`, so a raw fallback caused silent decode failures. Now returns `Result<Vec<u8>>` and propagates the error.
- **CRITICAL**: Removed all `expect()` calls from library code (`daemon_sock.rs` 7 instances, `reader.rs` 2 instances). Replaced with proper error propagation (`Result`) or conservative fallbacks (bloom filter returns `true` on corrupt data).
- **CRITICAL**: Removed `unreachable!()` from `builder.rs` merge loop — replaced with `ok_or_else(Error::Config)` for proper error propagation.
- CDX block decompression errors now logged via `tracing::warn` instead of silently returning `None`.
- `DaemonServer::start()` now returns `Result<()>` instead of panicking on resource exhaustion (EMFILE, thread spawn failure).
- `broadcast()` now serializes the message once and uses per-client write timeouts (5s) to prevent a slow consumer from blocking the entire daemon socket subsystem.
- All clippy pedantic warnings fixed.

## [0.5.0] - 2026-04-29

### Added
- **CDX (Concentrated Delta X) compression** for the trigram table — always-on since format v1.3
  - Trigram table is delta-encoded + varint + ZSTD compressed in 1024-entry blocks
  - Block index: `(u32 first_key, u64 block_offset)` × N + sentinel `(u32::MAX, end_offset)`
  - Reader does two-level search: block index → decompress block → linear scan
  - `HAS_CDX_INDEX` flag (bit 4) in index header
- **Cache layer** for daemon and repeated-query performance:
  - `PostingCache`: LRU cache with 64MB ceiling, keyed by `Trigram`
  - `NegCache`: HashSet negative-result cache, keyed by `(query_fingerprint, content_hash)`
  - `RegexPool`: compiled regex cache with FIFO eviction
  - `AdaptiveCachePolicy`: read-only policy object reading `ResourceGuard::pressure()`, producing `CacheDirective`s
- Cache modules exported as public API: `posting_cache`, `neg_cache`, `regex_pool`, `cache_policy`
- `Executor::new_with_caches()` for daemon mode with shared `Arc<PostingCache/NegCache/RegexPool>`
- `QueryStats` now includes `posting_cache_hits/misses` and `neg_cache_hits/misses`
- `QueryPlan::pattern_str()` method for neg cache fingerprinting
- CLI `--stats` now reports cache hit/miss ratios
- **Daemon socket IPC** (`daemon_sock` module, feature-gated `notify`):
  - `DaemonServer`: binds Unix domain socket, accepts connections, broadcasts to all clients
  - `DaemonClient`: connects, receives NDJSON push notifications, sends queries
  - `ServerMessage` enum: `Status`, `FilesChanged`, `QueryResult`
  - `ClientMessage` enum: `StatusQuery`, `HistoryQuery`
  - History buffer: `VecDeque` capped at 1024 batches
  - Socket path: `$XDG_RUNTIME_DIR/ixd/{hash}.sock` (fallback `~/.local/run/ixd/`, last resort `/tmp/ixd-{uid}-{hash}.sock`)
  - Hash = first 16 hex chars of `XXH64(canonical_root, 0)`
- `Beacon` struct: added `socket_path: Option<PathBuf>` with `#[serde(default)]` for backward compatibility
- `ixd` binary: creates `DaemonServer` on startup, broadcasts status and file changes
- `IdleTracker` module for daemon idle-state detection

### Changed
- **Index format v1.3** — NOT backward compatible with v1.2. Rebuild indexes after upgrade.
- `Executor::execute()` is now `&mut self` — sets `neg_query_fingerprint` before dispatching
- All clippy pedantic warnings fixed across library and binaries (32 warnings)
- `write_cdx_blocks` zstd encode now propagates errors instead of falling back to raw data

### Fixed
- Removed `align_to_8` from inside `write_cdx_blocks` — ZSTD frames are self-delimiting, trailing padding caused "Unknown frame descriptor" decode failures
- CDX zstd fallback contract mismatch: `unwrap_or(buf)` replaced with `map_err(|e| Error::Config(...))?` to prevent silent trigram loss

### Removed
- Legacy uncompressed trigram table path removed (CDX is always-on)

## [0.3.3] - 2026-04-15

### Fixed
- **CRITICAL**: Fixed stale beacon detection that caused false "managed by ixd" errors due to PID reuse
- `is_live()` now verifies process identity via `/proc/{pid}/comm` before declaring beacon valid
- Prevents orphan `.ix.run.*` file accumulation when ixd crashes and PID is reused

### Technical Details
- `is_live()` was checking only PID existence, not process identity
- Linux reuses PIDs, so a crashed ixd's PID could be assigned to an unrelated process
- This caused `ix` to incorrectly believe ixd was still running
- Now reads `/proc/{pid}/comm` and verifies it contains "ixd"

## [0.3.2] - 2026-04-14

### Fixed
- **CRITICAL**: Removed LLMOSafe cognitive filter that silently skipped ~10 source files during indexing
- Missing files included `src/bin/ix.rs`, `src/bin/ixd.rs`, and several `src/lib/*.rs` files
- Search results are now complete - all source files are properly indexed

### Technical Details
- Removed `cognitive_memory` field from Builder struct
- Removed `sift_perceptions` content filtering during index build
- LLMOSafe is still used for ResourceGuard (memory safety) and ixd daemon safety decisions
- Index now includes ALL non-binary, non-oversized source files

## [0.3.1] - 2026-04-14

### Fixed
- **CRITICAL**: Removed broken auto-stdin detection that caused empty results when running in non-TTY environments (bash tools, pi, CI)
- Root cause: Code incorrectly assumed stdin search when stdin was not a terminal
- Now requires explicit `(stdin)` path argument for stdin search (follows Unix convention)

### Technical Details
- Removed `is_stdin_pipe` detection logic in `ix.rs`
- Removed unused `IsTerminal` import
- Fixed benchmark build error in `benches/search.rs`

## [0.3.0] - 2026-04-14

### Changed
- **BREAKING**: Posting lists now use ZSTD compression (format v1.2)
  - Index size reduced by 75% (676 MB → 170 MB on test corpus)
  - Query latency remains negligible (<100ms)
  - CRC32C replaced with ZSTD's built-in XXHash64 checksum
- `zstd` is now a required dependency (not optional)

### Technical Details
- `posting.rs`: Added ZSTD compression level 3 after delta+varint encoding
- `format.rs`: VERSION_MINOR 1 → 2 (format v1.2)
- Index ratio improved from ~15x to ~4x source size

### Migration
**Important**: Index format v1.2 is NOT backward compatible with v1.1.
After upgrading, rebuild your indexes:
```bash
rm -rf .ix/
ix --build .
```

## [0.2.8] - 2026-04-01

### Fixed
- Error logging in builder
- Backup mechanism for index files
- chrono dependency for timestamps
- Grace period handling
- Type fixes for error handling
