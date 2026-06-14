# Changelog

All notable changes to this project will be documented in this file.

## [0.12.6] - 2026-06-14

### Changed
- Upgraded llmosafe from 0.7.4 (yanked) to 0.7.5: 6 getter functions now use
  out-parameter pattern (`int32_t` return + `uint16_t*` out) instead of direct
  return values. Adds `LLMOSafeError` exception for Python bindings.

## [0.12.5] - 2026-06-11

### Changed
- Daemon event loop uses 30×1s timeout sub-iterations instead of single 30s timeout,
  so shutdown signals are detected within ~1s (was up to 30s)
- `daemon_sock.rs` (1733 lines) decomposed into 6 domain modules under
  `src/lib/daemon_sock/`: `types`, `resolve`, `server`, `client`, `search`
- `ix.rs` binary (1541 lines) decomposed into 5 modules under `src/bin/ix/`:
  `args`, `output`, `commands`, `service`
  (entry point renamed `src/bin/ix.rs` → `src/bin/ix/main.rs`)

### Fixed
- Events arriving during `builder.build()` compaction are now deferred and replayed
  as delta updates instead of being silently discarded by the event-drain loop
- TOCTOU window between pressure check and `builder.build()` closed: `compact()`
  now re-checks `guard.pressure()` immediately before the build and skips if ≥60%
- `#[allow(clippy::too_many_lines)]` moved from delegation function `run()` to the
  actual long function `run_main_loop()`
- Stale module declaration order comment in `lib.rs` updated to dual-heading format
  (alphabetical declaration order + leaf-first dependency order)
- Daemon wire protocol now surfaces partial-result warnings: `SearchResults.error`
  is set to a diagnostic message when `files_failed_verify > 0` during search
  execution — clients can detect incomplete results without inspecting `QueryStats`
- CLI `print_stats()` warning upgraded from bare counter to `[WARNING]` message with
  actionable text when files fail verification during search
- `VerificationResult::into_option()` documented as declared scar tissue:
  `Failed → None` semantic overload is intentional (rayon `filter_map` constraint);
  consumers must inspect `QueryStats::files_failed_verify` for partial-result detection

## [0.12.4] - 2026-06-10

### Added
- `CwdGuard` struct in CLI binary for exception-safe CWD mutation with Drop guard
- `BeaconStatusJson` and `SimpleStatusJson` serde structs replacing hand-built JSON formatting
- `QueryStats.total_available` field signaling `max_results` truncation to callers

### Changed
- CWD mutation in `execute_local_search` made exception-safe via `CwdGuard` Drop
- CLI `do_build()` now loads `.ixd.toml` config for `exclude_patterns`
- `config.rs` doc updated to note both CLI and daemon load `.ixd.toml`
- `.ixd.toml.md` docs expanded with CLI usage section
- `DAEMON-RUNBOOK.md` deprecated `ix --daemon` reference removed; last-verified date refreshed
- `README.md` clarified that `.ixd.toml` applies to both `ixd` and `ix --build`

### Fixed
- Error-path opacity hardened: additional `tracing::warn!` calls in reader, format, and executor for previously silent failure paths
- `usize` to `u32` FFI safety in Python bindings via `try_from` with `PyOverflowError` fallback
- 6 rustdoc warnings resolved (private item links, unresolved references) for clean docs.rs build

### Removed
- `run_daemon` function (deprecated since v0.7.0, zero consumers)
- Duplicated error doc line in `error.rs`

## [0.12.3] - 2026-06-10

### Added
- Progressive streaming search with `SearchResultsIter` and backpressure via `execute_progressive` in `daemon_sock`
- `--stdin` mode with `do_stdin_stream_search` using `stream_file_chunked` for piped input search
- `--chunk-size` and `--chunk-overlap` CLI flags for streaming search over large files
- `QueryStats.lines_read` tracking for stdin statistics in query results
- `stream_file_chunked` in streaming module with `QueryOptions` chunk size and overlap fields
- `search_window` helper and `StreamStats` struct for streaming search metrics
- `CdxBlockCorrupted` typed error variant in the `Error` enum replacing string panics
- `DaemonStatus` safety variants: `SafetyHalt`, `SafetyExit`, `Escalated`, `Warned`
- `broadcast_status` function for safety state propagation to all socket clients
- `Beacon.status` field (String) for human-readable daemon state in beacon.json
- `QueryStats.total_available` field signaling `max_results` truncation to callers
- `CwdGuard` struct in CLI binary for exception-safe CWD mutation with Drop guard
- `BeaconStatusJson` and `SimpleStatusJson` serde structs for JSON output formatting
- `ix-py` Python context manager support for `Index` class plus build tests
- `rust-toolchain.toml` pinning stable channel with MSRV 1.85
- `rustfmt.toml` configuring edition 2024 and max_width 100
- `clippy.toml` additions: `avoid-breaking-exported-api`, `type-complexity` threshold

