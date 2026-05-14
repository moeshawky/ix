# Delta Index Format Specification

**Version:** 1.3 (v0.6.2+)  
**Source:** `src/lib/format.rs`, `src/lib/builder.rs`, `src/lib/reader.rs`  
**Last Verified:** 2026-05-14  
**Verification:** `cargo test --all-features -- delta`  

---

## Overview

The delta index format enables incremental updates to the trigram index without full rebuilds. Instead of rewriting the entire `shard.ix` file, changes are appended to `shard.ix.delta`.

**Key Properties:**
- **Append-only**: No in-place modifications (prevents corruption on crash)
- **Tombstone-based deletion**: Deleted files marked, not removed
- **Backward compatible**: Readers ignore missing delta files
- **Not persisted**: Delta is merged into main index on daemon restart

---

## File Structure

### Magic Bytes

Delta files start with a 4-byte magic sequence:

```rust
pub const DELTA_MAGIC: [u8; 4] = [0x49, 0x58, 0x44, 0x4C]; // b"IXDL"
```

### Entry Types

| Type | Byte | Description |
|------|------|-------------|
| `DELTA_TOMBSTONE` | `0x01` | Marks a file as deleted |
| `DELTA_FILE_ENTRY` | `0x02` | Inline file metadata + bloom filter |
| `DELTA_TRIGRAM_ENTRY` | `0x03` | Trigram posting list entry |

---

## Entry Formats

### Tombstone Entry

Marks a file ID as deleted.

```
Offset  Size  Field
------  ----  -----
0       1     Entry type (0x01)
1       4     File ID (little-endian u32)
```

**Example:**
```rust
// Tombstone for file ID 42
[0x01, 0x2A, 0x00, 0x00, 0x00]
```

### File Entry

Inline file metadata with bloom filter.

```
Offset  Size  Field
------  ----  -----
0       1     Entry type (0x02)
1       4     File ID (little-endian u32)
5       2     Path length (little-endian u16)
7       N     Path bytes (UTF-8, N = path length)
7+N     8     Mtime (little-endian u64, nanoseconds)
15+N    8     Size (little-endian u64)
23+N    8     Content hash (XXH64, little-endian u64)
31+N    260   Bloom filter (256-bit + 4-byte header)
```

**Example:**
```rust
// File entry for "/tmp/test.txt", ID=1, mtime=..., size=1234, hash=...
[0x02,                   // Entry type
 0x01, 0x00, 0x00, 0x00, // File ID = 1
 0x0E, 0x00,             // Path length = 14
 b"/tmp/test.txt",       // Path (14 bytes)
 ... // mtime, size, hash, bloom
]
```

### Trigram Entry

Posts offsets for a trigram to a file.

```
Offset  Size  Field
------  ----  -----
0       1     Entry type (0x03)
1       4     Trigram (little-endian u32)
5       4     Reserved (always 1)
9       4     File ID (little-endian u32)
13      4     Offsets count (little-endian u32)
17      4×N   Offsets (little-endian u32 array)
```

**Example:**
```rust
// Trigram 0x746573 ("tes") with 2 offsets in file 1
[0x03,                   // Entry type
 0x73, 0x65, 0x74, 0x00, // Trigram (little-endian)
 0x01, 0x00, 0x00, 0x00, // Reserved = 1
 0x01, 0x00, 0x00, 0x00, // File ID = 1
 0x02, 0x00, 0x00, 0x00, // Offsets count = 2
 0x00, 0x00, 0x00, 0x00, // Offset 0
 0x05, 0x00, 0x00, 0x00, // Offset 5
]
```

---

## Reader Behavior

### Opening a Delta File

```rust
// From src/lib/reader.rs:DeltaReader::open()
pub fn open(path: &Path) -> Result<Self> {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == NotFound => return Ok(Self::default()),
        Err(e) => return Err(e.into()),
    };
    
    // Verify magic
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)?;
    if magic != DELTA_MAGIC {
        return Ok(Self::default());
    }
    
    // Parse entries...
}
```

**Behavior:**
- Returns `Default::default()` if file missing or invalid magic
- Returns `Err` on corruption (I/O error)
- Parses entries sequentially until EOF

### Merging with Main Index

```rust
// From src/lib/executor.rs:Executor::get_file_info()
fn get_file_info(&self, fid: u32) -> Option<FileInfo> {
    // Check delta first (overrides main index)
    if fid >= self.index.header.file_count {
        let delta = self.delta.as_ref()?;
        let info = delta.id_to_fileinfo.get(&fid)?;
        Some(FileInfo { ... })
    } else {
        self.index.get_file(fid).ok()
    }
}

// Tombstone filtering
fn is_tombstoned(&self, file_id: u32) -> bool {
    self.delta
        .as_ref()
        .is_some_and(|d| d.tombstones.contains(&file_id))
}
```

**Merge Strategy:**
1. Delta file entries override main index
2. Tombstoned files are excluded from search results
3. New trigram postings are merged at query time

---

## Writer Behavior

### Appending a File Entry

