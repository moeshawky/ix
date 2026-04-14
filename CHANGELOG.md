# Changelog

All notable changes to this project will be documented in this file.

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
