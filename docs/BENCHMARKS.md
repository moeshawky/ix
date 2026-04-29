# Benchmark Results

## Test Environment
- **Machine**: Linux (workspace container)
- **ix version**: 0.4.0 (release build)

---

## Test 1: Small Dataset (100 files, 5.7MB, all files match)

| Tool | Time | Notes |
|------|------|-------|
| **ix (indexed)** | 305ms | 60170 matches, all 1000 files verified |
| **ripgrep (rg)** | 28ms | Brute-force parallel scan |
| **grep** | 32ms | Standard recursive grep |

**Winner**: rg (small dataset, all files match trigram)

---

## Test 2: Selective Matching (5000 files, 20MB, 10% match "authtoken")

| Tool | Time | Files Scanned |
|------|------|---------------|
| **ix (indexed)** | 40ms | 500 candidates, 500 verified |
| **ripgrep (rg)** | 38ms | All 5000 files scanned |
| **grep** | 63ms | All 5000 files scanned |

**Winner**: Tie (ix slightly slower, but scans 10x fewer files)

### ix Internal Stats (Test 2)
```
trigrams_queried: 7
posting_lists_decoded: 3
candidate_files: 500  (pruned from 5000 via trigrams!)
files_verified: 500
bytes_verified: 25KB
total_matches: 500
search_time_ms: 40
```

---

## Test 3: Criterion Micro-benchmarks

| Benchmark | Time |
|-----------|------|
| trigram_extraction_1mb | 5.29ms |
| posting_decode_1000_files | 104µs |
| search_literal_100_files_1mb | 21.7ms |

---

## When ix Wins

1. **Large codebases** — trigram pruning avoids scanning all files
2. **Repeated searches** — index built once, reused
3. **Agent workflows** — JSON output, context, UTCP schema
4. **Complex regex** — trigram pre-filtering reduces verification

## When rg Wins

1. **One-shot searches** — no index needed
2. **Small codebases** — brute-force is fast enough
3. **Simple pattern matching** — rg's optimization shines

---

## Running Benchmarks

```bash
# Build release
cargo build --release

# Run criterion benchmarks
cargo bench

# Compare with rg
time rg "pattern" /path/to/codebase

# Compare with ix (with stats)
ix --stats "pattern" /path/to/codebase
```

---
*Last updated: 2026-04-26*