### Changed
- Hand-built JSON in CLI replaced with `serde_json` for consistent string escaping
- CWD mutation in `execute_local_search` made exception-safe via `CwdGuard` Drop
- `daemon.rs` cache invalidation after compaction on both idle and update paths
- `verify_file` wired through streaming pipeline for thread-safe file verification
- CLI now loads `.ixd.toml` config in `do_build()` for `exclude_patterns`
- `config.rs` doc updated to note both CLI and daemon load `.ixd.toml`
- `--archive` flag warns without `--no-index`; `--max-file-size` warns in scanner mode

### Fixed
- Error-path opacity hardened: 35+ `tracing::warn!` calls added for silent failures in builder, reader, format, scanner, and executor
- Daemon socket error swallowing: query errors now propagated via wire protocol `error` field, enabling clients to distinguish query failure from zero results
- Archive security: `sanitize_archive_path` strips traversal sequences, tar entry type filter rejects non-file/non-dir entries, 100 MiB decompression bomb limit enforced
- Path length overflow in index paths now returns `Err` instead of silent truncation
- `usize` to `u32` FFI safety in Python bindings via `try_from` with `PyOverflowError` fallback
- Executable compression reintegrated for binary files such as Android ELF executables
- Scanner CWD resolution fixed for relative paths, matching builder's root-aware behavior

### Removed
- 5 dead exports: `varint::encoded_len`, `Header::file_count_usize`, `Header::total_trigrams`, and unused bloom and executor methods
- Orphaned `verify_stream` function and associated unused imports
- `run_daemon` function (deprecated since v0.7.0, zero consumers)
- Duplicated error doc line in `error.rs`

## [0.12.2] - 2026-06-10

### Fixed
- **Bloom filter div-by-zero panic** — `insert()`, `contains()`, and `slice_contains()` now guard against zero-size bloom filters, preventing a panic when a corrupted index passes an empty bits slice through the mmap reader guard.
- **Daemon error handling** — `builder.update()` failure branch now captures the changed file count before the move and emits structured warnings via `tracing`. Corrupted `beacon.json` now logs a warning before falling back instead of silently ignoring the error.
- **`VerificationResult` enum** — `verify_candidate()` now returns a dedicated enum (`Matches`, `Cached`, `Failed`) instead of conflating I/O errors and cache hits behind `Option<Vec<Match>>`. Callers can distinguish "no matches found" from "file could not be verified."
- **CLI exits 0 for non-existent directory** — `Scanner::scan()` now checks that the root path exists before walking, returning `Err` instead of silently producing zero results.
- **Ambiguous "no index found" error** — `find_index()` error messages now include the searched path, distinguishing "directory doesn't exist" from "directory exists but has no `.ix/` index."
- **Upgrade llmosafe to 0.7.4** — System metrics parsing no longer silently substitutes `0` on `/proc` parse failures, producing more accurate pressure readings.

### Changed
- **`ixd` daemon responsiveness** — `recv_timeout` reduced from 500ms to 100ms with an idle-tick counter gating dormant compaction at 5s. Redundant `ResourceGuard::pressure()` call eliminated by passing the pre-read value to both `evaluate_safety()` and `cache_policy.directive_for_pressure()`. Combined ~300ms average per-batch overhead reduction.
- **Configurable watcher debounce** — New `debounce_ms` field in `.ixd.toml` (default 500ms, range 50–10000ms) allows power users to trade batch efficiency for single-file change latency.

## [0.12.1] - 2026-06-07

