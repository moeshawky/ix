//! Index builder — the complete pipeline from files to .ix shard.
//!
//! Phase 1: Discovery (walk directory, respect .gitignore)
//! Phase 2: Scan (mmap, check binary, extract trigrams, bloom filter)
//! Phase 3: Serialize (write sections, compute CRCs, atomic rename)

use crate::bloom::BloomFilter;
use crate::error::{Error, Result};
use crate::format::*;
use crate::posting::{PostingEntry, PostingList};
use crate::string_pool::StringPool;
use crate::trigram::{Extractor, Trigram};
use ignore::WalkBuilder;
use memmap2::Mmap;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub struct Builder {
    root: PathBuf,
    files: Vec<FileRecord>,
    postings: HashMap<Trigram, Vec<PostingEntry>>,
    string_pool: StringPool,
    bloom_filters: Vec<BloomFilter>,
    extractor: Extractor,
    stats: BuildStats,
}

pub struct FileRecord {
    pub file_id: u32,
    pub path: PathBuf,
    pub mtime_ns: u64,
    pub size_bytes: u64,
    pub content_hash: u64,
    pub trigram_count: u32,
    pub is_binary: bool,
}

#[derive(Default, Debug)]
pub struct BuildStats {
    pub files_scanned: u64,
    pub files_skipped_binary: u64,
    pub files_skipped_size: u64,
    pub bytes_scanned: u64,
    pub unique_trigrams: u64,
}

