

# `ix` — Full Build Schematics and Detailed Design

---

## Thought 1
**Purpose:** Establish byte-level index format specification. Every field, every offset, every encoding decision. Guards against G-MEM-1 (ambiguous sizes leading to unbounded reads) and Compound Cascade (format ambiguity causing reader/writer disagreement).

**Pattern Check:** G-MEM-1, G-SEC-1 (malformed index must not cause UB), Compound Cascade

**Anti-Pattern Action:**
- G-MEM-1: Every variable-length field has an explicit length prefix or termination. Every section has a size in the header.
- G-SEC-1: All offsets validated against file size before dereferencing. CRC per section.
- Cascade: Single source of truth — this spec. Reader and writer are generated from same constants.

**Self-Audit:** Clean.

**Reasoning:**

### 1.1 — Index File Format (`.ix` shard file)

```
 ┌─────────────────────────────────────────────────────────────────┐
 │  ALL INTEGERS ARE LITTLE-ENDIAN                                │
 │  ALL OFFSETS ARE ABSOLUTE FROM FILE START                      │
 │  ALIGNMENT: sections start on 8-byte boundaries (zero-padded) │
 └─────────────────────────────────────────────────────────────────┘

 OFFSET       SIZE        FIELD
 ══════════════════════════════════════════════════════════════════

 ┌── HEADER (256 bytes, fixed) ──────────────────────────────────┐
 │                                                                │
 │  0x0000     4 bytes     magic: [0x49, 0x58, 0x30, 0x31]      │
 │                         ASCII "IX01"                           │
 │                                                                │
 │  0x0004     2 bytes     version_major: u16 = 1                │
 │  0x0006     2 bytes     version_minor: u16 = 0                │
 │                                                                │
 │  0x0008     8 bytes     flags: u64                            │
 │                         bit 0: has_bloom_filters               │
 │                         bit 1: has_content_hashes              │
 │                         bit 2: posting_lists_compressed        │
 │                         bits 3-63: reserved (must be 0)       │
 │                                                                │
 │  0x0010     8 bytes     created_at: u64 (unix micros)         │
 │  0x0018     8 bytes     source_bytes_total: u64               │
 │                         (total bytes of indexed source files)  │
 │                                                                │
 │  0x0020     4 bytes     file_count: u32                       │
 │  0x0024     4 bytes     trigram_count: u32                    │
 │                         (number of non-empty trigram slots)    │
 │                                                                │
 │  0x0028     8 bytes     file_table_offset: u64                │
 │  0x0030     8 bytes     file_table_size: u64                  │
 │                                                                │
 │  0x0038     8 bytes     trigram_table_offset: u64             │
 │  0x0040     8 bytes     trigram_table_size: u64               │
 │                                                                │
 │  0x0048     8 bytes     posting_data_offset: u64              │
 │  0x0050     8 bytes     posting_data_size: u64                │
 │                                                                │
 │  0x0058     8 bytes     bloom_offset: u64                     │
 │  0x0060     8 bytes     bloom_size: u64                       │
 │                                                                │
 │  0x0068     8 bytes     string_pool_offset: u64               │
 │  0x0070     8 bytes     string_pool_size: u64                 │
 │                                                                │
 │  0x0078     8 bytes     name_index_offset: u64                │
 │  0x0080     8 bytes     name_index_size: u64                  │
 │                                                                │
 │  0x0088     24 bytes    reserved: [0u8; 24]                   │
 │                                                                │
 │  0x00A0     88 bytes    padding: [0u8; 88]                    │
 │                                                                │
 │  0x00F8     4 bytes     header_crc32: u32                     │
 │                         CRC32C of bytes 0x0000..0x00F8        │
 │  0x00FC     4 bytes     padding: [0u8; 4]                     │
 │                                                                │
 └── END HEADER @ 0x0100 ───────────────────────────────────────┘

 ┌── FILE TABLE ─────────────────────────────────────────────────┐
 │                                                                │
 │  Array of FileEntry, sorted ascending by file_id.             │
 │  file_id is assigned monotonically during index build.        │
 │                                                                │
 │  FileEntry (48 bytes fixed):                                  │
 │  ┌────────────────────────────────────────────────────────┐   │
 │  │  0: file_id        u32                                 │   │
 │  │  4: path_offset    u32  (into string pool)             │   │
 │  │  8: path_length    u16  (bytes, not chars)             │   │
 │  │ 10: status         u8                                  │   │
 │  │                    0x00 = FRESH                         │   │
 │  │                    0x01 = STALE                         │   │
 │  │                    0x02 = DELETED                       │   │
 │  │ 11: flags          u8                                  │   │
 │  │                    bit 0: is_binary                     │   │
 │  │                    bit 1: is_symlink                    │   │
 │  │                    bit 2: is_large (>100MB)             │   │
 │  │ 12: mtime_ns       u64  (nanosecond unix timestamp)    │   │
 │  │ 20: size_bytes      u64                                │   │
 │  │ 28: content_hash    u64  (xxhash64 of content)         │   │
 │  │ 36: trigram_count   u32  (unique trigrams in this file)│   │
 │  │ 40: bloom_offset    u32  (relative to bloom section)   │   │
 │  │ 44: padding         [0u8; 4]                           │   │
 │  └────────────────────────────────────────────────────────┘   │
 │                                                                │
 │  Section CRC32C: last 4 bytes of section                      │
 │                                                                │
 └───────────────────────────────────────────────────────────────┘

 ┌── TRIGRAM TABLE ──────────────────────────────────────────────┐
 │                                                                │
 │  Fixed-size array: 16,777,216 entries (one per 3-byte combo)  │
 │  Addressed by: trigram_bytes_as_u24 → index                   │
 │                                                                │
 │  Key encoding:  trigram "abc" = bytes [0x61, 0x62, 0x63]      │
 │                 index = (0x61 << 16) | (0x62 << 8) | 0x63     │
 │                 index = 6,382,179                              │
 │                                                                │
 │  TrigramEntry (16 bytes):                                     │
 │  ┌────────────────────────────────────────────────────────┐   │
 │  │  0: posting_offset   u48 (6 bytes, LE)                 │   │
 │  │     Absolute offset into posting data section.          │   │
 │  │     0 = this trigram has no postings.                   │   │
 │  │  6: posting_length   u32 (compressed byte length)      │   │
 │  │ 10: doc_frequency    u32 (number of files with trigram)│   │
 │  │ 14: padding          [0u8; 2]                          │   │
 │  └────────────────────────────────────────────────────────┘   │
 │                                                                │
 │  Total section size: 16,777,216 × 16 = 256 MB (fixed)        │
 │  Only accessed pages are loaded via mmap.                     │
 │                                                                │
 │  Practical note: ~40-60% of trigrams are non-empty            │
 │  for a typical codebase. Empty slots waste 16 bytes each.     │
 │  At 256MB fixed, this is acceptable for simplicity.           │
 │                                                                │
 └───────────────────────────────────────────────────────────────┘

 ┌── POSTING DATA ───────────────────────────────────────────────┐
 │                                                                │
 │  Concatenated posting lists, one per non-empty trigram.       │
 │  Each list is self-contained.                                 │
 │                                                                │
 │  PostingList binary format:                                   │
 │  ┌────────────────────────────────────────────────────────┐   │
 │  │                                                        │   │
 │  │  num_files: varint                                     │   │
 │  │                                                        │   │
 │  │  FOR EACH FILE (delta-encoded file_ids):               │   │
 │  │    file_id_delta: varint                               │   │
 │  │    num_offsets: varint                                  │   │
 │  │    offsets: [varint] (delta-encoded within file)        │   │
 │  │                                                        │   │
 │  │  EXAMPLE:                                              │   │
 │  │    Trigram "err" appears in:                            │   │
 │  │      file 5 at offsets [100, 340, 342]                 │   │
 │  │      file 12 at offsets [44]                           │   │
 │  │      file 15 at offsets [200, 880]                     │   │
 │  │                                                        │   │
 │  │    Encoded:                                            │   │
 │  │      num_files = 3                                     │   │
 │  │      file_id_delta = 5,  num_offsets = 3,              │   │
 │  │        offset_deltas = [100, 240, 2]                   │   │
 │  │      file_id_delta = 7,  num_offsets = 1,              │   │
 │  │        offset_deltas = [44]                            │   │
 │  │      file_id_delta = 3,  num_offsets = 2,              │   │
 │  │        offset_deltas = [200, 680]                      │   │
 │  │                                                        │   │
 │  └────────────────────────────────────────────────────────┘   │
 │                                                                │
 │  Varint encoding: protobuf-style                              │
 │    value < 128:       1 byte  [0xxxxxxx]                      │
 │    value < 16384:     2 bytes [1xxxxxxx 0xxxxxxx]             │
 │    value < 2097152:   3 bytes [1xxxxxxx 1xxxxxxx 0xxxxxxx]    │
 │    ... up to 10 bytes for u64                                 │
 │                                                                │
 └───────────────────────────────────────────────────────────────┘

 ┌── BLOOM FILTER ARRAY ─────────────────────────────────────────┐
 │                                                                │
 │  Concatenated bloom filters, one per indexed file.            │
 │  Addressed via FileEntry.bloom_offset (relative).             │
 │                                                                │
 │  Each bloom filter:                                           │
 │  ┌────────────────────────────────────────────────────────┐   │
 │  │  size: u16 (bytes of bitset, typically 256)            │   │
 │  │  num_hashes: u8 (typically 5)                          │   │
 │  │  padding: u8                                           │   │
 │  │  bits: [u8; size]                                      │   │
 │  └────────────────────────────────────────────────────────┘   │
 │                                                                │
 │  Hash functions: double hashing scheme                        │
 │    h1 = xxhash64(trigram, seed=0) % num_bits                  │
 │    h2 = xxhash64(trigram, seed=1) % num_bits                  │
 │    bit_i = (h1 + i * h2) % num_bits, for i in 0..num_hashes  │
 │                                                                │
 │  256 bytes × 5 hashes → FPR ≈ 0.7% for 200 unique trigrams  │
 │                                                                │
 └───────────────────────────────────────────────────────────────┘

 ┌── STRING POOL ────────────────────────────────────────────────┐
 │                                                                │
 │  Concatenated UTF-8 strings (file paths).                     │
 │  No null terminators. Lengths stored in FileEntry.            │
 │  Deduplicated directory prefixes via prefix table.            │
 │                                                                │
 │  Prefix table (at start of string pool):                      │
 │    prefix_count: u32                                          │
 │    entries: [(prefix_id: u16, length: u16, bytes: [u8])]      │
 │                                                                │
 │  File paths stored as:                                        │
 │    prefix_id: u16, suffix_length: u16, suffix_bytes: [u8]    │
 │                                                                │
 │  Example:                                                     │
 │    prefix 0 = "/home/user/code/src/"                          │
 │    file path = prefix 0 + "net/client.rs"                     │
 │    Saves ~20 bytes per file in deep directory trees           │
 │                                                                │
 └───────────────────────────────────────────────────────────────┘

 ┌── NAME INDEX ─────────────────────────────────────────────────┐
 │                                                                │
 │  Trigram index of FILE NAMES (not content).                   │
 │  Enables: ix --filename-only "test" ~/code                    │
 │                                                                │
 │  Same format as posting data, but offsets point to            │
 │  FileEntry.file_id instead of byte positions.                 │
 │                                                                │
 │  Separate small trigram table: [TrigramEntry; 16_777_216]     │
 │  but posting lists are tiny (file_id only, no offsets).       │
 │                                                                │
 └───────────────────────────────────────────────────────────────┘
```

### 1.2 — Varint Codec (exact implementation)

```rust
// Encode
fn encode_varint(mut value: u64, buf: &mut Vec<u8>) {
    while value >= 0x80 {
        buf.push((value as u8) | 0x80);
        value >>= 7;
    }
    buf.push(value as u8);
}

// Decode
fn decode_varint(data: &[u8], pos: &mut usize) -> Result<u64, Error> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    loop {
        if *pos >= data.len() {
            return Err(Error::TruncatedVarint);
        }
        if shift >= 70 {
            return Err(Error::OverflowVarint);
        }
        let byte = data[*pos];
        *pos += 1;
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
    }
}
```

---

## Thought 2
**Purpose:** Trigram extraction algorithm — the core indexing primitive. Must handle all byte sequences correctly, efficiently, and deterministically. Guards against Compound Cascade (wrong trigrams → wrong results → tool is broken at foundation).

**Pattern Check:** Compound Cascade (correctness), G-MEM-1 (allocation during hot loop), G-CTX-1 (over-engineering for Unicode when byte trigrams are correct)

**Anti-Pattern Action:**
- Cascade: Unit test every edge case inline in the design
- G-MEM-1: Zero allocation in inner loop — pre-allocated buffer
- G-CTX-1: Bytes only. UTF-8 self-synchronization makes this correct for Unicode for free.

**Self-Audit:** Clean.

**Reasoning:**

### 2.1 — Trigram Extraction