### Fixed
- **Library panic elimination** — `sigaction()` in `daemon.rs` now returns `Result` instead of panicking via `expect()`. `compile_regex()` in `planner.rs` now returns `Result<Regex>` instead of calling `abort()`. Both comply with AGENTS.md no-panic library constraint.
- **CLI→daemon thread forwarding** — `SearchQuery` wire protocol now carries `threads` field. The `-j`/`--threads` CLI flag is forwarded to the daemon instead of being silently ignored.
- **Python bindings QueryPlan error handling** — Handle `Result<QueryPlan>` in Python bindings properly when compiling regular expressions.
- **Upgrade llmosafe to 0.7.3** — Tightened memory safety net and dependency upgrades.

### Fixed (clippy & test)
- Suppressed `clippy::unnecessary_wraps` in handle_service stubs.
- Fixed `cast_lossless` clippy lint in builder `free_bytes_at`.
- Fixed `sample_project` test fixture to use `ix.build()` as fallback.
- Fixed two macOS CI test failures (such as `rss_bytes_is_non_negative` test).

## [0.12.0] - 2026-06-07

### Fixed
- **Reader path resolution** — `Reader::get_file()` now resolves relative paths from the string pool against the index root, so file I/O works regardless of the caller's CWD. Previously, Python bindings returned `files_failed_verify: 1` when CWD differed from the index root.
- **`ix.build()` on fresh directories** — `Index.build()` is now a `@staticmethod` that creates an index without requiring an existing `.ix/shard.ix`. The convenience `ix.build(path)` function works on first build.
- **Stale cache after rebuild** — `Index.rebuild()` now clears posting, neg, and regex caches after rebuilding, preventing stale cached results from being returned.

### Changed
- **Python API: `Index.build()` → `Index.rebuild()`** — Instance method renamed to `rebuild()` to avoid collision with the new `@staticmethod build()`. Callers using `idx.build()` must update to `idx.rebuild()`.
- **`Config` derives `PartialEq`** — Enables equality comparison in tests.
- **Reuse `String` allocations in context_before** — `VecDeque<String>` in executor, archive, scanner, and streaming now reuses popped strings via `clear()` + `push_str()` instead of allocating new ones each iteration.

### Fixed (clippy)
- Non-ASCII literal escapes in format tests (`\u{}` syntax).
- `repeat().take()` → `repeat_n()` in format and property tests.
- `assert_eq!(x, true)` → `assert!(x)` in daemon_sock tests.
- Doc comments on proptest macro invocations → line comments.
- `cast_precision_loss` allows in format tests.

## [0.11.9] - 2026-06-06

### Added
- **llmosafe v0.7.2** — Upgraded from 0.7.1. Spin-loop DoS fix, Python bindings mirror, u128 FFI fixes, generation counter for handle reuse.
- **Python feature mirror** — `ix-py` now ships with `full` feature set (notify + decompress + archive) in PyPI wheels.
- **5 new regression tests** — delta-only keyword search, regex delta-only search, PostingCache oversized entry rejection, PostingCache evict_fraction FIFO semantics, daemon_sock `from_notify_kind()` rename mapping.

### Fixed
- **Stale detection timestamp** — `check_stale()` now displays `effective_created_at` (max of header + delta mtime) instead of stale header `created_at`.
- **Delta search - tombstoned files** — Tombstoned delta files no longer appear in candidate set.
- **Delta search - literal intersection** — Literal search delta candidates no longer eliminated by base-index intersection.
- **Delta search - regex merge** — Regex indexed search now merges delta postings during fragment intersection.
- **Planner non-UTF-8 regex** — Non-UTF-8 regex literals now use raw bytes for trigram extraction instead of `from_utf8_lossy()`.
- **Builder stream position** — Error propagation for stream position (no more silent `unwrap_or(0)`).
- **Builder offset chunking** — Trigram entries with >10,000 offsets now chunked into multiple PostingEntry records.
- **PostingCache oversized entries** — Entries exceeding memory ceiling now rejected instead of violating budget.
- **Daemon `from_notify_kind()`** — Rename events now correctly map to `FileOp::Rename`.
- **Daemon beacon write errors** — 10 `beacon.write_to()` calls now emit `eprintln!` warnings on failure.
- **Daemon idle double-count** — `idle.record_change()` no longer called twice in compaction path.
- **Watcher race condition** — Events accumulated during `build()` are now drained to prevent duplicate delta entries.
- **Watcher `.ix` path filter** — Now uses `starts_with(ix_dir)` instead of component matching to prevent false exclusion of paths like `src/.ix_utils/`.
- **CLI flag gaps** — `--archive` warns without `--no-index`; `--max-file-size` warns in scanner mode; `--daemon` errors on non-Unix with notify feature.

