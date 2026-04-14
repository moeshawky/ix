# Changelog

All notable changes to this project will be documented in this file.

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