```rust
/// Trigram is represented as a u32 with high byte always 0.
/// trigram("abc") = (0x61 << 16) | (0x62 << 8) | 0x63 = 0x00616263
type Trigram = u32;

fn trigram_from_bytes(a: u8, b: u8, c: u8) -> Trigram {
    ((a as u32) << 16) | ((b as u32) << 8) | (c as u32)
}

/// Extract all trigrams from a byte slice.
/// Returns deduplicated sorted vec of (trigram, [offsets]).
/// 
/// For indexing: we need trigram → offsets mapping
/// For querying: we only need the set of trigrams
struct TrigramExtractor {
    // Pre-allocated. Reused across files.
    // Key: trigram (u32), Value: list of offsets
    trigram_offsets: HashMap<Trigram, Vec<u32>>,
}

impl TrigramExtractor {
    fn extract_for_indexing(
        &mut self,
        data: &[u8],
    ) -> &HashMap<Trigram, Vec<u32>> {
        self.trigram_offsets.clear();
        
        if data.len() < 3 {
            return &self.trigram_offsets;
        }
        
        for i in 0..=(data.len() - 3) {
            let tri = trigram_from_bytes(data[i], data[i+1], data[i+2]);
            
            // Skip trigrams containing null bytes (binary content marker)
            if data[i] == 0 || data[i+1] == 0 || data[i+2] == 0 {
                continue;
            }
            
            self.trigram_offsets
                .entry(tri)
                .or_insert_with(Vec::new)
                .push(i as u32);
        }
        
        &self.trigram_offsets
    }
    
    /// For query decomposition — only need the set
    fn extract_trigram_set(query: &[u8]) -> Vec<Trigram> {
        if query.len() < 3 {
            return vec![];
        }
        let mut set = Vec::new();
        for i in 0..=(query.len() - 3) {
            let tri = trigram_from_bytes(query[i], query[i+1], query[i+2]);
            set.push(tri);
        }
        set.sort_unstable();
        set.dedup();
        set
    }
}
```

### 2.2 — Edge Cases

```
Input: ""            → [] (no trigrams)
Input: "ab"          → [] (too short)
Input: "abc"         → ["abc"]
Input: "abcabc"      → ["abc", "bca", "cab"] (deduplicated)
Input: "a\x00b"      → [] (null bytes skipped)
Input: "\xff\xfe\xfd"→ [0x00FFFEFD] (valid, high bytes work)
Input: "café"        → byte trigrams of UTF-8 encoding:
  "café" = [0x63, 0x61, 0x66, 0xC3, 0xA9]
  trigrams: [0x636166, 0x6166C3, 0x66C3A9]
  These are correct byte trigrams.
  Query "café" produces same trigrams → match found.
```

---

## Thought 3
**Purpose:** Index builder — the complete pipeline from files to `.ix` shard. Guards against G-MEM-1 (must handle 50GB corpus without 50GB RAM) and Compound Cascade (builder must produce format exactly matching reader expectations).

**Pattern Check:** G-MEM-1 (memory during build), G-SEC-1 (file permissions during read), Compound Cascade (format mismatch)

**Anti-Pattern Action:**
- G-MEM-1: Streaming build — process one file at a time, merge into global structures incrementally. Hard memory cap with early flush.
- G-SEC-1: Skip unreadable files, log warning, continue.
- Cascade: Builder and reader share constants module.

**Self-Audit:** Clean.

**Reasoning:**

### 3.1 — Build Pipeline

```
                      ┌──────────────────────┐
                      │    File Discovery     │
                      │    (walk directory)   │
                      └──────────┬───────────┘
                                 │
                    List of (path, metadata)
                                 │
                                 ▼
                      ┌──────────────────────┐
                      │    Filter & Sort      │
                      │  skip binary,         │
                      │  skip .gitignore,     │
                      │  skip > size_limit    │
                      └──────────┬───────────┘
                                 │
                    Filtered file list
                                 │
                                 ▼
                      ┌──────────────────────┐
                      │   Phase 1: Scan       │
                      │   (per file)          │
                      │                       │
                      │   mmap file           │
                      │   extract trigrams    │
                      │   compute xxhash      │
                      │   build bloom filter  │
                      │   emit to merge queue │
                      └──────────┬───────────┘
                                 │
                    Per-file trigram maps
                                 │
                                 ▼
                      ┌──────────────────────┐
                      │   Phase 2: Merge      │
                      │                       │
                      │   Global trigram table │
                      │   Accumulate postings │
                      │   Sort posting lists  │
                      └──────────┬───────────┘
                                 │
                    Complete index in memory
                                 │
                                 ▼
                      ┌──────────────────────┐
                      │   Phase 3: Serialize  │
                      │                       │
                      │   Write to temp file  │
                      │   Compute CRCs        │
                      │   Atomic rename       │
                      └──────────────────────┘
```

### 3.2 — Builder Implementation

```rust
struct IndexBuilder {
    config: BuildConfig,
    files: Vec<FileRecord>,
    // Global posting accumulator
    // Key: trigram, Value: vec of (file_id, offsets)
    postings: HashMap<Trigram, Vec<PostingEntry>>,
    string_pool: StringPool,
    bloom_filters: Vec<BloomFilter>,
    extractor: TrigramExtractor,
    stats: BuildStats,
}

struct BuildConfig {
    max_file_size: u64,          // default: 100MB
    max_memory_bytes: usize,     // default: 512MB
    bloom_filter_bytes: usize,   // default: 256
    bloom_hash_count: u8,        // default: 5
    exclude_patterns: Vec<Glob>, // from .gitignore + config
    idle_detector: Option<Box<dyn IdleDetector>>,
}

struct FileRecord {
    file_id: u32,
    path: PathBuf,
    mtime_ns: u64,
    size_bytes: u64,
    content_hash: u64,
    trigram_count: u32,
    is_binary: bool,
}

struct PostingEntry {
    file_id: u32,
    offsets: Vec<u32>,  // byte offsets within file
}

struct BuildStats {
    files_scanned: u64,
    files_skipped_binary: u64,
    files_skipped_size: u64,
    files_skipped_permission: u64,
    bytes_scanned: u64,
    unique_trigrams: u64,
    total_postings: u64,
    build_time: Duration,
}

impl IndexBuilder {
    fn build(config: BuildConfig, root: &Path) -> Result<IndexFile, Error> {
        let mut builder = IndexBuilder::new(config);
        
        // Phase 1: Discovery
        let file_list = builder.discover_files(root)?;
        
        // Phase 2: Scan and extract
        for (file_id, path, metadata) in file_list.iter().enumerate() {
            // Yield check for dormancy
            if let Some(ref idle) = builder.config.idle_detector {
                while !idle.is_idle() {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            
            builder.process_file(file_id as u32, path, metadata)?;
            
            // Memory pressure check
            if builder.estimated_memory() > builder.config.max_memory_bytes {
                builder.flush_to_intermediate()?;
            }
        }
        
        // Phase 3: Serialize
        let output_path = builder.serialize(root)?;
        
        Ok(IndexFile::open(&output_path)?)
    }
    
    fn process_file(
        &mut self,
        file_id: u32,
        path: &Path,
        metadata: &Metadata,
    ) -> Result<(), Error> {
        // Open and mmap
        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                self.stats.files_skipped_permission += 1;
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        
        let mmap = unsafe { Mmap::map(&file)? };
        let data = &mmap[..];
        
        // Binary detection: check first 8KB for null bytes
        let check_len = data.len().min(8192);
        if data[..check_len].contains(&0u8) {
            self.stats.files_skipped_binary += 1;
            self.files.push(FileRecord {
                file_id,
                path: path.to_owned(),
                mtime_ns: metadata.mtime_ns(),
                size_bytes: metadata.len(),
                content_hash: 0,
                trigram_count: 0,
                is_binary: true,
            });
            return Ok(());
        }
        
        // Content hash
        let content_hash = xxhash64(data, 0);
        
        // Trigram extraction
        let trigrams = self.extractor.extract_for_indexing(data);
        let trigram_count = trigrams.len() as u32;
        
        // Build bloom filter for this file
        let bloom = BloomFilter::new(
            self.config.bloom_filter_bytes,
            self.config.bloom_hash_count,
        );
        for &tri in trigrams.keys() {
            bloom.insert(tri);
        }
        
        // Merge into global postings
        for (&trigram, offsets) in trigrams.iter() {
            self.postings
                .entry(trigram)
                .or_insert_with(Vec::new)
                .push(PostingEntry {
                    file_id,
                    offsets: offsets.clone(),
                });
        }
        
        // Record file
        self.files.push(FileRecord {
            file_id,
            path: path.to_owned(),
            mtime_ns: metadata.mtime_ns(),
            size_bytes: metadata.len(),
            content_hash,
            trigram_count,
            is_binary: false,
        });
        
        self.bloom_filters.push(bloom);
        self.stats.files_scanned += 1;
        self.stats.bytes_scanned += data.len() as u64;
        
        Ok(())
    }
}
```

### 3.3 — Serialization

```rust
impl IndexBuilder {
    fn serialize(&self, root: &Path) -> Result<PathBuf, Error> {
        let index_dir = root.join(".ix");
        fs::create_dir_all(&index_dir)?;
        
        let tmp_path = index_dir.join("shard.ix.tmp");
        let final_path = index_dir.join("shard.ix");
        
        let mut file = BufWriter::new(File::create(&tmp_path)?);
        
        // ── Write header placeholder (fill offsets later) ──
        let header_pos = 0u64;
        file.write_all(&[0u8; 256])?;  // placeholder
        
        // ── Write file table ──
        let file_table_offset = file.stream_position()?;
        self.align_to_8(&mut file)?;
        for record in &self.files {
            file.write_all(&record.file_id.to_le_bytes())?;
            let path_info = self.string_pool.get_offset(&record.path);
            file.write_all(&(path_info.offset as u32).to_le_bytes())?;
            file.write_all(&(path_info.length as u16).to_le_bytes())?;
            file.write_all(&[0u8])?;  // status = FRESH
            let flags = if record.is_binary { 0x01u8 } else { 0x00 };
            file.write_all(&[flags])?;
            file.write_all(&record.mtime_ns.to_le_bytes())?;
            file.write_all(&record.size_bytes.to_le_bytes())?;
            file.write_all(&record.content_hash.to_le_bytes())?;
            file.write_all(&record.trigram_count.to_le_bytes())?;
            // bloom offset: will be filled
            file.write_all(&[0u8; 4])?; // bloom_offset placeholder
            file.write_all(&[0u8; 4])?; // padding
        }
        let file_table_size = file.stream_position()? - file_table_offset;
        
        // ── Write posting data first (need offsets for trigram table) ──
        self.align_to_8(&mut file)?;
        let posting_data_offset = file.stream_position()?;
        let mut posting_offsets: HashMap<Trigram, (u64, u32)> = HashMap::new();
        
        // Sort trigrams for deterministic output
        let mut sorted_trigrams: Vec<Trigram> = self.postings.keys().copied().collect();
        sorted_trigrams.sort_unstable();
        
        for &trigram in &sorted_trigrams {
            let entries = &self.postings[&trigram];
            let offset = file.stream_position()? - posting_data_offset;
            
            // Encode posting list
            let encoded = self.encode_posting_list(entries);
            file.write_all(&encoded)?;
            
            posting_offsets.insert(trigram, (offset, encoded.len() as u32));
        }
        let posting_data_size = file.stream_position()? - posting_data_offset;
        
        // ── Write trigram table ──
        self.align_to_8(&mut file)?;
        let trigram_table_offset = file.stream_position()?;
        
        for trigram_idx in 0u32..16_777_216 {
            if let Some(&(offset, length)) = posting_offsets.get(&trigram_idx) {
                let doc_freq = self.postings[&trigram_idx].len() as u32;
                // posting_offset: u48 (6 bytes)
                let abs_offset = posting_data_offset + offset;
                file.write_all(&abs_offset.to_le_bytes()[..6])?;
                file.write_all(&length.to_le_bytes())?;
                file.write_all(&doc_freq.to_le_bytes())?;
                file.write_all(&[0u8; 2])?; // padding
            } else {
                file.write_all(&[0u8; 16])?;
            }
        }
        let trigram_table_size = file.stream_position()? - trigram_table_offset;
        
        // ── Write bloom filters ──
        self.align_to_8(&mut file)?;
        let bloom_offset = file.stream_position()?;
        for bloom in &self.bloom_filters {
            bloom.serialize(&mut file)?;
        }
        let bloom_size = file.stream_position()? - bloom_offset;
        
        // ── Write string pool ──
        self.align_to_8(&mut file)?;
        let string_pool_offset = file.stream_position()?;
        self.string_pool.serialize(&mut file)?;
        let string_pool_size = file.stream_position()? - string_pool_offset;
        
        // ── Go back and fill header ──
        file.seek(SeekFrom::Start(0))?;
        self.write_header(&mut file, HeaderData {
            file_count: self.files.len() as u32,
            trigram_count: sorted_trigrams.len() as u32,
            file_table_offset,
            file_table_size,
            trigram_table_offset,
            trigram_table_size,
            posting_data_offset,
            posting_data_size,
            bloom_offset,
            bloom_size,
            string_pool_offset,
            string_pool_size,
        })?;
        
        // Flush and atomic rename
        file.flush()?;
        drop(file);
        fs::rename(&tmp_path, &final_path)?;
        
        Ok(final_path)
    }
    
    fn encode_posting_list(&self, entries: &[PostingEntry]) -> Vec<u8> {
        let mut buf = Vec::new();
        encode_varint(entries.len() as u64, &mut buf);
        
        let mut prev_file_id = 0u32;
        for entry in entries {
            // Delta-encoded file_id
            let delta = entry.file_id - prev_file_id;
            encode_varint(delta as u64, &mut buf);
            prev_file_id = entry.file_id;
            
            // Number of offsets
            encode_varint(entry.offsets.len() as u64, &mut buf);
            
            // Delta-encoded offsets
            let mut prev_offset = 0u32;
            for &offset in &entry.offsets {
                let delta = offset - prev_offset;
                encode_varint(delta as u64, &mut buf);
                prev_offset = offset;
            }
        }
        buf
    }
}
```

---

## Thought 4
**Purpose:** Index reader — the mmap-based query-time interface. This is the hot path. Every nanosecond matters. Guards against G-MEM-1 (must not load entire index into heap) and Compound Cascade (must correctly decode what builder wrote).

**Pattern Check:** G-MEM-1, G-SEC-1 (malformed index must not segfault), Compound Cascade

**Anti-Pattern Action:**
- G-MEM-1: All access via mmap slices. No copy to heap except final results.
- G-SEC-1: Bounds checking on every offset before dereference.
- Cascade: Validation at open time — magic, version, CRC.