### Docs
- Fixed `.ixd.toml.md` format, broken intra-doc links, outdated entropy thresholds in DAEMON-RUNBOOK, added `service status` to README, Pipeline stub in `ix-py/__init__.pyi`.

## [0.11.8] - 2026-06-05

### Added
- **llmosafe v0.7.1** — Upgraded from 0.6.2. DesignAssuranceLevel-A wired into daemon EscalationPolicy preventing silent safety regression.
- **Pipeline safety binding for ix-py** — New `Pipeline` class wrapping llmosafe cognitive safety pipeline via C-ABI for Python consumers.
- **PostingCache evict_fraction()** — Proportional eviction method for CacheDirective-driven memory pressure management.

### Changed
- **PostingCache memory ceiling unified with CachePolicy** — `PostingCache::with_ceiling()` added; daemon passes CachePolicy ceiling to PostingCache instead of hard-coded 64MB.
- **Watcher receives watch_roots and exclude_patterns** — Watcher now scopes to configured watch roots and excludes patterns, preventing out-of-scope file indexing.
- **Header flag contract enforced** — `POSTING_LISTS_COMPRESSED` flag now set (reflects reality: postings are ZSTD-compressed). Added `has_compressed_postings()` and `has_checksum()` methods.
- **Shared file-walk filter** — `default_filter_entry()` extracted to builder.rs reducing duplication across builder/watcher modules.
- **ResourceGuard::for_testing()** — Added `testing` feature for deterministic pressure injection in cache_policy tests.
- **Daemon cache invalidation on update** — Both PostingCache and NegCache invalidated after builder.update(), preventing stale cache entries in long-running daemons.

### Fixed
- **Missing SAFETY comments** — Added SAFETY documentation on `libc::statvfs`, `Mmap::map`, and `libc::getuid` unsafe blocks.
- **Fuzz Cargo.toml build** — Fuzz manifest now links against moeix; both fuzz targets updated for current API.
- **Property test doc comments** — Converted doc-comment-on-proptest-macro warnings to regular comments.
- **Unnecessary clippy allows** — Scoped `too_many_lines` allow from crate-level to module-level.

## [0.11.7] - 2026-06-03

### Fixed
- **Delta compaction unreachable during active development** — `record_change()` was resetting the idle timer before the `Dormant` check, making auto-compaction structurally unreachable when files changed frequently. The delta file grew unbounded (15MB → 182MB over hours), degrading query latency and eventually hitting `ENODEV` on `fsync()`.

### Changed
- **Delta size threshold** — Added a 50MB delta size threshold that triggers compaction even when the daemon never becomes `Dormant` (active development scenario).
- **Periodic idle compaction** — The main loop timeout branch now checks for `Dormant` state and compacts during long idle periods, even without file changes.

## [0.11.6] - 2026-06-01

### Added
- **Daemon auto-compaction** — The ixd daemon now automatically rebuilds the index after 30+ minutes of inactivity when delta entries exist. Prevents unbounded delta growth that degraded query latency over the daemon's lifetime.

### Fixed
- **Delta not deleted after build** — `Builder::build()` now removes `shard.ix.delta` and `shard.ix.delta.tmp` after successful serialization, preventing stale tombstone corruption on the next update.

## [0.11.5] - 2026-06-01

### Fixed
- **Windows cross-compilation (release workflow, round 3)** — Remaining `ix::daemon_sock` references in `ix` binary IPC path gated behind `#[cfg(unix)]`. `ixd` binary given Windows stub. All platform-exclusive modules (`daemon`, `daemon_sock`) and their consumers now properly guarded.

## [0.11.4] - 2026-06-01

### Fixed
- **Windows cross-compilation (release workflow)** — `daemon` and `daemon_sock` modules gated behind `#[cfg(all(feature = "notify", unix))]`; `Beacon::is_live()` body gated `#[cfg(unix)]` with Windows fallback. Fixes 6 additional compilation errors on `x86_64-pc-windows-msvc` (`nix::sys::signal`, `nix::unistd`, `os::unix::net`) that blocked the `cargo-dist` release workflow.

## [0.11.3] - 2026-06-01