impl Builder {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            files: Vec::new(),
            postings: HashMap::new(),
            string_pool: StringPool::new(),
            bloom_filters: Vec::new(),
            extractor: Extractor::new(),
            stats: BuildStats::default(),
        }
    }

    pub fn build(&mut self) -> Result<PathBuf> {
        let start = Instant::now();

        // 1. Discovery
        let file_paths = self.discover_files()?;

        // 2. Scan & Extract
        let mut next_id = 0u32;
        for path in file_paths {
            if self.process_file(next_id, path)? {
                next_id += 1;
            }
        }

        // 3. Serialize
        let output_path = self.serialize()?;

        tracing::info!("Build completed in {:?}: {:?}", start.elapsed(), self.stats);

        Ok(output_path)
    }

    fn discover_files(&mut self) -> Result<Vec<PathBuf>> {
        let mut paths = Vec::new();
        let walker = WalkBuilder::new(&self.root)
            .hidden(false)
            .git_ignore(true)
            .build();

        for result in walker {
            let entry = result.map_err(|e| Error::Config(e.to_string()))?;
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                paths.push(entry.path().to_owned());
                // Don't add to string_pool yet, only if not skipped
            }
        }
        Ok(paths)
    }

    fn process_file(&mut self, file_id: u32, path: PathBuf) -> Result<bool> {
        let metadata = fs::metadata(&path)?;
        let size = metadata.len();
        let mtime = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        if size > 100 * 1024 * 1024 {
            // 100MB limit
            self.stats.files_skipped_size += 1;
            return Ok(false);
        }

        let file = File::open(&path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        let data = &mmap[..];

        // Binary check (null byte in first 8KB)
        let check_len = data.len().min(8192);
        if data[..check_len].contains(&0u8) {
            self.stats.files_skipped_binary += 1;
            return Ok(false);
        }

        let content_hash = xxhash_rust::xxh64::xxh64(data, 0);
        let trigrams = self.extractor.extract_with_offsets(data);
        let trigram_count = trigrams.len() as u32;

        self.string_pool.add_path(&path);

        let mut bloom = BloomFilter::new(256, 5);
        for (&tri, offsets) in trigrams {
            bloom.insert(tri);
            self.postings.entry(tri).or_default().push(PostingEntry {
                file_id,
                offsets: offsets.clone(),
            });
        }

        self.files.push(FileRecord {
            file_id,
            path,
            mtime_ns: mtime,
            size_bytes: size,
            content_hash,
            trigram_count,
            is_binary: false,
        });

        self.bloom_filters.push(bloom);
        self.stats.files_scanned += 1;
        self.stats.bytes_scanned += size;

        Ok(true)
    }

    fn serialize(&mut self) -> Result<PathBuf> {
        let ix_dir = self.root.join(".ix");
        fs::create_dir_all(&ix_dir)?;
        let tmp_path = ix_dir.join("shard.ix.tmp");
        let final_path = ix_dir.join("shard.ix");

        let mut f = BufWriter::new(File::create(&tmp_path)?);

        // Placeholder for header
        f.write_all(&[0u8; HEADER_SIZE])?;

        // Pre-serialize string pool to get offsets
        let mut string_pool_buf = std::io::Cursor::new(Vec::new());
        self.string_pool.serialize(&mut string_pool_buf)?;
        let string_pool_data = string_pool_buf.into_inner();

        // 1. File Table
        let file_table_offset = self.align_to_8(&mut f)?;
        for record in &self.files {
            let (path_off, path_len) = self.string_pool.get_info(&record.path);
            f.write_all(&record.file_id.to_le_bytes())?;
            f.write_all(&path_off.to_le_bytes())?;
            f.write_all(&path_len.to_le_bytes())?;
            f.write_all(&[FileStatus::Fresh as u8])?;
            f.write_all(&[0u8])?; // flags
            f.write_all(&record.mtime_ns.to_le_bytes())?;
            f.write_all(&record.size_bytes.to_le_bytes())?;
            f.write_all(&record.content_hash.to_le_bytes())?;
            f.write_all(&record.trigram_count.to_le_bytes())?;
            f.write_all(&0u32.to_le_bytes())?; // bloom_offset (filled later)
            f.write_all(&[0u8; 4])?; // padding
        }
        let file_table_size = f.stream_position()? - file_table_offset;

        // 2. Posting Data
        self.align_to_8(&mut f)?;
        let posting_data_offset = f.stream_position()?;
        let mut posting_infos = HashMap::new();

        let mut sorted_trigrams: Vec<Trigram> = self.postings.keys().cloned().collect();
        sorted_trigrams.sort_unstable();

        for &tri in &sorted_trigrams {
            let entries = &self.postings[&tri];
            let list = PostingList {
                entries: entries.clone(),
            };
            let encoded = list.encode();
            let offset = f.stream_position()? - posting_data_offset;
            f.write_all(&encoded)?;
            posting_infos.insert(tri, (offset, encoded.len() as u32, entries.len() as u32));
        }
        let posting_data_size = f.stream_position()? - posting_data_offset;

        // 3. Trigram Table
        self.align_to_8(&mut f)?;
        let trigram_table_offset = f.stream_position()?;
        for i in 0..TRIGRAM_SLOTS {
            if let Some(&(off, len, freq)) = posting_infos.get(&(i as u32)) {
                let abs_off = posting_data_offset + off;
                f.write_all(&abs_off.to_le_bytes()[..6])?; // u48
                f.write_all(&len.to_le_bytes())?; // u32
                f.write_all(&freq.to_le_bytes())?; // u32
                f.write_all(&[0u8; 2])?; // padding
            } else {
                f.write_all(&[0u8; 16])?;
            }
        }
        let trigram_table_size = f.stream_position()? - trigram_table_offset;

        // 4. Bloom Filters
        self.align_to_8(&mut f)?;
        let bloom_offset = f.stream_position()?;
        let mut bloom_relative_offsets = Vec::new();
        for bloom in &self.bloom_filters {
            let rel_off = (f.stream_position()? - bloom_offset) as u32;
            bloom_relative_offsets.push(rel_off);
            bloom.serialize(&mut f)?;
        }
        let bloom_size = f.stream_position()? - bloom_offset;

        // 5. String Pool
        self.align_to_8(&mut f)?;
        let string_pool_offset = f.stream_position()?;
        f.write_all(&string_pool_data)?;
        let string_pool_size = string_pool_data.len() as u64;

        // 6. Name Index (TODO, optional for now)
        let name_index_offset = f.stream_position()?;
        let name_index_size = 0u64;

        // Update File Table with bloom offsets
        f.seek(SeekFrom::Start(file_table_offset))?;
        for (i, _record) in self.files.iter().enumerate() {
            f.seek(SeekFrom::Current(40))?; // skip to bloom_offset
            f.write_all(&bloom_relative_offsets[i].to_le_bytes())?;
            f.seek(SeekFrom::Current(4))?; // skip padding
        }

        // Finalize Header
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;
        let mut header_bytes = [0u8; HEADER_SIZE];
        header_bytes[0..4].copy_from_slice(&MAGIC);
        header_bytes[0x04..0x06].copy_from_slice(&VERSION_MAJOR.to_le_bytes());
        header_bytes[0x06..0x08].copy_from_slice(&VERSION_MINOR.to_le_bytes());
        header_bytes[0x08..0x10]
            .copy_from_slice(&(flags::HAS_BLOOM_FILTERS | flags::HAS_CONTENT_HASHES).to_le_bytes());
        header_bytes[0x10..0x18].copy_from_slice(&created_at.to_le_bytes());
        header_bytes[0x18..0x20].copy_from_slice(&self.stats.bytes_scanned.to_le_bytes());
        header_bytes[0x20..0x24].copy_from_slice(&(self.files.len() as u32).to_le_bytes());
        header_bytes[0x24..0x28].copy_from_slice(&(sorted_trigrams.len() as u32).to_le_bytes());
        header_bytes[0x28..0x30].copy_from_slice(&file_table_offset.to_le_bytes());
        header_bytes[0x30..0x38].copy_from_slice(&file_table_size.to_le_bytes());
        header_bytes[0x38..0x40].copy_from_slice(&trigram_table_offset.to_le_bytes());
        header_bytes[0x40..0x48].copy_from_slice(&trigram_table_size.to_le_bytes());
        header_bytes[0x48..0x50].copy_from_slice(&posting_data_offset.to_le_bytes());
        header_bytes[0x50..0x58].copy_from_slice(&posting_data_size.to_le_bytes());
        header_bytes[0x58..0x60].copy_from_slice(&bloom_offset.to_le_bytes());
        header_bytes[0x60..0x68].copy_from_slice(&bloom_size.to_le_bytes());
        header_bytes[0x68..0x70].copy_from_slice(&string_pool_offset.to_le_bytes());
        header_bytes[0x70..0x78].copy_from_slice(&string_pool_size.to_le_bytes());
        header_bytes[0x78..0x80].copy_from_slice(&name_index_offset.to_le_bytes());
        header_bytes[0x80..0x88].copy_from_slice(&name_index_size.to_le_bytes());

        let crc = crc32c::crc32c(&header_bytes[0..0xF8]);
        header_bytes[0xF8..0xFC].copy_from_slice(&crc.to_le_bytes());

        f.seek(SeekFrom::Start(0))?;
        f.write_all(&header_bytes)?;
        f.flush()?;
        drop(f);

        fs::rename(&tmp_path, &final_path)?;
        Ok(final_path)
    }

    fn align_to_8<W: Write + Seek>(&self, mut w: W) -> std::io::Result<u64> {
        let pos = w.stream_position()?;
        let padding = (8 - (pos % 8)) % 8;
        if padding > 0 {
            w.write_all(&vec![0u8; padding as usize])?;
        }
        w.stream_position()
    }
}
