# ADR: Streaming Search for ix

## Status: Proposed

## Context
Current decompression and archive logic eager-loads decompressed content into a `Vec<u8>`. This creates OOM risks for large files or decompression bombs, necessitating a 10MB cap. Users want to search larger files without caps.

## Decision
We will transition from eager buffering to streaming line-by-line verification.

## Requirements
- **R-01: Streaming Decompression**: Handle arbitrarily large `.gz`, `.zst`, `.bz2`, `.xz` files.
- **R-02: Constant Memory Verification**: Process files line-by-line using `BufRead`.
- **R-03: Context Support**: Implement context (-C) via a sliding window of lines.
- **R-04: Multiline Support**: For `-U`, buffer only the match window, not the whole file.

## Architecture Pattern: Streaming Pipe
We will use the **Streaming Pipe** pattern. Decompressors will return a `Box<dyn Read + Send>` which is wrapped in a `BufReader` by the executor/scanner.

## Boundaries & Contracts

### Boundary: Decompression API (`src/lib/decompress.rs`)
- **Input**: `path: &Path`, `raw: &[u8]` (the mmap'd compressed data)
- **Output**: `Result<Option<Box<dyn Read + Send>>>`
- **Invariant**: The returned reader must be thread-safe (`Send`) to support Rayon.

### Boundary: Archive API (`src/lib/archive.rs`)
- **Input**: `path: &Path`, `regex: &Regex`, `options: &QueryOptions`
- **Output**: `Result<Vec<Match>>`
- **Change**: Internal implementation will use streaming entries instead of `read_to_end`.

### Boundary: Verification API (`src/lib/executor.rs` and `scanner.rs`)
- **Method**: `verify_stream<R: Read>(reader: R, ...)`
- **Logic**: 
  1. Wrap in `BufReader`.
  2. Maintain `VecDeque<String>` for `context_before`.
  3. For each match, read ahead N lines for `context_after`.

## Implementation Plan

### Phase 1: Streaming Decompressor
1. Update `maybe_decompress` to return `Box<dyn Read + Send>`.
2. Implement for all 4 formats.

### Phase 2: Streaming Executor Logic
1. Implement `verify_stream` in `executor.rs`.
2. Implement sliding window for context.
3. Handle multiline matching in chunks or line-buffered mode.

### Phase 3: Streaming Scanner Logic
1. Implement `scan_stream` in `scanner.rs`.
2. Standardize binary check on the first 8KB of the stream.

### Phase 4: Integration & Verification
1. Remove `DECOMPRESSION_LIMIT`.
2. Test with 50MB+ gzipped file.
3. Verify `-C 3` works correctly on large streams.