### Fixed
- **Windows cross-compilation in release build** — `free_bytes_at()` and `socket_path()` now gated behind `#[cfg(unix)]` with no-op fallbacks on Windows, fixing the `cargo-dist` release workflow on `x86_64-pc-windows-msvc` that failed due to missing `libc::statvfs` and `libc::getuid`.

## [0.11.2] - 2026-06-01

### Fixed
- **`looks_like_regex()` heuristic now uses `regex_syntax` parser** — The manual char-scan heuristic only detected 13 of ~60 regex constructs (backslash-escaped metacharacters: `\|\(\)\{\}\.\*\+\?\[\]\^\$`), giving 23 false negatives for common patterns like `\d+`, `\w+`, `\s*`, `^foo`, `foo$`, `\bword\b`, and all bare metacharacters. Replaced with `regex_syntax::ParserBuilder` + `HirKind::Literal` check, achieving 100% coverage via the same parser already used by the planner. Eliminates the dual-decision compound bug where two independently-maintained regex detection systems produced different answers. The 0.11.0 changelog entry claiming the hint detects `\w`, `\d`, `\s` is now actually true.

## [0.11.1] - 2026-05-31

### Fixed
- **False "index is stale" warning with ixd daemon** — `check_stale()` now considers delta file freshness via `max(header.created_at, delta_mtime)`, preventing false positives when the daemon's incremental `update()` keeps the index current but never refreshes the main shard's `created_at`. Also aligned `get_last_modified()` WalkBuilder filters with `Builder::build()` by adding `.git_global(true).git_exclude(true)`.
- **Docstring drift** — `Header.created_at` docstring corrected from "seconds" to "microseconds" (code was always micros).

### Changed
- **Service install bug documented** — `docs/bugs/BUG--service-install-multi-root.md` records the single-path overwrite and multi-root dead-loop issues for future fix.

## [0.11.0] - 2026-05-30

### Added
- **Property-based tests** — 13 proptest-powered invariants spanning trigram, posting list, varint, bloom filter, binary detection, and string pool modules. Every G-SEM failure mode now has randomized edge-case coverage.
- **CBP boundary tests** — 6 integration tests covering C1 (delta format contract), C3 (CDX block index semantic preservation), and C4 (build sequence independence).
- **Concurrency test** — C5 resource contention verified: 8 threads × 50 queries on shared read-only index, confirming cache consistency under parallel access.
- **UX hint** — When literal-mode search uses regex-like escapes (`\w`, `\d`, `\s`) and returns zero results, stderr suggests `--regex` flag.

### Changed
- **T-WEAKORACLE fixes** — Integration and JSON golden tests now assert exact content, line numbers, and file names instead of substring/length-only checks.
- **Test count** — 102 → 123 tests (21 net new: 6 boundary + 2 concurrency + 13 property).
- **Documentation updates** — Lib-level crate docs, README library example, CLI help text, and daemon module docs corrected for G-HALL issues (wrong API signatures, stale version claims).

### Fixed
- **F1-F4 CBP compound bugs** — Atomic delta rename, verify error counter, pressure divergence unification via `AdaptiveCachePolicy`, and `DeltaReader` `?` propagation.
- **37 clippy warnings resolved** — Zero-warning gate across `all` + `nursery` + `pedantic` + `cargo` lint groups. Only 2 `#[allow]` annotations remain (justified: `trivial_regex` fallback patterns, `option_if_let_else` readability).
- **Lock tightening** — 18 `significant_drop_tightening` violations fixed via explicit `drop(guard)` after last mutation in all cache hot paths.

### Technical Details
- Index format: unchanged (v1.3)
- Backward compatible: yes
- Breaking changes: no
- Migration: none required
- All 123 tests pass (76 lib + 47 integration)
- Zero clippy warnings (all + nursery + pedantic + cargo)

## [0.10.0] - 2026-05-28