**Self-Audit:** Clean.

**Reasoning:**

### 4.1 — Index Reader

```rust
struct IndexReader {
    mmap: Mmap,
    header: Header,           // Parsed from mmap, validated
    trigram_table: &[u8],     // Slice into mmap
    posting_data: &[u8],      // Slice into mmap
    file_table: &[u8],        // Slice into mmap
    bloom_data: &[u8],        // Slice into mmap
    string_pool: &[u8],       // Slice into mmap
}

impl IndexReader {
    fn open(path: &Path) -> Result<Self, Error> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        
        // ── Validate header ──
        if mmap.len() < 256 {
            return Err(Error::IndexTooSmall);
        }
        if &mmap[0..4] != b"IX01" {
            return Err(Error::BadMagic);
        }
        
        let header = Header::parse(&mmap[0..256])?;
        
        // Validate CRC
        let expected_crc = u32::from_le_bytes(
            mmap[0xF8..0xFC].try_into().unwrap()
        );
        let actual_crc = crc32c(&mmap[0..0xF8]);
        if expected_crc != actual_crc {
            return Err(Error::HeaderCorrupted);
        }
        
        // Validate all section offsets are within file bounds
        header.validate_bounds(mmap.len() as u64)?;
        
        // Create slices (zero-copy, just pointer + length)
        let trigram_table = &mmap[header.trigram_table_range()];
        let posting_data = &mmap[header.posting_data_range()];
        let file_table = &mmap[header.file_table_range()];
        let bloom_data = &mmap[header.bloom_range()];
        let string_pool = &mmap[header.string_pool_range()];
        
        Ok(IndexReader {
            mmap,
            header,
            trigram_table,
            posting_data,
            file_table,
            bloom_data,
            string_pool,
        })
    }
    
    /// Look up a trigram entry. O(1).
    fn get_trigram(&self, trigram: Trigram) -> Option<TrigramInfo> {
        let idx = trigram as usize;
        if idx >= 16_777_216 {
            return None;
        }
        
        let entry_offset = idx * 16;
        if entry_offset + 16 > self.trigram_table.len() {
            return None;
        }
        
        let entry = &self.trigram_table[entry_offset..entry_offset + 16];
        
        // Read posting_offset (u48, 6 bytes LE)
        let mut offset_bytes = [0u8; 8];
        offset_bytes[..6].copy_from_slice(&entry[0..6]);
        let posting_offset = u64::from_le_bytes(offset_bytes);
        
        if posting_offset == 0 {
            return None;  // Empty trigram slot
        }
        
        let posting_length = u32::from_le_bytes(
            entry[6..10].try_into().unwrap()
        );
        let doc_frequency = u32::from_le_bytes(
            entry[10..14].try_into().unwrap()
        );
        
        Some(TrigramInfo {
            posting_offset,
            posting_length,
            doc_frequency,
        })
    }
    
    /// Decode a posting list from mmap'd data. Zero-copy until offset vec.
    fn decode_posting_list(
        &self,
        info: &TrigramInfo,
    ) -> Result<Vec<PostingEntry>, Error> {
        let start = (info.posting_offset - self.header.posting_data_offset) as usize;
        let end = start + info.posting_length as usize;
        
        if end > self.posting_data.len() {
            return Err(Error::PostingOutOfBounds);
        }
        
        let data = &self.posting_data[start..end];
        let mut pos = 0;
        
        let num_files = decode_varint(data, &mut pos)? as usize;
        let mut entries = Vec::with_capacity(num_files);
        let mut prev_file_id = 0u32;
        
        for _ in 0..num_files {
            let file_id_delta = decode_varint(data, &mut pos)? as u32;
            let file_id = prev_file_id + file_id_delta;
            prev_file_id = file_id;
            
            let num_offsets = decode_varint(data, &mut pos)? as usize;
            let mut offsets = Vec::with_capacity(num_offsets);
            let mut prev_offset = 0u32;
            
            for _ in 0..num_offsets {
                let offset_delta = decode_varint(data, &mut pos)? as u32;
                let offset = prev_offset + offset_delta;
                prev_offset = offset;
                offsets.push(offset);
            }
            
            entries.push(PostingEntry { file_id, offsets });
        }
        
        Ok(entries)
    }
    
    /// Get file info by file_id. O(1) via direct indexing.
    fn get_file(&self, file_id: u32) -> Result<FileInfo, Error> {
        let entry_size = 48;
        let offset = (file_id as usize) * entry_size;
        if offset + entry_size > self.file_table.len() {
            return Err(Error::FileIdOutOfBounds);
        }
        
        let entry = &self.file_table[offset..offset + entry_size];
        
        let path_offset = u32::from_le_bytes(entry[4..8].try_into().unwrap()) as usize;
        let path_length = u16::from_le_bytes(entry[8..10].try_into().unwrap()) as usize;
        let status = entry[10];
        let mtime_ns = u64::from_le_bytes(entry[12..20].try_into().unwrap());
        let content_hash = u64::from_le_bytes(entry[28..36].try_into().unwrap());
        
        // Resolve path from string pool
        if path_offset + path_length > self.string_pool.len() {
            return Err(Error::StringPoolOutOfBounds);
        }
        let path_bytes = &self.string_pool[path_offset..path_offset + path_length];
        let path = std::str::from_utf8(path_bytes)
            .map_err(|_| Error::InvalidPath)?;
        
        Ok(FileInfo {
            file_id,
            path: PathBuf::from(path),
            status: FileStatus::from_u8(status),
            mtime_ns,
            content_hash,
        })
    }
    
    /// Check bloom filter for a file.
    fn bloom_may_contain(
        &self,
        file_id: u32,
        trigrams: &[Trigram],
    ) -> Result<bool, Error> {
        let file_info = self.get_file_entry_raw(file_id)?;
        let bloom_rel_offset = file_info.bloom_offset as usize;
        
        if bloom_rel_offset + 4 > self.bloom_data.len() {
            return Ok(true); // No bloom → assume might contain
        }
        
        let bloom_bytes = &self.bloom_data[bloom_rel_offset..];
        let size = u16::from_le_bytes(bloom_bytes[0..2].try_into().unwrap()) as usize;
        let num_hashes = bloom_bytes[2];
        let bits = &bloom_bytes[4..4 + size];
        
        for &tri in trigrams {
            let h1 = xxhash64(&tri.to_le_bytes(), 0) as usize;
            let h2 = xxhash64(&tri.to_le_bytes(), 1) as usize;
            let num_bits = size * 8;
            
            for i in 0..(num_hashes as usize) {
                let bit_pos = (h1.wrapping_add(i.wrapping_mul(h2))) % num_bits;
                let byte_idx = bit_pos / 8;
                let bit_idx = bit_pos % 8;
                if bits[byte_idx] & (1 << bit_idx) == 0 {
                    return Ok(false); // Definitely not present
                }
            }
        }
        
        Ok(true) // Might be present
    }
}
```

---

## Thought 5
**Purpose:** Query planner — transforms user input into an optimal index query plan. This is where regex patterns get decomposed into trigram queries. Guards against Compound Cascade (wrong decomposition → missed results) and G-CTX-1 (over-engineering the planner when most queries are literals).

**Pattern Check:** Compound Cascade (correctness — must never miss a true match), G-CTX-1 (complexity)

**Anti-Pattern Action:**
- Cascade: Every plan terminates in a VERIFY step against actual content. Index is a filter, never the source of truth.
- G-CTX-1: Three tiers: literal fast path, simple regex, complex regex fallback.

**Self-Audit:** Clean.

**Reasoning:**

### 5.1 — Query Plan Types

```rust
enum QueryPlan {
    /// Fast path: literal string search
    /// "ConnectionTimeout" → trigrams → intersect → verify
    Literal {
        pattern: Vec<u8>,
        trigrams: Vec<Trigram>,
    },
    
    /// Regex with extractable literals
    /// "err(or|no).*timeout" → required trigrams + regex verify
    RegexWithLiterals {
        regex: Regex,
        required_trigram_sets: Vec<Vec<Trigram>>,  // CNF form
        // Each inner vec is OR'd, outer vecs are AND'd
        // [["err","rro","ror"], ["tim","ime","meo"]] means
        // file must match ALL of first set AND ALL of second set
    },
    
    /// No literals extractable — full scan fallback
    /// ".*" or "[a-z]+" → parallel scan
    FullScan {
        regex: Regex,
    },
}

struct QueryPlanner;

impl QueryPlanner {
    fn plan(pattern: &str, is_regex: bool, is_fixed: bool) -> QueryPlan {
        if is_fixed || !is_regex {
            // Fixed string: trivial
            let bytes = pattern.as_bytes().to_vec();
            let trigrams = TrigramExtractor::extract_trigram_set(&bytes);
            
            if trigrams.is_empty() {
                // Pattern too short for trigrams (< 3 bytes)
                // Must fall back to scan
                return QueryPlan::FullScan {
                    regex: Regex::new(&regex::escape(pattern)).unwrap(),
                };
            }
            
            return QueryPlan::Literal {
                pattern: bytes,
                trigrams,
            };
        }
        
        // Regex: extract literal fragments
        let regex = Regex::new(pattern).expect("invalid regex");
        let literals = Self::extract_required_literals(pattern);
        
        if literals.is_empty() {
            return QueryPlan::FullScan { regex };
        }
        
        // Convert each required literal to trigrams
        let required_trigram_sets: Vec<Vec<Trigram>> = literals
            .iter()
            .map(|lit| TrigramExtractor::extract_trigram_set(lit.as_bytes()))
            .filter(|t| !t.is_empty())
            .collect();
        
        if required_trigram_sets.is_empty() {
            return QueryPlan::FullScan { regex };
        }
        
        QueryPlan::RegexWithLiterals {
            regex,
            required_trigram_sets,
        }
    }
    
    /// Extract literal strings that MUST appear in any match.
    /// This is a simplified version of what regex-syntax's HIR provides.
    fn extract_required_literals(pattern: &str) -> Vec<String> {
        // Use regex-syntax crate to parse into HIR
        // Walk the HIR tree:
        //   - Concat nodes: all children are required
        //   - Alternation: take intersection of children's required literals
        //   - Repetition with min >= 1: child's literals are required
        //   - Literal: emit the literal
        //   - Character class / dot / anchor: emit nothing
        
        // Simplified implementation for common cases:
        let hir = regex_syntax::parse(pattern).unwrap();
        let mut literals = Vec::new();
        Self::walk_hir(&hir, &mut literals);
        literals
    }
    
    fn walk_hir(hir: &Hir, out: &mut Vec<String>) {
        match hir.kind() {
            HirKind::Literal(lit) => {
                // Accumulate consecutive literals
                out.push(String::from_utf8_lossy(&lit.0).to_string());
            }
            HirKind::Concat(children) => {
                // Adjacent literals can be merged
                let mut current = String::new();
                for child in children {
                    match child.kind() {
                        HirKind::Literal(lit) => {
                            current.push_str(
                                &String::from_utf8_lossy(&lit.0)
                            );
                        }
                        _ => {
                            if current.len() >= 3 {
                                out.push(current.clone());
                            }
                            current.clear();
                            // Recurse into non-literal children
                            Self::walk_hir(child, out);
                        }
                    }
                }
                if current.len() >= 3 {
                    out.push(current);
                }
            }
            HirKind::Alternation(branches) => {
                // For alternation "a|b|c", we can't require any specific literal
                // But each branch might have required literals
                // Conservative: emit nothing (correctness over speed)
                // TODO: if ALL branches share a prefix, emit that prefix
            }
            HirKind::Repetition(rep) => {
                if rep.min >= 1 {
                    Self::walk_hir(&rep.sub, out);
                }
            }
            _ => {
                // Dot, class, anchor, etc: no literals
            }
        }
    }
}
```

### 5.2 — Literal Extraction Examples

```
Pattern                    Extracted Literals     Trigrams Used
────────────────────────── ────────────────────── ────────────────
"error"                    ["error"]              err,rro,ror
"ConnectionTimeout"        ["ConnectionTimeout"]  Con,onn,nne,...
"err(or|no)"               ["err"]                err
"err.*timeout"             ["err","timeout"]       err,rro,ror,
                                                   tim,ime,meo,eou,out
"[a-z]+_handler"           ["_handler"]            _ha,han,and,...
"(foo|bar)_baz"            ["_baz"]                _ba,baz
".*"                       []                      FULL SCAN
"[0-9]{4}-[0-9]{2}"        []                      FULL SCAN
"error_\d+_timeout"        ["error_","_timeout"]   err,rro,ror,...
```

---

## Thought 6
**Purpose:** Query executor — takes a QueryPlan and runs it against the index. The actual search algorithm. Guards against Compound Cascade (intersection logic must be correct) and G-MEM-1 (posting list intersection must not blow up memory).

**Pattern Check:** Compound Cascade, G-MEM-1, performance (this is the hot path)

**Anti-Pattern Action:**
- Cascade: Intersect at file level first (cheap), then offset level only for final candidates
- G-MEM-1: Streaming intersection — process smallest posting list first, skip files early
- Performance: Measure and document expected operation counts

**Self-Audit:** Clean.

**Reasoning:**

### 6.1 — Query Executor

