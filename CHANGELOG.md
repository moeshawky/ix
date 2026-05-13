# Changelog

All notable changes to this project will be documented in this file.

## [0.6.2] - 2026-05-13
### Fixed
- **JSON output backslash escaping** — Fixed critical silent data loss bug where `ix --json` output was malformed for `.jsonl` files and files containing backslash-quote sequences (`\"`). The JSON serializer now properly escapes backslashes before quotes, preventing 50-84% result loss on `.jsonl` files. Added `.replace('\\', '\\\\')` before quote escaping in `print_match()`.
- **Library panic prevention** — Replaced `.expect()` calls in `planner.rs::compile_regex()` with safe fallback to `^$` regex (matches nothing) plus `tracing::warn!` logging. Library code now complies with AGENTS.md requirement: "No unwrap/expect/panic in library code."
- **Silent data corruption prevention** — Five varint decode points in `reader.rs::get_trigram_cdx()` now return `None` and log warnings on error instead of silently using `0`. Corrupted CDX index blocks are now detected and handled gracefully, preventing silent wrong/empty search results.

### Security
- **No silent failures** — All varint decode errors in CDX reader now logged and propagated
- **No panics in library code** — All error paths use proper error handling or safe fallbacks

### Technical Details
- Index format: unchanged (v1.3)
- Backward compatible: yes (no index format change)
- Breaking changes: no
- Migration: none required (existing indexes remain valid)
- All 90 tests pass (71 lib + 19 integration/robustness/streaming)


## [0.6.1] - 2026-05-09

### Fixed
- **Defensive state reset at build start** — Ensure clean state for incremental builds:
  - Clear `postings` HashMap before walking files
  - Clear `temp_runs` vector before merge
  - Reset `file_count` to 0 to prevent ID collisions
  - Reset `BuildStats` to default for accurate metrics
  - Clear `dead_ends` tracking for fresh build
  - Set `committed=false` for clean state

These changes provide defense-in-depth for the clean-before-build pattern,
ensuring incremental builds don't carry over residual state from previous builds.

### Verified
- 5 consecutive incremental builds all correctly index and search files
- No temp file accumulation
- All 86 tests pass

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