### Added
- **`.ixd.toml` configuration** — The daemon now discovers `.ixd.toml` files to scope watch roots and exclude patterns. Configs are merged from root + subdirectories (2 levels deep). See `docs/.ixd.toml.md` for schema.
- **Builder `exclude_patterns`** — `Builder::with_exclude_patterns()` accepts directory name patterns to skip during file walk. Patterns are matched against the last component of each entry's path.
- **`cache_policy` module** — `AdaptiveCachePolicy` driven by `ResourceGuard` memory pressure, now wired into the daemon's main loop. Produces `CacheDirective`s with zone-based eviction decisions, logged per change batch.
- **`streaming` module** — Alternative file search implementations (line-based and mmap-windowed) now declared in `lib.rs` and fully functional. 4 new tests unearthed.
- **`api` module** — Convenience `execute()` wrapper around planner + executor pipeline for simple single-call searches. Rewritten with correct `Executor`/`Planner` API signatures.
- **Service management** — `ix service install/start/stop/restart` nested under `ix service` subcommand (was flat `ix install/start/stop`). Added `Restart` support via `systemctl --user restart`.
- **Documentation routing table** — `README.md` now serves as a hub linking to QUICKSTART, DAEMON-RUNBOOK, SOCKET-API, `.ixd.toml`, DELTA-FORMAT, BENCHMARKS, CONTRIBUTING, and CHANGELOG.
- **`rust-version` in `[package]`** — crates.io now displays minimum Rust version (1.85) on the package page.
- **`ixd --help`** — Now includes examples and links to DAEMON-RUNBOOK and `.ixd.toml` docs.
- **`doc(cfg)` annotations** — All feature-gated (`notify`) modules and re-exports annotated for docs.rs rendering.
- **`.ixd.toml.md`** — Full config reference with schema, examples, and verification commands.

### Changed
- **llmosafe 0.6.1 → 0.6.2** — Upgraded for v0.6.2 witness-token architecture. Watcher fallback cognitive pipeline simplified to `ResourceGuard::check()` + `pressure()` — same safety, no witness-token overhead.
- **Documentation moved** — `POSTMORTEM*` + `DAEMON-SOCKET-INTERNALS` → `docs/internals/`. `SECURITY.md` + `CODE_OF_CONDUCT.md` → repository root (GitHub convention).
- **`Plan::plan_with_pool()`** — Wired into `daemon_sock::execute_search()`, replacing `plan_with_options()` for regex-pool-aware planning.
- **`Config` struct** — No longer dead code. `Config::load()` and `Config::discover_under()` implemented. Daemon applies discovered exclude patterns before building.
- **`--negate` ghost removed** — README no longer references the unimplemented `--negate` flag.

### Fixed
- **5 dead-code modules/types wired** (~587 lines): `streaming.rs` (370L, 4 tests), `api.rs` (21L), `Config` struct, `AdaptiveCachePolicy`/`CacheDirective`, `Planner::plan_with_pool`.
- **Non-ASCII literals** — Em dashes replaced with ASCII dashes (clippy `non_ascii_literal`).

### Security
- **llmosafe 0.6.2** — `check_blocking()` now bounded (3 retries → `DeadlineExceeded`) instead of spinning indefinitely under sustained pressure. `ResourceGuard::auto()` fail-closed on non-Linux.

### Housekeeping
- **`.gitignore` hardened** — Blocks 32+ agentic/AI tooling directories (`cursor`, `opencode`, `claude`, `copilot`, `windsurf`, etc.) and generated files (`.prompt.md`, `PLAN.md`, `analysis.md`, agent sessions).

### Technical Details
- Index format: unchanged (v1.3)
- Backward compatible: yes
- Breaking changes: no
- Migration: `--build` not required (index format unchanged)
- All 97 tests pass (73 lib + 24 integration/streaming)

## [0.9.0] - 2026-05-21

### Added
- **Multi-root daemon support** — The `ixd` daemon can now watch multiple project roots simultaneously in a single process. Each root runs on its own thread with independent index, watcher, beacon, and Unix domain socket. Signal handling and `ResourceGuard` are shared across all roots.
  - CLI: `ixd /project-a /project-b /project-c`
  - Backward compatible: single-root usage `ixd /project` still works
  - Instance ID prevents false positive concurrent instance detection

### Changed
- **Improved `-n` / `--max-results` flag reliability** — Removed broken early termination logic from parallel iterator that caused non-deterministic behavior. Results are now collected fully and truncated afterward, improving reliability from 0% to ~70% for small limits.
  - Note: Due to parallel execution, exact result count may vary slightly. For precise limiting, use `-n 0 | head -n N`.

### Technical Details
- Index format: unchanged (v1.3)
- Backward compatible: yes
- Breaking changes: no
- Migration: none required
- All 73 tests pass

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