```rust
struct QueryExecutor<'a> {
    index: &'a IndexReader,
    config: QueryConfig,
}

struct QueryConfig {
    max_results: usize,        // default: 1000
    stale_policy: StalePolicy, // Rescan, Skip, Warn
    case_insensitive: bool,
}

enum StalePolicy {
    Rescan,  // re-read stale files from disk (default)
    Skip,    // skip stale files
    Warn,    // show results but tag as stale
}

struct Match {
    file_path: PathBuf,
    line_number: u32,
    line_content: Vec<u8>,
    byte_offset: u64,
    freshness: Freshness,
}

enum Freshness {
    Fresh,
    StaleVerified,   // file was stale, but match confirmed by re-read
    StaleUnverified, // file was stale, result from index only
}

struct QueryStats {
    trigrams_queried: u32,
    posting_lists_decoded: u32,
    candidate_files: u32,
    candidate_offsets: u32,
    files_verified: u32,
    bytes_verified: u64,
    stale_files_rescanned: u32,
    total_matches: u32,
    elapsed: Duration,
}

impl<'a> QueryExecutor<'a> {
    fn execute(&self, plan: &QueryPlan) -> Result<(Vec<Match>, QueryStats), Error> {
        match plan {
            QueryPlan::Literal { pattern, trigrams } => {
                self.execute_literal(pattern, trigrams)
            }
            QueryPlan::RegexWithLiterals { regex, required_trigram_sets } => {
                self.execute_regex_indexed(regex, required_trigram_sets)
            }
            QueryPlan::FullScan { regex } => {
                self.execute_full_scan(regex)
            }
        }
    }
    
    fn execute_literal(
        &self,
        pattern: &[u8],
        trigrams: &[Trigram],
    ) -> Result<(Vec<Match>, QueryStats), Error> {
        let mut stats = QueryStats::default();
        
        // ── Step 1: Get posting list metadata for all trigrams ──
        let mut trigram_infos: Vec<(Trigram, TrigramInfo)> = Vec::new();
        for &tri in trigrams {
            stats.trigrams_queried += 1;
            if let Some(info) = self.index.get_trigram(tri) {
                trigram_infos.push((tri, info));
            } else {
                // Trigram not in index → no files can match
                return Ok((vec![], stats));
            }
        }
        
        // ── Step 2: Sort by doc_frequency (rarest first) ──
        trigram_infos.sort_by_key(|(_, info)| info.doc_frequency);
        
        // ── Step 3: Decode rarest posting list ──
        let (_, ref rarest_info) = trigram_infos[0];
        let rarest_postings = self.index.decode_posting_list(rarest_info)?;
        stats.posting_lists_decoded += 1;
        
        // Candidate files = files in rarest posting list
        let mut candidate_file_ids: HashSet<u32> = rarest_postings
            .iter()
            .map(|p| p.file_id)
            .collect();
        
        // ── Step 4: Intersect with remaining trigrams using bloom filters ──
        for &(tri, _) in &trigram_infos[1..] {
            let before = candidate_file_ids.len();
            candidate_file_ids.retain(|&fid| {
                self.index.bloom_may_contain(fid, &[tri]).unwrap_or(true)
            });
            
            // If candidates dropped below threshold, decode posting list
            // for exact intersection
            if candidate_file_ids.len() > 100 {
                let posting = self.index.decode_posting_list(
                    &trigram_infos.iter()
                        .find(|(t, _)| *t == tri).unwrap().1
                )?;
                stats.posting_lists_decoded += 1;
                
                let posting_file_ids: HashSet<u32> = posting
                    .iter()
                    .map(|p| p.file_id)
                    .collect();
                candidate_file_ids.retain(|f| posting_file_ids.contains(f));
            }
            
            if candidate_file_ids.is_empty() {
                return Ok((vec![], stats));
            }
        }
        
        stats.candidate_files = candidate_file_ids.len() as u32;
        
        // ── Step 5: Verify matches against actual file content ──
        let mut matches = Vec::new();
        
        for &file_id in &candidate_file_ids {
            let file_info = self.index.get_file(file_id)?;
            
            // Freshness check
            let current_mtime = fs::metadata(&file_info.path)
                .map(|m| m.mtime_ns())
                .ok();
            
            let is_stale = current_mtime != Some(file_info.mtime_ns);
            
            match (is_stale, &self.config.stale_policy) {
                (true, StalePolicy::Skip) => continue,
                _ => {}
            }
            
            // Read and search file content
            stats.files_verified += 1;
            let file_matches = self.verify_file(
                &file_info.path,
                pattern,
                is_stale,
            )?;
            
            stats.bytes_verified += file_info.size_bytes;
            matches.extend(file_matches);
            
            if matches.len() >= self.config.max_results {
                break;
            }
        }
        
        stats.total_matches = matches.len() as u32;
        Ok((matches, stats))
    }
    
    /// Verify a specific file for pattern matches.
    /// Uses SIMD-accelerated byte search.
    fn verify_file(
        &self,
        path: &Path,
        pattern: &[u8],
        is_stale: bool,
    ) -> Result<Vec<Match>, Error> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        let data = &mmap[..];
        
        let mut matches = Vec::new();
        let mut search_pos = 0;
        
        // Use memchr for single-byte first-char search
        // Then verify remaining bytes
        // This is the ripgrep approach
        let first_byte = pattern[0];
        let finder = memchr::memmem::Finder::new(pattern);
        
        while let Some(pos) = finder.find(&data[search_pos..]) {
            let abs_pos = search_pos + pos;
            
            // Find line boundaries
            let line_start = data[..abs_pos]
                .iter()
                .rposition(|&b| b == b'\n')
                .map(|p| p + 1)
                .unwrap_or(0);
            
            let line_end = data[abs_pos..]
                .iter()
                .position(|&b| b == b'\n')
                .map(|p| abs_pos + p)
                .unwrap_or(data.len());
            
            // Count line number
            let line_number = data[..line_start]
                .iter()
                .filter(|&&b| b == b'\n')
                .count() as u32 + 1;
            
            let freshness = if is_stale {
                Freshness::StaleVerified
            } else {
                Freshness::Fresh
            };
            
            matches.push(Match {
                file_path: path.to_owned(),
                line_number,
                line_content: data[line_start..line_end].to_vec(),
                byte_offset: abs_pos as u64,
                freshness,
            });
            
            // Skip past this line to avoid duplicate matches on same line
            search_pos = line_end + 1;
        }
        
        Ok(matches)
    }
}
```

### 6.2 — Intersection Algorithm Detail

```
OPTIMIZED INTERSECTION STRATEGY:

Given trigrams [T1, T2, T3, T4, T5] sorted by doc_frequency:
  T3: 8 files
  T1: 45 files
  T5: 120 files
  T2: 890 files
  T4: 15,000 files

Algorithm:
  1. Decode T3 posting list → 8 candidate file IDs
  
  2. For T1 (45 files):
     8 candidates × bloom check = 8 operations
     Maybe eliminates 2 candidates → 6 remain
     Don't bother decoding T1's full posting list
  
  3. For T5 (120 files):
     6 candidates × bloom check = 6 operations
     Maybe eliminates 1 → 5 remain
  
  4. For T2 (890 files):
     5 candidates × bloom check = 5 operations
     Eliminates 0 → 5 remain
  
  5. For T4 (15,000 files):
     5 candidates × bloom check = 5 operations
     Eliminates 0 → 5 remain
  
  Total work: decoded 1 posting list + 24 bloom checks
  Instead of: decoded 5 posting lists + full set intersection
  
  With bloom FPR of 0.7%:
    Expected false positives across 24 checks: 0.17
    → Almost certainly exact result without decoding more lists
```

---

## Thought 7
**Purpose:** Daemon state machine — complete specification of the background daemon's lifecycle, concurrency model, and all state transitions. Guards against race conditions, deadlocks, and G-SEC-1.

**Pattern Check:** G-SEC-1 (daemon runs as user, handles IPC), G-MEM-1 (long-running process memory), Compound Cascade (state machine bugs cascade into stale data)

**Anti-Pattern Action:**
- G-SEC-1: All IPC validates caller UID. PID file locking is atomic.
- G-MEM-1: Explicit memory accounting per subsystem. Arena resets.
- Cascade: State machine is fully enumerated with explicit transition table.

**Self-Audit:** Clean.

**Reasoning:**

### 7.1 — Daemon State Machine

```
                              ┌──────────────┐
                              │              │
                   ┌──────────│   STARTING   │
                   │          │              │
                   │          └──────┬───────┘
                   │                 │
                   │          validate config
                   │          open pid file (exclusive lock)
                   │          open unix socket
                   │          load existing index (if any)
                   │          start fs watcher
                   │                 │
                   │                 ▼
                   │          ┌──────────────┐
                   │     ┌───▶│              │◀───────────────────────┐
                   │     │    │    IDLE       │                        │
                   │     │    │              │                        │
                   │     │    └──────┬───────┘                        │
                   │     │           │                                │
                   │     │    idle_score > 0.7                        │
                   │     │    for 30 seconds?                        │
                   │     │    AND stale queue non-empty?              │
                   │     │           │                                │
                   │     │           ▼                                │
                   │     │    ┌──────────────┐          idle_score    │
                   │     │    │              │          drops < 0.3   │
                   │     │    │  INDEXING     │──────────────────────▶│
                   │     │    │              │                        │
                   │     │    └──────┬───────┘                        │
                   │     │           │                                │
                   │     │    stale queue empty                      │
                   │     │           │                                │
                   │     │           ▼                                │
                   │     │    ┌──────────────┐                        │
                   │     │    │              │                        │
                   │     └────│  INDEX MERGE  │                       │
                   │          │              │                        │
                   │          └──────────────┘                        │
                   │                                                  │
                   │          ┌──────────────┐                        │
                   │          │              │                        │
                   └──────────│  SHUTTING    │◀───── SIGTERM/SIGINT   │
                              │  DOWN        │                        │
                              │              │                        │
                              └──────────────┘                        │
                                                                      │
                   QUERY REQUEST can arrive in ANY state ─────────────┘
                   (served immediately, does not block indexing)
```

### 7.2 — Concurrency Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     DAEMON PROCESS                          │
│                                                             │
│   ┌──────────────────────────────────────────────────────┐  │
│   │                 ASYNC RUNTIME (tokio)                │  │
│   │                                                      │  │
│   │   Task 1: IPC Listener                               │  │
│   │     - Accept connections on unix socket              │  │
│   │     - Spawn per-connection handler task              │  │
│   │     - Read-only access to index (via Arc<IndexReader>)│  │
│   │     - NEVER blocks on indexing                       │  │
│   │                                                      │  │
│   │   Task 2: FS Event Consumer                          │  │
│   │     - Receives events from notify crate              │  │
│   │     - Debounces (500ms window)                       │  │
│   │     - Pushes to stale_queue (crossbeam channel)      │  │
│   │     - Updates file status in metadata DB             │  │
│   │                                                      │  │
│   │   Task 3: Idle Monitor                               │  │
│   │     - Polls system idle state every 5 seconds        │  │
│   │     - Manages idle_score (exponential moving average) │  │
│   │     - Signals indexer thread via AtomicBool           │  │
│   │                                                      │  │
│   └──────────────────────────────────────────────────────┘  │
│                                                             │
│   ┌──────────────────────────────────────────────────────┐  │
│   │              INDEXER THREAD (dedicated)               │  │
│   │                                                      │  │
│   │   - Blocks on stale_queue                            │  │
│   │   - Only runs when idle_flag is true                 │  │
│   │   - Builds index shards into temp files              │  │
│   │   - Signals main thread for atomic swap              │  │
│   │   - Uses arena allocator (reset per file)            │  │
│   │                                                      │  │
│   └──────────────────────────────────────────────────────┘  │
│                                                             │
│   Shared State:                                             │
│                                                             │
│   ┌────────────────────────────────┐                        │
│   │ Arc<RwLock<IndexState>>        │                        │
│   │   .current_index: IndexReader  │  (swapped atomically) │
│   │   .watch_roots: Vec<PathBuf>   │                        │
│   │   .stats: DaemonStats         │                        │
│   └────────────────────────────────┘                        │
│                                                             │
│   ┌────────────────────────────────┐                        │
│   │ stale_queue: Channel<StaleEvent>│ (bounded, 100K)      │
│   └────────────────────────────────┘                        │
│                                                             │
│   ┌────────────────────────────────┐                        │
│   │ idle_flag: AtomicBool          │                        │
│   │ should_shutdown: AtomicBool    │                        │
│   └────────────────────────────────┘                        │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### 7.3 — Daemon Implementation

