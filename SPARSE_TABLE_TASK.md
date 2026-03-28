# ix Sparse Trigram Table — Release Blocker Fix

You are HANDS. Execute these operations IN ORDER. Do not add anything not listed.

## Problem

The trigram table is FIXED at 256MB (16.7M slots × 16 bytes) regardless of project size. A 10-file project gets a 256MB index. This kills adoption.

## Fix

Replace the fixed-size trigram table with a sparse hash map. Only store non-empty trigram slots.

### Op 1: Change trigram table format

File: `src/lib/format.rs`

The trigram table currently assumes direct indexing: `trigram_value → array[trigram_value]`. Change to a sorted array of `(trigram: u32, entry: TrigramEntry)` pairs, binary-searched at query time.

New trigram table layout:
```
trigram_count: u32 (from header, already exists)
entries: [(trigram: u32, posting_offset: u48, posting_length: u32, doc_frequency: u32, padding: u16)]
```

Each entry: 4 (trigram key) + 16 (existing entry) = 20 bytes.
For Charlie (~6M non-empty trigrams): 6M × 20 = ~120MB instead of 256MB.
For a 10-file project (~50K trigrams): 50K × 20 = ~1MB instead of 256MB.

Update `TRIGRAM_ENTRY_SIZE` from 16 to 20.

### Op 2: Update builder

File: `src/lib/builder.rs`

When writing the trigram table:
- Collect only non-empty trigrams into a sorted Vec<(u32, TrigramEntry)>
- Write trigram_count to header (already done)
- Write sorted entries sequentially
- Remove the `for trigram_idx in 0..16_777_216` loop

### Op 3: Update reader

File: `src/lib/reader.rs`

When looking up a trigram:
- Binary search the sorted trigram table for the target trigram key
- O(log n) where n = non-empty trigrams (~6M for large repos = ~23 comparisons)
- This replaces O(1) direct index. 23 comparisons on mmap'd data is still <1μs.

### Op 4: Bump format version

File: `src/lib/format.rs`

Change `VERSION_MINOR` from 0 to 1. The reader should reject indexes with version 1.0 (old format) and require 1.1 (sparse).

### Op 5: Verify

```bash
cargo test
cargo clippy -- -D warnings
cargo build --release
cargo install --path . --force

# Rebuild Charlie index — should be significantly smaller
cd /workspace/charlie && rm -rf .ix && ix --build
ls -lh .ix/shard.ix

# Search still works
ix "ControlPlane" core/ | head -5
ix --json "HookEngine" core/ | head -3
ix -c "fn "

# Build on a tiny project — index should be small
cd /workspace/ix && rm -rf .ix && ix --build
ls -lh .ix/shard.ix
```

Report: old size vs new size for both Charlie and ix repos. Commit and push.

## BANNED

- Changing anything not listed above
- Adding features
- Touching CLI flags, scanner, or executor logic beyond what trigram lookup changes require
