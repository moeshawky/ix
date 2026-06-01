# Changelog

All notable changes to this project will be documented in this file.

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