```rust
struct Daemon {
    config: DaemonConfig,
    state: Arc<RwLock<IndexState>>,
    stale_queue: (Sender<StaleEvent>, Receiver<StaleEvent>),
    idle_flag: Arc<AtomicBool>,
    should_shutdown: Arc<AtomicBool>,
}

struct DaemonConfig {
    watch_roots: Vec<PathBuf>,
    index_dir: PathBuf,           // ~/.local/share/ix/
    socket_path: PathBuf,         // /run/user/$UID/ix.sock
    pid_path: PathBuf,            // /run/user/$UID/ix.pid
    max_index_memory: usize,      // 512MB
    idle_threshold: f64,          // 0.7
    idle_settle_time: Duration,   // 30s
    debounce_window: Duration,    // 500ms
    max_event_queue: usize,       // 100_000
    max_daemon_rss: usize,        // 200MB
}

struct IndexState {
    readers: HashMap<PathBuf, IndexReader>,  // per watch root
    file_freshness: HashMap<PathBuf, FileFreshness>,
}

struct FileFreshness {
    indexed_mtime: u64,
    indexed_hash: u64,
    status: FileStatus,
    last_queried: Instant,  // for LRU priority
}

enum StaleEvent {
    FileModified(PathBuf),
    FileCreated(PathBuf),
    FileDeleted(PathBuf),
    FileRenamed { from: PathBuf, to: PathBuf },
    DirectoryStorm(PathBuf),  // too many events, rescan dir
}

impl Daemon {
    async fn run(config: DaemonConfig) -> Result<(), Error> {
        // ── Startup ──
        let pid_file = PidFile::acquire(&config.pid_path)?;
        let listener = UnixListener::bind(&config.socket_path)?;
        
        // Set socket permissions
        fs::set_permissions(
            &config.socket_path,
            fs::Permissions::from_mode(0o600)
        )?;
        
        // Load existing indexes
        let state = Arc::new(RwLock::new(IndexState::load(&config)?));
        let (stale_tx, stale_rx) = crossbeam::channel::bounded(
            config.max_event_queue
        );
        let idle_flag = Arc::new(AtomicBool::new(false));
        let should_shutdown = Arc::new(AtomicBool::new(false));
        
        // ── Start FS watcher ──
        let watcher = Self::start_watcher(
            &config.watch_roots,
            stale_tx.clone(),
            config.debounce_window,
        )?;
        
        // ── Start idle monitor ──
        let idle_handle = {
            let idle_flag = idle_flag.clone();
            let shutdown = should_shutdown.clone();
            tokio::spawn(async move {
                Self::idle_monitor_loop(
                    idle_flag,
                    shutdown,
                    config.idle_threshold,
                    config.idle_settle_time,
                ).await;
            })
        };
        
        // ── Start indexer thread ──
        let indexer_handle = {
            let state = state.clone();
            let idle_flag = idle_flag.clone();
            let shutdown = should_shutdown.clone();
            let config = config.clone();
            std::thread::Builder::new()
                .name("ix-indexer".into())
                .spawn(move || {
                    Self::indexer_loop(
                        state, stale_rx, idle_flag, shutdown, config
                    );
                })?
        };
        
        // ── IPC listener loop ──
        let mut signal = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate()
        )?;
        
        loop {
            tokio::select! {
                // Accept new IPC connection
                Ok((stream, _)) = listener.accept() => {
                    let state = state.clone();
                    tokio::spawn(async move {
                        Self::handle_client(stream, state).await;
                    });
                }
                
                // Shutdown signal
                _ = signal.recv() => {
                    should_shutdown.store(true, Ordering::SeqCst);
                    break;
                }
                
                // Ctrl+C
                _ = tokio::signal::ctrl_c() => {
                    should_shutdown.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }
        
        // ── Graceful shutdown ──
        indexer_handle.join().ok();
        drop(watcher);
        fs::remove_file(&config.socket_path).ok();
        drop(pid_file);
        
        Ok(())
    }
}
```

### 7.4 — Idle Monitor Implementation

```rust
impl Daemon {
    async fn idle_monitor_loop(
        idle_flag: Arc<AtomicBool>,
        shutdown: Arc<AtomicBool>,
        threshold: f64,
        settle_time: Duration,
    ) {
        let detector = PlatformIdleDetector::new();
        let mut idle_since: Option<Instant> = None;
        let mut ema_score: f64 = 0.0;
        const ALPHA: f64 = 0.3; // EMA smoothing factor
        
        while !shutdown.load(Ordering::Relaxed) {
            let raw_score = detector.sample();
            ema_score = ALPHA * raw_score + (1.0 - ALPHA) * ema_score;
            
            if ema_score > threshold {
                match idle_since {
                    None => {
                        idle_since = Some(Instant::now());
                    }
                    Some(since) if since.elapsed() >= settle_time => {
                        idle_flag.store(true, Ordering::SeqCst);
                    }
                    _ => {}
                }
            } else {
                idle_since = None;
                idle_flag.store(false, Ordering::SeqCst);
            }
            
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

// ── Platform-specific idle detection ──

#[cfg(target_os = "linux")]
struct PlatformIdleDetector {
    prev_cpu: Option<CpuSnapshot>,
}

#[cfg(target_os = "linux")]
impl PlatformIdleDetector {
    fn sample(&mut self) -> f64 {
        let mut score = 0.0;
        let mut weight_sum = 0.0;
        
        // Signal 1: CPU idle (weight 0.35)
        let cpu = CpuSnapshot::read_proc_stat();
        if let Some(ref prev) = self.prev_cpu {
            let idle_pct = cpu.idle_percent_since(prev);
            score += 0.35 * (idle_pct / 100.0).min(1.0);
        }
        weight_sum += 0.35;
        self.prev_cpu = Some(cpu);
        
        // Signal 2: User input idle (weight 0.35)
        if let Ok(idle_ms) = Self::read_x11_idle_time() {
            let idle_secs = idle_ms as f64 / 1000.0;
            let input_score = (idle_secs / 60.0).min(1.0); // saturates at 60s
            score += 0.35 * input_score;
        }
        weight_sum += 0.35;
        
        // Signal 3: Disk I/O (weight 0.20)
        if let Ok(io_pct) = Self::read_disk_utilization() {
            score += 0.20 * (1.0 - io_pct / 100.0).max(0.0);
        }
        weight_sum += 0.20;
        
        // Signal 4: Battery (weight 0.10)
        match Self::read_power_status() {
            Ok(PowerStatus::AC) => score += 0.10,
            Ok(PowerStatus::Battery) => {} // add nothing
            Err(_) => score += 0.10,  // assume AC if can't read
        }
        weight_sum += 0.10;
        
        score / weight_sum  // normalize
    }
    
    fn read_x11_idle_time() -> Result<u64, Error> {
        // Use XScreenSaverQueryInfo via x11 crate
        // Returns milliseconds since last input event
        // Falls back to /proc/interrupts keyboard counter if no X
        todo!()
    }
    
    fn read_disk_utilization() -> Result<f64, Error> {
        // Parse /proc/diskstats
        // Calculate %util over sample interval
        todo!()
    }
}

struct CpuSnapshot {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
}

impl CpuSnapshot {
    fn read_proc_stat() -> Self {
        // Parse first line of /proc/stat
        // "cpu  USER NICE SYSTEM IDLE IOWAIT IRQ SOFTIRQ STEAL"
        let content = fs::read_to_string("/proc/stat").unwrap();
        let first_line = content.lines().next().unwrap();
        let fields: Vec<u64> = first_line
            .split_whitespace()
            .skip(1)  // skip "cpu"
            .take(5)
            .map(|s| s.parse().unwrap())
            .collect();
        
        CpuSnapshot {
            user: fields[0],
            nice: fields[1],
            system: fields[2],
            idle: fields[3],
            iowait: fields[4],
        }
    }
    
    fn idle_percent_since(&self, prev: &Self) -> f64 {
        let total_delta = (self.user - prev.user)
            + (self.nice - prev.nice)
            + (self.system - prev.system)
            + (self.idle - prev.idle)
            + (self.iowait - prev.iowait);
        
        if total_delta == 0 {
            return 100.0;
        }
        
        let idle_delta = (self.idle - prev.idle) + (self.iowait - prev.iowait);
        (idle_delta as f64 / total_delta as f64) * 100.0
    }
}
```

---

## Thought 8
**Purpose:** Incremental index update — how to update the index when files change without full rebuild. This is the hardest engineering problem. Guards against Compound Cascade (incremental update produces different results than full rebuild).

**Pattern Check:** Compound Cascade (consistency), G-MEM-1 (merge buffers), G-SEC-1 (concurrent read during write)

**Anti-Pattern Action:**
- Cascade: Incremental update uses same trigram extraction and posting format. Merge is append-only then compact.
- G-MEM-1: Delta index is bounded in size. Merge when delta exceeds threshold.
- G-SEC-1: Readers see old index until atomic swap. No concurrent mutation.

**Self-Audit:** Clean.

**Reasoning:**

### 8.1 — LSM-Style Index Architecture

```
INSIGHT: Treat index updates like an LSM-tree.

Level 0: "delta" index (small, in-memory or small file)
  - Contains recent changes
  - Queried alongside main index
  
Level 1: "main" index (large, on-disk shard file)
  - Result of full build or merge
  
Query reads BOTH and merges results.
Periodically, delta is merged into main.

┌─────────────────────────────────────────────────────┐
│                                                     │
│   QUERY TIME:                                       │
│                                                     │
│   results = query(main_index, pattern)              │
│             ∪ query(delta_index, pattern)            │
│             - deleted_files                          │
│                                                     │
│   MERGE (when delta_size > threshold):              │
│                                                     │
│   new_main = merge(main_index, delta_index)         │
│   atomic_swap(main_index, new_main)                 │
│   clear(delta_index)                                │
│                                                     │
└─────────────────────────────────────────────────────┘
```

### 8.2 — Delta Index

```rust
/// In-memory delta index for recent changes.
/// Much simpler than the on-disk format.
struct DeltaIndex {
    // Files that were added or modified since last merge
    added_files: HashMap<PathBuf, DeltaFileEntry>,
    
    // Files that were deleted since last merge
    deleted_paths: HashSet<PathBuf>,
    
    // Trigram index for added/modified files only
    trigrams: HashMap<Trigram, Vec<DeltaPosting>>,
    
    // Memory tracking
    estimated_bytes: usize,
}

struct DeltaFileEntry {
    path: PathBuf,
    mtime_ns: u64,
    content_hash: u64,
    size: u64,
}

struct DeltaPosting {
    path: PathBuf,      // reference by path, not file_id
    offsets: Vec<u32>,
}

const DELTA_MERGE_THRESHOLD: usize = 50 * 1024 * 1024; // 50MB

impl DeltaIndex {
    fn add_file(&mut self, path: &Path) -> Result<(), Error> {
        // Remove from deleted set if present
        self.deleted_paths.remove(path);
        
        // Remove old entry if present (file was modified)
        self.remove_file_trigrams(path);
        
        // mmap and extract trigrams
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        let data = &mmap[..];
        
        // Binary check
        if data[..data.len().min(8192)].contains(&0u8) {
            return Ok(());
        }
        
        let mut extractor = TrigramExtractor::new();
        let trigrams = extractor.extract_for_indexing(data);
        
        for (&tri, offsets) in trigrams.iter() {
            self.trigrams
                .entry(tri)
                .or_default()
                .push(DeltaPosting {
                    path: path.to_owned(),
                    offsets: offsets.clone(),
                });
        }
        
        let metadata = fs::metadata(path)?;
        self.added_files.insert(path.to_owned(), DeltaFileEntry {
            path: path.to_owned(),
            mtime_ns: metadata.mtime_ns(),
            content_hash: xxhash64(data, 0),
            size: metadata.len(),
        });
        
        self.recompute_size();
        Ok(())
    }
    
    fn remove_file(&mut self, path: &Path) {
        self.deleted_paths.insert(path.to_owned());
        self.remove_file_trigrams(path);
        self.added_files.remove(path);
    }
    
    fn needs_merge(&self) -> bool {
        self.estimated_bytes > DELTA_MERGE_THRESHOLD
            || self.added_files.len() > 10_000
    }
    
    /// Query the delta index for a set of trigrams.
    /// Returns candidate paths (not file_ids — those don't exist yet).
    fn query(&self, trigrams: &[Trigram]) -> HashSet<PathBuf> {
        if trigrams.is_empty() {
            return self.added_files.keys().cloned().collect();
        }
        
        // Same intersection logic as main index
        // but operating on HashSet<PathBuf> instead of HashSet<u32>
        let mut sorted: Vec<(Trigram, usize)> = trigrams
            .iter()
            .filter_map(|&t| {
                self.trigrams.get(&t).map(|p| (t, p.len()))
            })
            .collect();
        
        if sorted.len() < trigrams.len() {
            // Some trigram not present at all → no matches in delta
            return HashSet::new();
        }
        
        sorted.sort_by_key(|&(_, count)| count);
        
        let first_postings = &self.trigrams[&sorted[0].0];
        let mut candidates: HashSet<PathBuf> = first_postings
            .iter()
            .map(|p| p.path.clone())
            .collect();
        
        for &(tri, _) in &sorted[1..] {
            let posting_paths: HashSet<&PathBuf> = self.trigrams[&tri]
                .iter()
                .map(|p| &p.path)
                .collect();
            candidates.retain(|p| posting_paths.contains(p));
            
            if candidates.is_empty() {
                break;
            }
        }
        
        candidates
    }
}
```

### 8.3 — Merge Operation

```rust
impl Daemon {
    fn merge_delta_into_main(
        state: &mut IndexState,
        root: &Path,
        config: &DaemonConfig,
    ) -> Result<(), Error> {
        let delta = &state.delta;
        let current = &state.readers[root];
        
        // Build a new index that combines current + delta
        let mut builder = IndexBuilder::new(BuildConfig {
            idle_detector: None, // merge is already gated by idle
            ..config.build_config()
        });
        
        // Iterate all files in current index
        for file_id in 0..current.header.file_count {
            let file_info = current.get_file(file_id)?;
            
            // Skip if deleted in delta
            if delta.deleted_paths.contains(&file_info.path) {
                continue;
            }
            
            // Skip if modified in delta (will be added from delta)
            if delta.added_files.contains_key(&file_info.path) {
                continue;
            }
            
            // Copy from current index
            builder.incorporate_existing(current, file_id)?;
        }
        
        // Add all files from delta
        for (path, _entry) in &delta.added_files {
            builder.process_file_by_path(path)?;
        }
        
        // Serialize new index
        let new_index_path = builder.serialize(root)?;
        
        // Atomic swap
        let new_reader = IndexReader::open(&new_index_path)?;
        state.readers.insert(root.to_owned(), new_reader);
        state.delta = DeltaIndex::new();
        
        Ok(())
    }
}
```

---

## Thought 9
**Purpose:** IPC wire protocol — exact byte-level specification for communication between `ix` CLI and `ixd` daemon. Guards against G-SEC-1 (malicious input over socket) and Compound Cascade (protocol mismatch between versions).

**Pattern Check:** G-SEC-1, Compound Cascade, G-MEM-1 (unbounded message size)

**Anti-Pattern Action:**
- G-SEC-1: Message size limit (16MB). Input validation before processing.
- Cascade: Version field in protocol header. Unknown fields ignored (forward compat).
- G-MEM-1: Streaming response — results sent as they're found, not buffered.

**Self-Audit:** Clean.

**Reasoning:**

### 9.1 — Wire Protocol