```rust
// From src/lib/builder.rs:Builder::process_file_delta()
fn process_file_delta<W: Write>(
    &mut self,
    path: &Path,
    file_id: u32,
    delta: &mut W,
) -> Result<bool> {
    // Extract metadata
    let metadata = fs::metadata(path)?;
    let size = metadata.len();
    let mtime = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_nanos() as u64;
    let content_hash = xxh64::xxh64(&data, 0);
    
    // Extract trigrams
    let pairs = self.extractor.extract_with_offsets(&data);
    
    // Build bloom filter
    let mut bloom = BloomFilter::new(256, 5);
    for (tri, _) in &pairs {
        bloom.insert(*tri);
    }
    
    // Write file entry
    delta.write_all(&[DELTA_FILE_ENTRY])?;
    delta.write_all(&file_id.to_le_bytes())?;
    delta.write_all(&(path.len() as u16).to_le_bytes())?;
    delta.write_all(path.as_bytes())?;
    delta.write_all(&mtime.to_le_bytes())?;
    delta.write_all(&size.to_le_bytes())?;
    delta.write_all(&content_hash.to_le_bytes())?;
    
    let mut bloom_buf = Vec::new();
    bloom.serialize(&mut bloom_buf)?;
    delta.write_all(&bloom_buf)?;
    
    // Write trigram entries
    for (tri, offsets) in trigram_entries {
        delta.write_all(&[DELTA_TRIGRAM_ENTRY])?;
        delta.write_all(&tri.to_le_bytes())?;
        delta.write_all(&1u32.to_le_bytes())?; // reserved
        delta.write_all(&file_id.to_le_bytes())?;
        delta.write_all(&(offsets.len() as u32).to_le_bytes())?;
        for off in offsets {
            delta.write_all(&off.to_le_bytes())?;
        }
    }
    
    Ok(true)
}
```

### Tombstoning a File

```rust
// From src/lib/builder.rs:Builder::update()
for path in changed_files {
    // If file existed before, mark as tombstone
    if let Some(&old_id) = path_to_id.get(path) {
        delta_out.write_all(&[DELTA_TOMBSTONE])?;
        delta_out.write_all(&old_id.to_le_bytes())?;
    }
    
    // If file still exists, add new entry
    if path.exists() && self.process_file_delta(path, next_file_id, &mut delta_out)? {
        next_file_id += 1;
    }
}
```

---

## Performance Characteristics

| Metric | Value | Measurement |
|--------|-------|-------------|
| Write latency | O(changed_files) | Linear in changed files, not total files |
| Read overhead | O(delta_entries) | Merged at query time |
| Tombstone lookup | O(1) | HashSet in `DeltaReader` |
| Bloom filter size | 260 bytes | 256-bit + 4-byte header |

**Sekel Compliance:**
- Cold start: <3s (full rebuild)
- Incremental update: <100ms for 10 changed files
- Memory overhead: ~1MB per 1000 delta entries

---

## Error Modes

### Corruption Detection

```rust
// Magic check fails
if magic != DELTA_MAGIC {
    return Ok(Self::default()); // Treat as missing, not corrupt
}

// I/O error during read
file.read_exact(&mut buf)?; // Returns Err, propagated to caller
```

**Behavior:**
- Invalid magic → treat as empty delta (graceful degradation)
- I/O error → return `Err`, caller handles (usually skips delta)

### Recovery

1. **On corruption**: Delete `shard.ix.delta`, rebuild from main index
2. **On crash**: Delta is append-only, so partial writes are safe (incomplete entries ignored)
3. **On merge failure**: Retry on next query (delta is immutable once written)

---

## Example Usage

### Building with Delta

```bash
# Initial build (creates shard.ix)
ix --build /path/to/repo

# File changes detected by daemon
# Daemon appends to shard.ix.delta

# Search uses both main + delta
ix "query"
```

### Inspecting Delta

```rust
use ix::reader::DeltaReader;

let delta = DeltaReader::open("/path/to/.ix/shard.ix.delta")?;
println!("Tombstoned files: {:?}", delta.tombstones);
println!("New file entries: {}", delta.total_file_entries);
println!("Trigram postings: {}", delta.postings.len());
```

---

## Verification

### Test Coverage

```bash
# Run delta-related tests
cargo test --all-features delta
cargo test --all-features update
cargo test --all-features tombstone
```

### Manual Inspection

```bash
# Build index
ix --build /tmp/test

# Modify a file
echo "new content" >> /tmp/test/file.rs

# Check delta exists
ls -la /tmp/test/.ix/shard.ix.delta

# Inspect delta (hex dump)
hexdump -C /tmp/test/.ix/shard.ix.delta | head -20
```

---

## Changelog

| Version | Date | Changes |
|---------|------|---------|
| 1.3 | 2026-05-03 | Initial implementation (v0.6.2) |
| 1.3 | 2026-05-14 | Documentation added |

---

## See Also

- `src/lib/format.rs` — Constants and magic bytes
- `src/lib/builder.rs:816-876` — Delta writer implementation
- `src/lib/reader.rs:549-677` — Delta reader implementation
- `src/lib/executor.rs` — Delta merging at query time