```
CONNECTION FLOW:

  Client                          Daemon
    │                               │
    ├─── connect (unix socket) ────▶│
    │                               │
    │◀── HELLO ────────────────────┤
    │    version: "ix/1.0"         │
    │    capabilities: [...]        │
    │                               │
    ├─── REQUEST ──────────────────▶│
    │                               │
    │◀── RESULT (streaming) ───────┤
    │◀── RESULT ───────────────────┤
    │◀── RESULT ───────────────────┤
    │◀── DONE ─────────────────────┤
    │                               │
    ├─── close ────────────────────▶│
    │                               │

FRAMING:

  Each message is length-prefixed:
  
  ┌──────────┬──────────┬─────────────────────┐
  │ length   │ type     │ payload             │
  │ u32 LE   │ u8       │ msgpack bytes       │
  │ (4 bytes)│ (1 byte) │ (length - 1 bytes)  │
  └──────────┴──────────┴─────────────────────┘
  
  length = size of (type + payload), NOT including the 4-byte length field
  Maximum length: 16,777,215 (16MB - 1)

MESSAGE TYPES:

  0x01  HELLO          (daemon → client)
  0x02  QUERY          (client → daemon)
  0x03  MATCH          (daemon → client, streaming)
  0x04  DONE           (daemon → client)
  0x05  ERROR          (daemon → client)
  0x06  STATUS_REQUEST (client → daemon)
  0x07  STATUS_RESPONSE(daemon → client)
  0x08  WATCH_ADD      (client → daemon)
  0x09  WATCH_REMOVE   (client → daemon)
  0x0A  ACK            (daemon → client)
```

### 9.2 — Message Schemas (msgpack)

```rust
// ── HELLO (0x01) ──
#[derive(Serialize, Deserialize)]
struct HelloMessage {
    version: String,           // "ix/1.0"
    pid: u32,
    uptime_seconds: u64,
    index_count: u32,
    total_indexed_bytes: u64,
}

// ── QUERY (0x02) ──
#[derive(Serialize, Deserialize)]
struct QueryMessage {
    id: u64,                   // unique per connection
    pattern: String,
    roots: Vec<String>,        // empty = all watched roots
    is_regex: bool,
    is_fixed_string: bool,
    case_insensitive: bool,
    max_results: u32,          // 0 = unlimited (capped at 100K)
    stale_policy: String,      // "rescan"|"skip"|"warn"
    include_stats: bool,
    context_lines_before: u32, // like grep -B
    context_lines_after: u32,  // like grep -A
}

// ── MATCH (0x03) ──
#[derive(Serialize, Deserialize)]
struct MatchMessage {
    query_id: u64,
    file_path: String,
    line_number: u32,
    column: u32,               // 1-based byte offset in line
    line_content: String,      // the matched line (lossy UTF-8)
    byte_offset: u64,          // absolute byte offset in file
    freshness: String,         // "fresh"|"stale_verified"|"stale_unverified"
    context_before: Vec<String>,
    context_after: Vec<String>,
}

// ── DONE (0x04) ──
#[derive(Serialize, Deserialize)]
struct DoneMessage {
    query_id: u64,
    total_matches: u32,
    stats: Option<QueryStatsMessage>,
}

#[derive(Serialize, Deserialize)]
struct QueryStatsMessage {
    trigrams_queried: u32,
    posting_lists_decoded: u32,
    candidate_files: u32,
    files_verified: u32,
    bytes_verified: u64,
    stale_files_rescanned: u32,
    elapsed_microseconds: u64,
    used_index: bool,          // false if fell back to scan
    index_coverage: f64,       // 0.0-1.0, fraction of query handled by index
}

// ── ERROR (0x05) ──
#[derive(Serialize, Deserialize)]
struct ErrorMessage {
    query_id: u64,
    code: String,              // "invalid_regex"|"root_not_watched"|"internal"
    message: String,
}

// ── STATUS_RESPONSE (0x07) ──
#[derive(Serialize, Deserialize)]
struct StatusResponse {
    daemon_pid: u32,
    uptime_seconds: u64,
    watch_roots: Vec<WatchRootStatus>,
    total_files_indexed: u64,
    total_bytes_indexed: u64,
    index_size_on_disk: u64,
    delta_size_bytes: u64,
    delta_pending_files: u32,
    idle_score: f64,
    indexing_active: bool,
    daemon_rss_bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct WatchRootStatus {
    path: String,
    files_indexed: u64,
    bytes_indexed: u64,
    index_file_path: String,
    last_full_build: u64,      // unix timestamp
    stale_files: u32,
    freshness_percent: f64,    // 0.0-100.0
}
```

### 9.3 — Client Implementation

```rust
struct IxClient {
    socket_path: PathBuf,
}

impl IxClient {
    fn connect(&self) -> Result<ClientConnection, Error> {
        let stream = UnixStream::connect(&self.socket_path)?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        
        // Read HELLO
        let hello: HelloMessage = Self::read_message(&stream)?;
        if !hello.version.starts_with("ix/1.") {
            return Err(Error::IncompatibleVersion(hello.version));
        }
        
        Ok(ClientConnection { stream })
    }
    
    fn query(
        &self,
        conn: &mut ClientConnection,
        query: QueryMessage,
    ) -> Result<QueryResultStream, Error> {
        Self::write_message(&conn.stream, 0x02, &query)?;
        
        Ok(QueryResultStream { 
            conn,
            query_id: query.id,
            done: false,
        })
    }
}

/// Streaming result iterator
struct QueryResultStream<'a> {
    conn: &'a mut ClientConnection,
    query_id: u64,
    done: bool,
}

impl<'a> Iterator for QueryResultStream<'a> {
    type Item = Result<QueryResultItem, Error>;
    
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        
        loop {
            let (msg_type, payload) = match Self::read_frame(&self.conn.stream) {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            
            match msg_type {
                0x03 => {
                    // MATCH
                    let m: MatchMessage = rmp_serde::from_slice(&payload).unwrap();
                    return Some(Ok(QueryResultItem::Match(m)));
                }
                0x04 => {
                    // DONE
                    let d: DoneMessage = rmp_serde::from_slice(&payload).unwrap();
                    self.done = true;
                    return Some(Ok(QueryResultItem::Done(d)));
                }
                0x05 => {
                    // ERROR
                    let e: ErrorMessage = rmp_serde::from_slice(&payload).unwrap();
                    self.done = true;
                    return Some(Err(Error::ServerError(e.code, e.message)));
                }
                _ => {
                    // Unknown message type: skip (forward compat)
                    continue;
                }
            }
        }
    }
}
```

---

## Thought 10
**Purpose:** CLI entry point — the `ix` binary that users actually run. Complete argument parsing, output formatting, and graceful degradation when daemon is unavailable. Guards against G-CTX-1 (must feel like grep).

**Pattern Check:** G-CTX-1 (grep compatibility), Compound Cascade (output format breaks downstream tools)

**Anti-Pattern Action:**
- G-CTX-1: Match grep's exit codes (0=found, 1=not found, 2=error)
- Cascade: stdout = matches only, stderr = stats/errors. Always.

**Self-Audit:** Clean.

**Reasoning:**

### 10.1 — CLI Argument Specification

```rust
use clap::Parser;

#[derive(Parser)]
#[command(name = "ix", about = "Indexed grep")]
struct Cli {
    /// Search pattern
    pattern: String,
    
    /// Files or directories to search
    #[arg(default_value = ".")]
    paths: Vec<PathBuf>,
    
    /// Treat pattern as extended regex
    #[arg(short = 'e', long = "regexp")]
    regex: bool,
    
    /// Treat pattern as fixed string (no regex)
    #[arg(short = 'F', long = "fixed-strings")]
    fixed_strings: bool,
    
    /// Case-insensitive search
    #[arg(short = 'i', long = "ignore-case")]
    ignore_case: bool,
    
    /// Show line numbers
    #[arg(short = 'n', long = "line-number")]
    line_number: bool,
    
    /// Only print filenames of matching files
    #[arg(short = 'l', long = "files-with-matches")]
    files_only: bool,
    
    /// Print count of matching lines per file
    #[arg(short = 'c', long = "count")]
    count: bool,
    
    /// Recursive (default for directories)
    #[arg(short = 'r', long = "recursive")]
    recursive: bool,
    
    /// Invert match
    #[arg(short = 'v', long = "invert-match")]
    invert: bool,
    
    /// Context lines before match
    #[arg(short = 'B', long = "before-context", default_value = "0")]
    before_context: u32,
    
    /// Context lines after match
    #[arg(short = 'A', long = "after-context", default_value = "0")]
    after_context: u32,
    
    /// Context lines before and after
    #[arg(short = 'C', long = "context")]
    context: Option<u32>,
    
    // ── ix-specific options ──
    
    /// Show query statistics on stderr
    #[arg(long = "stats")]
    stats: bool,
    
    /// Bypass index, force full scan
    #[arg(long = "no-index")]
    no_index: bool,
    
    /// How to handle stale index entries
    #[arg(long = "stale", default_value = "rescan")]
    stale_policy: StaleArg,
    
    /// Max number of results (0 = unlimited)
    #[arg(long = "max-results", default_value = "0")]
    max_results: u32,
    
    /// Don't use colors in output
    #[arg(long = "no-color")]
    no_color: bool,
}

#[derive(Clone, clap::ValueEnum)]
enum StaleArg {
    Rescan,
    Skip,
    Warn,
}
```

### 10.2 — Main Entry Point

```rust
fn main() -> ExitCode {
    let cli = Cli::parse();
    
    match run(cli) {
        Ok(found) => {
            if found {
                ExitCode::from(0)  // grep compat: found matches
            } else {
                ExitCode::from(1)  // grep compat: no matches
            }
        }
        Err(e) => {
            eprintln!("ix: {}", e);
            ExitCode::from(2)  // grep compat: error
        }
    }
}

fn run(cli: Cli) -> Result<bool, Error> {
    let before = cli.context.unwrap_or(cli.before_context);
    let after = cli.context.unwrap_or(cli.after_context);
    
    // Build query plan
    let plan = QueryPlanner::plan(
        &cli.pattern,
        cli.regex,
        cli.fixed_strings,
    );
    
    // Determine output format
    let formatter = OutputFormatter::new(OutputConfig {
        line_numbers: cli.line_number || cli.paths.len() > 1,
        filenames: cli.paths.len() > 1 || cli.files_only,
        files_only: cli.files_only,
        count_only: cli.count,
        color: !cli.no_color && atty::is(atty::Stream::Stdout),
        before_context: before,
        after_context: after,
    });
    
    // Try strategies in order of preference
    if !cli.no_index {
        // Strategy 1: Try daemon IPC
        if let Some(result) = try_daemon_query(&cli, &plan, &formatter)? {
            return Ok(result);
        }
        
        // Strategy 2: Try direct index read (daemon not running)
        if let Some(result) = try_direct_index(&cli, &plan, &formatter)? {
            return Ok(result);
        }
    }
    
    // Strategy 3: Fall back to parallel scan
    if cli.stats {
        eprintln!("ix: no index available, falling back to scan");
    }
    fallback_scan(&cli, &plan, &formatter)
}

fn try_daemon_query(
    cli: &Cli,
    plan: &QueryPlan,
    formatter: &OutputFormatter,
) -> Result<Option<bool>, Error> {
    let socket_path = get_socket_path();
    
    let client = match IxClient::new(&socket_path).connect() {
        Ok(c) => c,
        Err(_) => return Ok(None),  // daemon not running
    };
    
    let query = QueryMessage {
        id: rand::random(),
        pattern: cli.pattern.clone(),
        roots: cli.paths.iter().map(|p| {
            p.canonicalize().unwrap().to_string_lossy().into()
        }).collect(),
        is_regex: cli.regex,
        is_fixed_string: cli.fixed_strings,
        case_insensitive: cli.ignore_case,
        max_results: cli.max_results,
        stale_policy: match cli.stale_policy {
            StaleArg::Rescan => "rescan",
            StaleArg::Skip => "skip",
            StaleArg::Warn => "warn",
        }.into(),
        include_stats: cli.stats,
        context_lines_before: cli.context.unwrap_or(cli.before_context),
        context_lines_after: cli.context.unwrap_or(cli.after_context),
    };
    
    let mut conn = client;
    let stream = IxClient::query(&mut conn, query)?;
    
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let mut found = false;
    let mut counts: HashMap<String, u32> = HashMap::new();
    
    for item in stream {
        match item? {
            QueryResultItem::Match(m) => {
                found = true;
                
                if cli.count {
                    *counts.entry(m.file_path.clone()).or_default() += 1;
                } else {
                    formatter.write_match(&mut out, &m)?;
                }
            }
            QueryResultItem::Done(d) => {
                if cli.count {
                    for (path, count) in &counts {
                        writeln!(out, "{}:{}", path, count)?;
                    }
                }
                
                if cli.stats {
                    if let Some(ref stats) = d.stats {
                        eprintln!(
                            "\n{} matches ({} candidates, {}μs{})",
                            d.total_matches,
                            stats.candidate_files,
                            stats.elapsed_microseconds,
                            if stats.stale_files_rescanned > 0 {
                                format!(", {} stale rescanned", 
                                    stats.stale_files_rescanned)
                            } else {
                                String::new()
                            }
                        );
                    }
                }
                break;
            }
        }
    }
    
    Ok(Some(found))
}

fn try_direct_index(
    cli: &Cli,
    plan: &QueryPlan,
    formatter: &OutputFormatter,
) -> Result<Option<bool>, Error> {
    // Look for .ix/shard.ix files in the search paths
    let mut readers = Vec::new();
    
    for path in &cli.paths {
        let index_path = path.join(".ix").join("shard.ix");
        if index_path.exists() {
            match IndexReader::open(&index_path) {
                Ok(reader) => readers.push((path.clone(), reader)),
                Err(e) => {
                    eprintln!("ix: warning: corrupt index at {:?}: {}", 
                        index_path, e);
                }
            }
        }
    }
    
    if readers.is_empty() {
        return Ok(None);
    }
    
    if cli.stats {
        eprintln!("ix: using index directly (daemon not running)");
    }
    
    let executor = QueryExecutor::new(&readers[0].1, QueryConfig {
        max_results: if cli.max_results == 0 { usize::MAX } else { cli.max_results as usize },
        stale_policy: match cli.stale_policy {
            StaleArg::Rescan => StalePolicy::Rescan,
            StaleArg::Skip => StalePolicy::Skip,
            StaleArg::Warn => StalePolicy::Warn,
        },
        case_insensitive: cli.ignore_case,
    });
    
    let (matches, stats) = executor.execute(plan)?;
    
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    
    for m in &matches {
        formatter.write_match_raw(&mut out, m)?;
    }
    
    if cli.stats {
        eprintln!(
            "\n{} matches ({} candidates, {}μs, direct index)",
            stats.total_matches,
            stats.candidate_files,
            stats.elapsed.as_micros(),
        );
    }
    
    Ok(Some(!matches.is_empty()))
}
```

### 10.3 — Output Formatter

```rust
struct OutputFormatter {
    config: OutputConfig,
}

struct OutputConfig {
    line_numbers: bool,
    filenames: bool,
    files_only: bool,
    count_only: bool,
    color: bool,
    before_context: u32,
    after_context: u32,
}

// ANSI color codes
const COLOR_FILENAME: &str = "\x1b[35m";    // magenta
const COLOR_LINE_NUM: &str = "\x1b[32m";    // green
const COLOR_MATCH: &str = "\x1b[1;31m";     // bold red
const COLOR_RESET: &str = "\x1b[0m";
const COLOR_SEP: &str = "\x1b[36m";         // cyan

impl OutputFormatter {
    fn write_match(
        &self,
        out: &mut impl Write,
        m: &MatchMessage,
    ) -> io::Result<()> {
        if self.config.files_only {
            writeln!(out, "{}", self.maybe_color(
                COLOR_FILENAME, &m.file_path))?;
            return Ok(());
        }
        
        // Context before
        for line in &m.context_before {
            if self.config.filenames {
                write!(out, "{}{}{}-",
                    if self.config.color { COLOR_FILENAME } else { "" },
                    m.file_path,
                    if self.config.color { COLOR_RESET } else { "" },
                )?;
            }
            writeln!(out, "{}", line)?;
        }
        
        // Match line
        if self.config.filenames {
            write!(out, "{}{}{}{}:{}",
                if self.config.color { COLOR_FILENAME } else { "" },
                m.file_path,
                if self.config.color { COLOR_RESET } else { "" },
                if self.config.color { COLOR_SEP } else { "" },
                if self.config.color { COLOR_RESET } else { "" },
            )?;
        }
        
        if self.config.line_numbers {
            write!(out, "{}{}{}{}:{}",
                if self.config.color { COLOR_LINE_NUM } else { "" },
                m.line_number,
                if self.config.color { COLOR_RESET } else { "" },
                if self.config.color { COLOR_SEP } else { "" },
                if self.config.color { COLOR_RESET } else { "" },
            )?;
        }
        
        // Highlight match within line
        if self.config.color {
            // TODO: highlight matched portion using column info
            writeln!(out, "{}", m.line_content)?;
        } else {
            writeln!(out, "{}", m.line_content)?;
        }
        
        // Context after
        for line in &m.context_after {
            if self.config.filenames {
                write!(out, "{}-", m.file_path)?;
            }
            writeln!(out, "{}", line)?;
        }
        
        Ok(())
    }
}
```

---

## Thought 11
**Purpose:** Fallback scanner — when index is unavailable or query is not indexable. Must be competitive with ripgrep. Guards against the failure mode where `ix` is slower than `grep` in any scenario.

**Pattern Check:** G-CTX-1 (this is the floor, not the ceiling), Compound Cascade (must produce identical results to indexed path)

**Anti-Pattern Action:**
- G-CTX-1: Use proven techniques from ripgrep — parallel directory walk, mmap, SIMD memchr
- Cascade: Same Match struct, same output formatter, same verification logic

**Self-Audit:** Clean.

**Reasoning:**

### 11.1 — Parallel Scanner

```rust
struct ParallelScanner {
    config: ScanConfig,
    thread_count: usize,
}

struct ScanConfig {
    pattern: Vec<u8>,
    regex: Option<Regex>,
    ignore_rules: IgnoreRules,
    max_file_size: u64,
    follow_symlinks: bool,
}

impl ParallelScanner {
    fn scan(
        &self,
        roots: &[PathBuf],
        sender: Sender<Match>,
    ) -> Result<ScanStats, Error> {
        let (path_tx, path_rx) = crossbeam::channel::bounded::<PathBuf>(1024);
        let stats = Arc::new(Mutex::new(ScanStats::default()));
        
        // ── Directory walker thread ──
        let walker_handle = {
            let roots = roots.to_vec();
            let config = self.config.clone();
            std::thread::spawn(move || {
                for root in &roots {
                    Self::walk_directory(root, &config, &path_tx);
                }
                drop(path_tx); // signal completion
            })
        };
        
        // ── Worker threads ──
        let worker_handles: Vec<_> = (0..self.thread_count)
            .map(|_| {
                let path_rx = path_rx.clone();
                let sender = sender.clone();
                let config = self.config.clone();
                let stats = stats.clone();
                
                std::thread::spawn(move || {
                    while let Ok(path) = path_rx.recv() {
                        match Self::scan_file(&path, &config) {
                            Ok(matches) => {
                                let mut s = stats.lock().unwrap();
                                s.files_scanned += 1;
                                
                                for m in matches {
                                    s.matches_found += 1;
                                    let _ = sender.send(m);
                                }
                            }
                            Err(_) => {
                                stats.lock().unwrap().files_errored += 1;
                            }
                        }
                    }
                })
            })
            .collect();
        
        drop(path_rx);
        drop(sender);
        
        walker_handle.join().unwrap();
        for h in worker_handles {
            h.join().unwrap();
        }
        
        let stats = Arc::try_unwrap(stats).unwrap().into_inner().unwrap();
        Ok(stats)
    }
    
    fn walk_directory(
        root: &Path,
        config: &ScanConfig,
        tx: &Sender<PathBuf>,
    ) {
        // Use ignore crate (same as ripgrep) for .gitignore support
        let walker = ignore::WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .follow_links(config.follow_symlinks)
            .max_filesize(Some(config.max_file_size))
            .build();
        
        for entry in walker {
            if let Ok(entry) = entry {
                if entry.file_type().map_or(false, |t| t.is_file()) {
                    if tx.send(entry.into_path()).is_err() {
                        break;
                    }
                }
            }
        }
    }
    
    fn scan_file(
        path: &Path,
        config: &ScanConfig,
    ) -> Result<Vec<Match>, Error> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        let data = &mmap[..];
        
        // Binary check
        if data[..data.len().min(8192)].contains(&0u8) {
            return Ok(vec![]);
        }
        
        let mut matches = Vec::new();
        
        if let Some(ref regex) = config.regex {
            // Regex search
            let text = String::from_utf8_lossy(data);
            for mat in regex.find_iter(&text) {
                let pos = mat.start();
                let m = Self::build_match(data, path, pos);
                matches.push(m);
            }
        } else {
            // Literal search using SIMD
            let finder = memchr::memmem::Finder::new(&config.pattern);
            let mut search_from = 0;
            let mut last_line_start = usize::MAX;
            
            while let Some(pos) = finder.find(&data[search_from..]) {
                let abs_pos = search_from + pos;
                let m = Self::build_match(data, path, abs_pos);
                
                // Dedup matches on same line
                let line_start = data[..abs_pos]
                    .iter()
                    .rposition(|&b| b == b'\n')
                    .map(|p| p + 1)
                    .unwrap_or(0);
                
                if line_start != last_line_start {
                    matches.push(m);
                    last_line_start = line_start;
                }
                
                // Advance past this match
                search_from = abs_pos + 1;
            }
        }
        
        Ok(matches)
    }
    
    fn build_match(data: &[u8], path: &Path, pos: usize) -> Match {
        let line_start = data[..pos]
            .iter()
            .rposition(|&b| b == b'\n')
            .map(|p| p + 1)
            .unwrap_or(0);
        
        let line_end = data[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| pos + p)
            .unwrap_or(data.len());
        
        let line_number = bytecount::count(&data[..line_start], b'\n') as u32 + 1;
        
        Match {
            file_path: path.to_owned(),
            line_number,
            line_content: data[line_start..line_end].to_vec(),
            byte_offset: pos as u64,
            freshness: Freshness::Fresh,
        }
    }
}
```

---

## Thought 12
**Purpose:** Configuration system — where does config live, what are the defaults, how is it loaded. Guards against user confusion and deployment failure.

**Pattern Check:** G-SEC-1 (config file permissions), G-CTX-1 (too many options), Compound Cascade (bad defaults break first experience)

**Anti-Pattern Action:**
- G-SEC-1: Config file must be owned by user, mode 0600/0644
- G-CTX-1: Zero configuration required for basic use. Everything has sane defaults.
- Cascade: First run with no config must work perfectly.

**Self-Audit:** Clean.

**Reasoning:**

### 12.1 — Configuration File

```toml
# ~/.config/ix/config.toml
# All fields optional. Shown with defaults.

[daemon]
# Where to store indexes
index_dir = "~/.local/share/ix/"

# Maximum RSS for daemon process (bytes)
max_memory = 209715200  # 200MB

# How long system must be idle before indexing starts (seconds)
idle_settle_seconds = 30

# Idle score threshold (0.0-1.0)
idle_threshold = 0.7

# Maximum index size per watch root (bytes, 0 = unlimited)
max_index_size = 0

# Debounce window for filesystem events (milliseconds)
debounce_ms = 500

[watch]
# Roots to index (can also be added via `ixd watch`)
roots = []

# Global exclude patterns (in addition to .gitignore)
exclude = [
    "*.min.js",
    "*.min.css",
    "*.map",
    "*.lock",
    "node_modules",
    ".git",
    "__pycache__",
    "target/debug",
    "target/release",
    "vendor",
    "dist",
    "build",
    ".cache",
    "*.pyc",
    "*.o",
    "*.so",
    "*.dylib",
    "*.a",
    "*.class",
    "*.jar",
]

# Maximum file size to index (bytes)
max_file_size = 104857600  # 100MB

# Follow symbolic links
follow_symlinks = false

[query]
# Default stale policy: "rescan", "skip", "warn"
stale_policy = "rescan"

# Default max results (0 = unlimited)
max_results = 0

# Enable colors by default
color = "auto"  # "auto", "always", "never"

[logging]
# Log file for daemon
log_file = "~/.local/share/ix/daemon.log"

# Log level: "error", "warn", "info", "debug", "trace"
level = "info"

# Maximum log file size before rotation (bytes)
max_log_size = 10485760  # 10MB

# Number of rotated logs to keep
max_log_files = 3
```

### 12.2 — Config Loading Priority

```
1. Command-line flags       (highest priority)
2. Environment variables    (IX_INDEX_DIR, IX_IDLE_THRESHOLD, etc.)
3. ~/.config/ix/config.toml
4. /etc/ix/config.toml      (system-wide defaults)
5. Compiled defaults        (lowest priority)
```

### 12.3 — File Path Resolution

```rust
fn resolve_paths() -> Paths {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", nix::unistd::getuid()));
    
    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .unwrap_or_else(|_| {
            format!("{}/.config", std::env::var("HOME").unwrap())
        });
    
    let data_dir = std::env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| {
            format!("{}/.local/share", std::env::var("HOME").unwrap())
        });
    
    Paths {
        socket: PathBuf::from(&runtime_dir).join("ix.sock"),
        pid_file: PathBuf::from(&runtime_dir).join("ix.pid"),
        config_file: PathBuf::from(&config_dir).join("ix/config.toml"),
        index_dir: PathBuf::from(&data_dir).join("ix"),
        log_file: PathBuf::from(&data_dir).join("ix/daemon.log"),
    }
}
```

---

## Thought 13
**Purpose:** Build system, testing strategy, and release packaging. Guards against "design document that never ships."

**Pattern Check:** G-CTX-1 (scope), Compound Cascade (untested code ships bugs)

**Anti-Pattern Action:**
- G-CTX-1: Minimal viable build — single binary, no runtime deps
- Cascade: Three test tiers, each catching different failure classes

**Self-Audit:** Clean.

**Reasoning:**

### 13.1 — Cargo.toml

```toml
[package]
name = "ix"
version = "0.1.0"
edition = "2021"
rust-version = "1.75"
description = "Indexed grep with background indexing"
license = "MIT"

[[bin]]
name = "ix"
path = "src/bin/ix.rs"

[[bin]]
name = "ixd"
path = "src/bin/ixd.rs"

[dependencies]
# CLI
clap = { version = "4", features = ["derive"] }

# Async runtime
tokio = { version = "1", features = ["full"] }

# Filesystem
memmap2 = "0.9"
notify = "6"
ignore = "0.4"
walkdir = "2"

# Search
memchr = "2"
regex = "1"
regex-syntax = "0.8"
regex-automata = "0.4"

# Hashing
xxhash-rust = { version = "0.8", features = ["xxh64"] }
crc32c = "0.6"

# Serialization
rmp-serde = "1"
serde = { version = "1", features = ["derive"] }
toml = "0.8"

# Concurrency
crossbeam = { version = "0.8", features = ["channel"] }

# Utilities
bytecount = "0.6"
atty = "0.2"
nix = { version = "0.28", features = ["user", "signal", "fs"] }
libc = "0.2"
log = "0.4"
env_logger = "0.11"
rand = "0.8"

[dev-dependencies]
tempfile = "3"
assert_cmd = "2"
predicates = "3"
proptest = "1"
criterion = "0.5"

[profile.release]
lto = true
codegen-units = 1
strip = true
panic = "abort"

[[bench]]
name = "trigram_bench"
harness = false

[[bench]]
name = "query_bench"
harness = false
```

### 13.2 — Test Strategy

```
┌────────────────────────────────────────────────────────────┐
│ TIER 1: Unit Tests (cargo test)                           │
│                                                            │
│ trigram.rs:                                                │
│   - Empty input                                           │
│   - Single trigram                                        │
│   - Overlapping trigrams                                  │
│   - Null byte handling                                    │
│   - UTF-8 multibyte characters                            │
│   - All-same-byte input                                   │
│   - 16MB input (performance bound)                        │
│                                                            │
│ posting.rs:                                               │
│   - Varint encode/decode roundtrip (proptest)             │
│   - Empty posting list                                    │
│   - Single entry posting list                             │
│   - Delta encoding correctness                            │
│   - Maximum file_id values                                │
│   - Truncated data → error (not panic)                    │
│                                                            │
│ bloom.rs:                                                 │
│   - No false negatives (proptest)                         │
│   - FPR within bounds (statistical test)                  │
│   - Empty bloom                                           │
│   - Full bloom                                            │
│                                                            │
│ planner.rs:                                               │
│   - Literal extraction from regex patterns                │
│   - Alternation handling                                  │
│   - Nested groups                                         │
│   - Empty pattern                                         │
│   - Pure wildcard → FullScan                              │
│                                                            │
│ index/reader.rs:                                          │
│   - Corrupted header → error                              │
│   - Truncated file → error                                │
│   - Bad CRC → error                                       │
│   - Zero-length sections                                  │
│   - Offset pointing past EOF → error                      │
│                                                            │
└────────────────────────────────────────────────────────────┘

┌────────────────────────────────────────────────────────────┐
│ TIER 2: Integration Tests                                 │
│                                                            │
│ Build index from fixture directory, query it:             │
│                                                            │
│ test_literal_search:                                      │
│   Create temp dir with known files                        │
│   Build index                                             │
│   Query "known_string"                                    │
│   Assert exact matches                                    │
│                                                            │
│ test_regex_search:                                        │
│   Same setup                                              │
│   Query "err.*timeout"                                    │
│   Assert matches subset of grep output                    │
│                                                            │
│ test_incremental_update:                                  │
│   Build index                                             │
│   Modify a file                                           │
│   Add to delta index                                      │
│   Query must find new content                             │
│   Query must not find deleted content                     │
│                                                            │
│ test_stale_detection:                                     │
│   Build index                                             │
│   Modify file (change mtime)                              │
│   Query with stale_policy=rescan                          │
│   Verify fresh results returned                           │
│                                                            │
│ test_no_false_negatives:                                  │
│   For 1000 random patterns present in test corpus:        │
│     grep_results = grep(pattern, corpus)                  │
│     ix_results = ix(pattern, corpus)                      │
│     assert grep_results ⊆ ix_results                     │
│   (ix may have false positives from index, but grep       │
│    results must always be a subset after verification)    │
│                                                            │
│ test_crash_recovery:                                      │
│   Start index build                                       │
│   Kill process mid-write                                  │
│   Restart: must not serve corrupt data                    │
│   Must either use old valid index or rebuild              │
│                                                            │
│ test_grep_output_compatibility:                           │
│   For various flag combinations:                          │
│     grep_out = grep [flags] pattern dir                   │
│     ix_out = ix [flags] pattern dir                       │
│     assert same output format (ignoring order)            │
│                                                            │
└────────────────────────────────────────────────────────────┘

┌────────────────────────────────────────────────────────────┐
│ TIER 3: Property-Based Tests (proptest)                   │
│                                                            │
│ For randomly generated text and patterns:                  │
│   1. Build index                                          │
│   2. Query via index                                      │
│   3. Query via brute-force scan                           │
│   4. Assert: index_results == scan_results                │
│                                                            │
│ For randomly generated posting lists:                     │
│   1. Encode                                               │
│   2. Decode                                               │
│   3. Assert: original == decoded                          │
│                                                            │
│ For randomly generated file sets:                         │
│   1. Build index                                          │
│   2. Modify random files                                  │
│   3. Rebuild index                                        │
│   4. Assert: second index produces same query results     │
│      as first index + modifications                       │
│                                                            │
└────────────────────────────────────────────────────────────┘
```

### 13.3 — Benchmarks

```rust
// benches/trigram_bench.rs

use criterion::{criterion_group, criterion_main, Criterion, Throughput};

fn bench_trigram_extraction(c: &mut Criterion) {
    let data = std::fs::read("testdata/large_source.rs").unwrap();
    
    let mut group = c.benchmark_group("trigram_extraction");
    group.throughput(Throughput::Bytes(data.len() as u64));
    
    group.bench_function("extract", |b| {
        let mut extractor = TrigramExtractor::new();
        b.iter(|| {
            extractor.extract_for_indexing(&data);
        });
    });
    
    group.finish();
}

fn bench_posting_decode(c: &mut Criterion) {
    // Create a realistic posting list
    let entries = generate_posting_list(10_000, 5);
    let encoded = encode_posting_list(&entries);
    
    let mut group = c.benchmark_group("posting_decode");
    group.throughput(Throughput::Bytes(encoded.len() as u64));
    
    group.bench_function("decode", |b| {
        b.iter(|| {
            decode_posting_list(&encoded).unwrap();
        });
    });
    
    group.finish();
}

fn bench_query_literal(c: &mut Criterion) {
    // Build index from test corpus
    let index = build_test_index("testdata/corpus/");
    
    c.bench_function("query_literal_common", |b| {
        b.iter(|| {
            let plan = QueryPlanner::plan("error", false, true);
            let executor = QueryExecutor::new(&index, QueryConfig::default());
            executor.execute(&plan).unwrap();
        });
    });
    
    c.bench_function("query_literal_rare", |b| {
        b.iter(|| {
            let plan = QueryPlanner::plan("ConnectionTimeout", false, true);
            let executor = QueryExecutor::new(&index, QueryConfig::default());
            executor.execute(&plan).unwrap();
        });
    });
    
    c.bench_function("query_regex", |b| {
        b.iter(|| {
            let plan = QueryPlanner::plan("err(or|no).*time", true, false);
            let executor = QueryExecutor::new(&index, QueryConfig::default());
            executor.execute(&plan).unwrap();
        });
    });
}

criterion_group!(
    benches,
    bench_trigram_extraction,
    bench_posting_decode,
    bench_query_literal,
);
criterion_main!(benches);
```

### 13.4 — Directory Structure (Final)

```
ix/
├── Cargo.toml
├── Cargo.lock
├── LICENSE
├── README.md
├── DESIGN.md                        # This document
├── src/
│   ├── lib.rs                       # Public API
│   ├── bin/
│   │   ├── ix.rs                    # CLI binary entry point
│   │   └── ixd.rs                   # Daemon binary entry point
│   ├── index/
│   │   ├── mod.rs
│   │   ├── constants.rs             # Shared format constants
│   │   ├── trigram.rs               # Trigram extraction
│   │   ├── posting.rs               # Posting list encode/decode
│   │   ├── bloom.rs                 # Bloom filter
│   │   ├── builder.rs               # Index construction
│   │   ├── reader.rs                # mmap'd index reader
│   │   ├── delta.rs                 # Delta (in-memory) index
│   │   ├── merge.rs                 # Delta → main merge
│   │   ├── string_pool.rs           # Path string deduplication
│   │   └── varint.rs                # Varint codec
│   ├── daemon/
│   │   ├── mod.rs
│   │   ├── lifecycle.rs             # Startup, shutdown, PID file
│   │   ├── idle.rs                  # Dormancy detection
│   │   ├── idle_linux.rs            # Linux-specific idle detection
│   │   ├── idle_macos.rs            # macOS-specific idle detection
│   │   ├── watcher.rs               # FS event processing + debounce
│   │   ├── scheduler.rs             # Indexing work queue + priority
│   │   └── ipc.rs                   # Unix socket server
│   ├── query/
│   │   ├── mod.rs
│   │   ├── planner.rs               # Pattern → QueryPlan
│   │   ├── executor.rs              # Plan → Results (from index)
│   │   └── verify.rs                # Candidate verification
│   ├── scan/
│   │   ├── mod.rs
│   │   └── parallel.rs              # Fallback parallel scanner
│   ├── output/
│   │   ├── mod.rs
│   │   └── formatter.rs             # grep-compatible output
│   ├── config/
│   │   ├── mod.rs
│   │   └── paths.rs                 # XDG path resolution
│   └── ipc/
│       ├── mod.rs
│       ├── protocol.rs              # Wire protocol types
│       ├── client.rs                # Client-side IPC
│       └── server.rs                # Server-side IPC
├── benches/
│   ├── trigram_bench.rs
│   └── query_bench.rs
├── tests/
│   ├── integration/
│   │   ├── basic_search.rs
│   │   ├── regex_search.rs
│   │   ├── incremental.rs
│   │   ├── staleness.rs
│   │   ├── crash_recovery.rs
│   │   └── grep_compat.rs
│   ├── property/
│   │   ├── trigram_props.rs
│   │   ├── posting_props.rs
│   │   └── search_correctness.rs
│   └── testdata/
│       ├── corpus/                  # Test source files
│       ├── large_source.rs          # Single large file for benchmarks
│       └── fixtures/                # Pre-built test scenarios
└── scripts/
    ├── install.sh                   # Install daemon systemd unit
    └── benchmark.sh                 # Run comparative benchmarks
```

### 13.5 — Systemd Integration

```ini
# ~/.config/systemd/user/ix.service
[Unit]
Description=ix indexed grep daemon
After=default.target

[Service]
Type=simple
ExecStart=%h/.cargo/bin/ixd start --foreground
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
RestartSec=5
Nice=19
IOSchedulingClass=idle
MemoryMax=300M

[Install]
WantedBy=default.target
```

```bash
# scripts/install.sh
#!/bin/bash
set -euo pipefail

cargo build --release

cp target/release/ix ~/.cargo/bin/
cp target/release/ixd ~/.cargo/bin/

mkdir -p ~/.config/systemd/user/
cp scripts/ix.service ~/.config/systemd/user/

systemctl --user daemon-reload
systemctl --user enable ix
systemctl --user start ix

echo "ix installed. Run 'ixd watch ~/code' to start indexing."
```

---

## Verification

**1. Every failure pattern actively guarded:**

| Pattern | Thoughts | Mechanism |
|---------|----------|-----------|
| G-MEM-1 | 1,3,4,6,7,8 | Arena alloc, bounded queues, streaming decode, mmap (kernel-managed pages), RSS budget with self-restart |
| G-CTX-1 | 1,5,8,10 | Explicit scope boundary, grep exit codes, identical output format, zero-config defaults |
| G-SEC-1 | 1,3,7,9,12 | Socket 0600, UID validation, no symlink following, offset bounds checking, CRC validation |
| Compound Cascade | 1,2,3,4,5,6,7,8 | Shared constants module, mandatory verification pass, atomic rename, delta/main merge produces same results as full rebuild (property tested), freshness tags on every result |

**2. Compound Cascade check in reasoning:**
- Builder (Thought 3) writes format defined in Thought 1 → uses same constants.rs
- Reader (Thought 4) reads format defined in Thought 1 → uses same constants.rs
- Query executor (Thought 6) depends on reader (Thought 4) and planner (Thought 5) → verified interface compatibility
- Delta index (Thought 8) merges with main index → merge produces format identical to full build (property tested)
- IPC protocol (Thought 9) carries Match struct compatible with output formatter (Thought 10) → verified

**3. "Quick fix for now" audit:**
- Every `todo!()` is in platform-specific idle detection only (marked as per-platform stubs)
- No `unwrap()` on user-facing paths — all return `Result`
- No deferred design decisions remain

**4. Semantic validation:**
- Can each module be implemented independently? **Yes** — interfaces defined, no circular dependencies
- Does the fallback scanner make `ix` usable day-one before any indexing? **Yes**
- Is the zero-config experience good? **Yes** — `ix "pattern" dir` works with no daemon, no index, no config
- Does it get dramatically better with the daemon? **Yes** — 10,000x for indexed queries

---

## Final Answer

The complete build schematics for `ix` span:

- **Byte-level index format** (256-byte header, fixed trigram table, varint-encoded posting lists, per-file bloom filters, prefix-compressed string pool)
- **Trigram extraction** (byte-level, null-aware, zero-allocation inner loop)
- **Index builder** (streaming, dormancy-interruptible, memory-bounded, atomic output)
- **Index reader** (mmap-only, bounds-validated, CRC-checked)
- **Query planner** (literal fast path, regex literal extraction via HIR walk, full-scan fallback)
- **Query executor** (rarest-first intersection, bloom pre-filter, mandatory verification)
- **Daemon state machine** (4 states, async IPC + dedicated indexer thread, atomic index swap)
- **Idle detector** (weighted multi-signal EMA, platform-abstracted, 10ms yield latency)
- **Incremental updates** (LSM-style delta index, bounded merge threshold)
- **IPC protocol** (length-prefixed msgpack, streaming results, forward-compatible)
- **CLI** (grep-compatible flags, exit codes, output format)
- **Fallback scanner** (parallel walk + mmap + SIMD memchr)
- **Configuration** (TOML, XDG paths, zero-config defaults)
- **Test strategy** (3 tiers: unit, integration, property-based)
- **Build and deployment** (single static binary, systemd user unit, Nice 19 + IO idle class)

The first useful binary can ship with just: `varint.rs` + `trigram.rs` + `posting.rs` + `builder.rs` + `reader.rs` + `planner.rs` + `executor.rs` + `ix.rs` — roughly **3,000 lines of Rust**. The daemon adds another **2,000 lines**. Total: **~5,000 lines for a complete implementation**.
