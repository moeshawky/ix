//! Index reader — the mmap-based query-time interface.
//!
//! Fast, zero-copy access to the index data.

use crate::bloom::BloomFilter;
use crate::error::{Error, Result};
use crate::format::{DELTA_FILE_ENTRY, DELTA_MAGIC, DELTA_TOMBSTONE, DELTA_TRIGRAM_ENTRY};
use crate::format::{FILE_ENTRY_SIZE, FileStatus, HEADER_SIZE, Header};
use crate::posting::PostingList;
use crate::string_pool::StringPoolReader;
use crate::trigram::Trigram;
use memmap2::Mmap;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// Lightweight snapshot of shard-level metadata (no mmap needed).
///
/// Public for external monitoring tools via [`Reader::metadata`].
#[derive(Debug, Clone, Copy)]
pub struct ShardMetadata {
    /// Microsecond-precision Unix timestamp from the shard header.
    pub shard_timestamp: u64,
    /// Total number of files indexed in this shard.
    pub file_count: u32,
    /// Total number of unique trigrams in this shard.
    pub trigram_count: u32,
}

/// One entry in the CDX block index: first trigram key + absolute block offset.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CdxBlockEntry {
    /// First trigram key in this block.
    pub first_key: u32,
    /// Absolute byte offset of the compressed block.
    pub block_offset: u64,
}

/// Index reader — mmaps the shard file for zero-copy lookups.
pub struct Reader {
    mmap: Mmap,
    /// Parsed shard header containing section offsets and sizes.
    pub header: Header,
    string_pool: StringPoolReader<'static>,
    #[cfg(unix)]
    inode: Option<u64>,
    cdx_blocks: Vec<CdxBlockEntry>,
    /// Root directory derived from the shard path (parent of `.ix/`).
    root: PathBuf,
}

/// Descriptor pointing into the trigram table for a single trigram.
#[derive(Debug)]
pub struct TrigramInfo {
    /// Absolute file offset where the posting list begins.
    pub posting_offset: u64,
    /// Number of bytes in the encoded posting list.
    pub posting_length: u32,
    /// How many files contain this trigram (document frequency).
    pub doc_frequency: u32,
}

/// Metadata about a single file known to the index.
#[derive(Debug)]
pub struct FileInfo {
    /// Internal 0-based file identifier.
    pub file_id: u32,
    /// Absolute path to the file on disk.
    pub path: PathBuf,
    /// Whether the file is fresh, stale, or deleted.
    pub status: FileStatus,
    /// Last modification time in nanoseconds since the Unix epoch.
    pub mtime_ns: u64,
    /// File size in bytes at index time.
    pub size_bytes: u64,
    /// XXH64 content hash computed at index time.
    pub content_hash: u64,
}

impl Reader {
    /// Open and memory-map an index file for reading.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened, memory-mapped, or its
    /// header is invalid.
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;

        // SAFETY: Mmap::map wraps the mmap(2) syscall. The file handle is kept alive
        // by Mmap's internal Arc<File>, ensuring the underlying data remains valid
        // for the lifetime of the mmap.
        let mmap = unsafe { Mmap::map(&file)? };

        let header_slice = mmap.get(0..HEADER_SIZE).ok_or(Error::IndexTooSmall)?;
        let file_len_u64 = u64::try_from(mmap.len()).unwrap_or(u64::MAX);

        let header = Header::parse(header_slice)?;
        header.validate_bounds(file_len_u64)?;

        #[cfg(unix)]
        let inode = Some(file.metadata()?.ino());

        // SAFETY: We transmute the slice lifetime to 'static. This is sound because:
        // INVARIANT: Reader owns the Mmap, which owns the underlying memory.
        // INVARIANT: Mmap's data remains valid for the entire lifetime of Reader.
        // INVARIANT: No mutable access to mmap occurs after construction.
        // INVARIANT: StringPoolReader<'static> cannot outlive Reader (it's a field).
        // This is the standard pattern for self-referential mmap structs in Rust.
        // Bounds-check before the self-referential transmute so a corrupt
        // header (overflowing or out-of-range offset/size) yields a typed
        // error instead of panicking on an out-of-bounds slice. Mirrors the
        // safe CDX path below (mmap.get(...).ok_or(SectionOutOfBounds)).
        let start = usize::try_from(header.string_pool_offset).unwrap_or(usize::MAX);
        let size = usize::try_from(header.string_pool_size).unwrap_or(usize::MAX);
        let end = start.checked_add(size).ok_or(Error::SectionOutOfBounds {
            section: "string_pool",
            offset: header.string_pool_offset,
            size: header.string_pool_size,
            file_len: file_len_u64,
        })?;
        let raw_pool_slice = mmap.get(start..end).ok_or(Error::SectionOutOfBounds {
            section: "string_pool",
            offset: header.string_pool_offset,
            size: header.string_pool_size,
            file_len: file_len_u64,
        })?;
        let string_pool_data: &'static [u8] =
            unsafe { std::mem::transmute::<&[u8], &'static [u8]>(raw_pool_slice) };
        let string_pool = StringPoolReader::new(string_pool_data)?;

        let cdx_blocks = if header.has_cdx() && header.cdx_block_index_size > 0 {
            let idx_start = usize::try_from(header.cdx_block_index_offset).unwrap_or(usize::MAX);
            let idx_size = usize::try_from(header.cdx_block_index_size).unwrap_or(usize::MAX);
            let idx_end = idx_start
                .checked_add(idx_size)
                .ok_or(Error::SectionOutOfBounds {
                    section: "cdx_block_index",
                    offset: header.cdx_block_index_offset,
                    size: header.cdx_block_index_size,
                    file_len: file_len_u64,
                })?;
            let idx_data = mmap
                .get(idx_start..idx_end)
                .ok_or(Error::SectionOutOfBounds {
                    section: "cdx_block_index",
                    offset: header.cdx_block_index_offset,
                    size: header.cdx_block_index_size,
                    file_len: file_len_u64,
                })?;
            let mut blocks = Vec::new();
            let mut pos = 0usize;
            while let Some(entry_bytes) = idx_data.get(pos..pos + 12) {
                let first_key = u32::from_le_bytes(
                    entry_bytes
                        .get(0..4)
                        .and_then(|s| s.try_into().ok())
                        .ok_or_else(|| Error::Config("bad cdx key".into()))?,
                );
                if first_key == u32::MAX {
                    break;
                }
                let block_offset = u64::from_le_bytes(
                    entry_bytes
                        .get(4..12)
                        .and_then(|s| s.try_into().ok())
                        .ok_or_else(|| Error::Config("bad cdx offset".into()))?,
                );
                blocks.push(CdxBlockEntry {
                    first_key,
                    block_offset,
                });
                pos += 12;
            }
            blocks
        } else {
            Vec::new()
        };

        let root = path
            .parent()
            .and_then(|p| p.parent())
            .map_or_else(|| path.to_path_buf(), Path::to_path_buf);

        Ok(Self {
            mmap,
            header,
            string_pool,
            #[cfg(unix)]
            inode,
            cdx_blocks,
            root,
        })
    }

    /// Get the last modification time among all source files in the tree.
    ///
    /// Honors `.ixd.toml` (`watch_roots` and `exclude_patterns`) as well as
    /// built-in directory exclusions including `.codegraph`, `.git`, and `.ix`.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory walk fails or metadata cannot be read.
    pub fn get_last_modified(root: &Path) -> Result<u64> {
        let walk_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let config = crate::config::Config::discover_under(&walk_root).unwrap_or_default();
        let has_root_config = walk_root.join(".ixd.toml").exists();
        let watch_roots: Vec<PathBuf> = config
            .watch_roots
            .into_iter()
            .map(|wr| {
                if wr.is_relative() {
                    walk_root.join(wr)
                } else {
                    wr
                }
            })
            .collect();
        let exclude_patterns = config.exclude_patterns;

        let mut last_modified = 0u64;
        let walker = ignore::WalkBuilder::new(&walk_root)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .require_git(true) // within-repo .gitignore only; never ancestor ~/.gitignore (audit D4)
            .add_custom_ignore_filename(".ixignore")
            .filter_entry({
                let walk_root = walk_root.clone();
                move |entry| {
                    if !crate::builder::default_filter_entry(
                        entry,
                        &walk_root,
                        &exclude_patterns,
                        &watch_roots,
                    ) {
                        return false;
                    }
                    if !has_root_config && entry.file_type().is_some_and(|t| t.is_dir()) {
                        let name = entry
                            .path()
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("");
                        if matches!(name, "__pycache__" | ".tox" | ".venv" | "venv") {
                            return false;
                        }
                    }
                    true
                }
            })
            .build();

        for result in walker {
            match result {
                Ok(entry) => {
                    if entry.file_type().is_some_and(|t| t.is_file()) {
                        let metadata =
                            entry.metadata().map_err(|e| Error::Config(e.to_string()))?;
                        let mtime = metadata
                            .modified()
                            .and_then(|t| {
                                t.duration_since(UNIX_EPOCH)
                                    .map_err(|_| std::io::Error::other("time went backwards"))
                            })
                            .map_or(0, |d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX));
                        if mtime > last_modified {
                            last_modified = mtime;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("ix: warning: stale check skipping path: {e}");
                }
            }
        }
        Ok(last_modified)
    }

    /// Binary search the trigram table. Returns `Ok(None)` if the trigram
    /// is unknown or an error if the index data is corrupted.
    ///
    /// When CDX compression is active, performs a two-level search:
    /// first on the block index, then within the decompressed block.
    ///
    /// # Errors
    ///
    /// Returns `Error::CdxBlockCorrupted` if the CDX block cannot be
    /// decompressed or contains malformed varint data.
    pub fn get_trigram(&self, trigram: Trigram) -> Result<Option<TrigramInfo>> {
        if self.header.has_cdx() && !self.cdx_blocks.is_empty() {
            return self.get_trigram_cdx(trigram);
        }

        // Legacy fallback (no CDX)
        let file_len_u64 = u64::try_from(self.mmap.len()).unwrap_or(u64::MAX);
        let section_oob = |off: u64, sz: u64| Error::SectionOutOfBounds {
            section: "trigram_table",
            offset: off,
            size: sz,
            file_len: file_len_u64,
        };
        let count = usize::try_from(self.header.trigram_count).map_err(|_| {
            section_oob(
                self.header.trigram_table_offset,
                self.header.trigram_table_size,
            )
        })?;
        let table_start = usize::try_from(self.header.trigram_table_offset).map_err(|_| {
            section_oob(
                self.header.trigram_table_offset,
                self.header.trigram_table_size,
            )
        })?;
        let entry_size = crate::format::TRIGRAM_ENTRY_SIZE;
        let entry_size_u64 = u64::try_from(entry_size).unwrap_or(u64::MAX);

        let mut low = 0usize;
        let mut high = count;

        while low < high {
            let mid = low + (high - low) / 2;
            let entry_off = mid
                .checked_mul(entry_size)
                .and_then(|rel| table_start.checked_add(rel))
                .ok_or_else(|| {
                    section_oob(
                        self.header.trigram_table_offset,
                        self.header.trigram_table_size,
                    )
                })?;
            let entry_off_u64 = u64::try_from(entry_off).unwrap_or(u64::MAX);
            let entry_end = entry_off
                .checked_add(4)
                .ok_or_else(|| section_oob(entry_off_u64, 4))?;

            let key_bytes = self
                .mmap
                .get(entry_off..entry_end)
                .ok_or_else(|| section_oob(entry_off_u64, 4))?;
            let key_val: [u8; 4] = key_bytes
                .try_into()
                .map_err(|_| section_oob(entry_off_u64, 4))?;
            let key = u32::from_le_bytes(key_val);

            match key.cmp(&trigram) {
                std::cmp::Ordering::Equal => {
                    let full_end = entry_off
                        .checked_add(entry_size)
                        .ok_or_else(|| section_oob(entry_off_u64, entry_size_u64))?;
                    let entry = self
                        .mmap
                        .get(entry_off..full_end)
                        .ok_or_else(|| section_oob(entry_off_u64, entry_size_u64))?;

                    let &[b0, b1, b2, b3, b4, b5] = entry
                        .get(4..10)
                        .ok_or_else(|| section_oob(entry_off_u64, entry_size_u64))?
                    else {
                        return Err(section_oob(entry_off_u64, entry_size_u64));
                    };
                    let posting_offset = u64::from_le_bytes([b0, b1, b2, b3, b4, b5, 0, 0]);

                    let posting_length = entry
                        .get(10..14)
                        .and_then(|s| s.try_into().ok())
                        .map(u32::from_le_bytes)
                        .ok_or_else(|| section_oob(entry_off_u64, entry_size_u64))?;

                    let doc_frequency = entry
                        .get(14..18)
                        .and_then(|s| s.try_into().ok())
                        .map(u32::from_le_bytes)
                        .ok_or_else(|| section_oob(entry_off_u64, entry_size_u64))?;

                    return Ok(Some(TrigramInfo {
                        posting_offset,
                        posting_length,
                        doc_frequency,
                    }));
                }
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
            }
        }

        Ok(None)
    }

    fn decode_cdx_block(&self, block_idx: usize) -> Result<Option<Vec<u8>>> {
        // Bound single decompressed CDX block (1024 entries x ~25 B max varints << 16 MiB).
        const MAX_CDX_BLOCK_BYTES: usize = 16 * 1024 * 1024;
        use std::io::Read;

        let Some(block_entry) = self.cdx_blocks.get(block_idx) else {
            return Ok(None);
        };
        let block_end = if let Some(next) = self.cdx_blocks.get(block_idx + 1) {
            next.block_offset
        } else {
            self.header
                .trigram_table_offset
                .checked_add(self.header.trigram_table_size)
                .ok_or_else(|| Error::CdxBlockCorrupted("trigram table end overflow".into()))?
        };
        let block_start = usize::try_from(block_entry.block_offset).map_err(|_| {
            Error::CdxBlockCorrupted(format!(
                "cdx block {block_idx} start offset overflow: {}",
                block_entry.block_offset
            ))
        })?;
        let block_end = usize::try_from(block_end).map_err(|_| {
            Error::CdxBlockCorrupted(format!(
                "cdx block {block_idx} end offset overflow: {block_end}"
            ))
        })?;
        let block_data = self.mmap.get(block_start..block_end).ok_or_else(|| {
            Error::CdxBlockCorrupted(format!(
                "cdx block {block_idx} range {block_start}..{block_end} out of bounds (len={})",
                self.mmap.len()
            ))
        })?;

        let limit = u64::try_from(MAX_CDX_BLOCK_BYTES).unwrap_or(u64::MAX);
        let decoder = zstd::stream::Decoder::new(block_data)
            .map_err(|e| Error::CdxBlockCorrupted(format!("zstd decoder init failed: {e}")))?;
        let mut decompressed = Vec::new();
        decoder
            .take(limit.saturating_add(1))
            .read_to_end(&mut decompressed)
            .map_err(|e| Error::CdxBlockCorrupted(format!("zstd decompression failed: {e}")))?;
        if u64::try_from(decompressed.len()).unwrap_or(u64::MAX) > limit {
            return Err(Error::CdxBlockCorrupted(format!(
                "cdx block exceeds {MAX_CDX_BLOCK_BYTES} byte cap"
            )));
        }
        Ok(Some(decompressed))
    }

    fn get_trigram_cdx(&self, trigram: Trigram) -> Result<Option<TrigramInfo>> {
        let idx = self
            .cdx_blocks
            .partition_point(|entry| entry.first_key <= trigram);
        if idx == 0 {
            return Ok(None);
        }
        let Some(decompressed) = self.decode_cdx_block(idx - 1)? else {
            return Ok(None);
        };

        let mut pos = 0;
        let num_entries = match crate::varint::decode(&decompressed, &mut pos) {
            Ok(v) => usize::try_from(v)
                .map_err(|_| Error::CdxBlockCorrupted("num_entries overflow".into()))?,
            Err(e) => {
                return Err(Error::CdxBlockCorrupted(format!(
                    "num_entries varint decode failed: {e}"
                )));
            }
        };

        let mut last_key = 0u32;
        for _ in 0..num_entries {
            let key_delta = match crate::varint::decode(&decompressed, &mut pos) {
                Ok(v) => u32::try_from(v)
                    .map_err(|_| Error::CdxBlockCorrupted("key_delta overflow".into()))?,
                Err(e) => {
                    return Err(Error::CdxBlockCorrupted(format!(
                        "key_delta varint decode failed: {e}"
                    )));
                }
            };
            let key = last_key
                .checked_add(key_delta)
                .ok_or_else(|| Error::CdxBlockCorrupted("cdx trigram key overflow".into()))?;
            last_key = key;

            let posting_offset = match crate::varint::decode(&decompressed, &mut pos) {
                Ok(v) => v,
                Err(e) => {
                    return Err(Error::CdxBlockCorrupted(format!(
                        "posting_offset varint decode failed: {e}"
                    )));
                }
            };
            let posting_length = match crate::varint::decode(&decompressed, &mut pos) {
                Ok(v) => u32::try_from(v)
                    .map_err(|_| Error::CdxBlockCorrupted("posting_length overflow".into()))?,
                Err(e) => {
                    return Err(Error::CdxBlockCorrupted(format!(
                        "posting_length varint decode failed: {e}"
                    )));
                }
            };
            let doc_frequency = match crate::varint::decode(&decompressed, &mut pos) {
                Ok(v) => u32::try_from(v)
                    .map_err(|_| Error::CdxBlockCorrupted("doc_frequency overflow".into()))?,
                Err(e) => {
                    return Err(Error::CdxBlockCorrupted(format!(
                        "doc_frequency varint decode failed: {e}"
                    )));
                }
            };

            if key == trigram {
                return Ok(Some(TrigramInfo {
                    posting_offset,
                    posting_length,
                    doc_frequency,
                }));
            }
            if key > trigram {
                break;
            }
        }

        Ok(None)
    }

    /// Decode the posting list for a given trigram info.
    ///
    /// # Errors
    ///
    /// Returns an error if the posting data is out of bounds or corrupted.
    pub fn decode_postings(&self, info: &TrigramInfo) -> Result<PostingList> {
        let start = usize::try_from(info.posting_offset).map_err(|_| Error::PostingOutOfBounds)?;
        let len = usize::try_from(info.posting_length).map_err(|_| Error::PostingOutOfBounds)?;
        let end = start.checked_add(len).ok_or(Error::PostingOutOfBounds)?;
        let data = self.mmap.get(start..end).ok_or(Error::PostingOutOfBounds)?;
        PostingList::decode(data)
    }

    /// Retrieve file metadata by its ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the file ID is out of bounds or the file table entry
    /// is malformed.
    pub fn get_file(&self, file_id: u32) -> Result<FileInfo> {
        if file_id >= self.header.file_count {
            return Err(Error::FileIdOutOfBounds(file_id));
        }

        let file_len_u64 = u64::try_from(self.mmap.len()).unwrap_or(u64::MAX);
        let file_table_off = usize::try_from(self.header.file_table_offset).map_err(|_| {
            Error::SectionOutOfBounds {
                section: "file_entry",
                offset: self.header.file_table_offset,
                size: u64::try_from(FILE_ENTRY_SIZE).unwrap_or(48),
                file_len: file_len_u64,
            }
        })?;
        let fid_usize = usize::try_from(file_id).map_err(|_| Error::FileIdOutOfBounds(file_id))?;
        let rel_off = fid_usize
            .checked_mul(FILE_ENTRY_SIZE)
            .ok_or(Error::FileIdOutOfBounds(file_id))?;
        let entry_off = file_table_off
            .checked_add(rel_off)
            .ok_or(Error::FileIdOutOfBounds(file_id))?;
        let entry_off_u64 = u64::try_from(entry_off).unwrap_or(u64::MAX);
        let entry_size_u64 = u64::try_from(FILE_ENTRY_SIZE).unwrap_or(48);
        let entry_end =
            entry_off
                .checked_add(FILE_ENTRY_SIZE)
                .ok_or(Error::SectionOutOfBounds {
                    section: "file_entry",
                    offset: entry_off_u64,
                    size: entry_size_u64,
                    file_len: file_len_u64,
                })?;

        let entry = self
            .mmap
            .get(entry_off..entry_end)
            .ok_or(Error::SectionOutOfBounds {
                section: "file_entry",
                offset: entry_off_u64,
                size: entry_size_u64,
                file_len: file_len_u64,
            })?;

        let entry_oob = || Error::SectionOutOfBounds {
            section: "file_entry",
            offset: entry_off_u64,
            size: entry_size_u64,
            file_len: file_len_u64,
        };
        let path_off = u32::from_le_bytes(
            entry
                .get(4..8)
                .and_then(|s| s.try_into().ok())
                .ok_or_else(entry_oob)?,
        );
        let status_byte = entry.get(10).copied().ok_or_else(entry_oob)?;
        let status = FileStatus::from_u8(status_byte);
        let mtime_ns = u64::from_le_bytes(
            entry
                .get(12..20)
                .and_then(|s| s.try_into().ok())
                .ok_or_else(entry_oob)?,
        );
        let size_bytes = u64::from_le_bytes(
            entry
                .get(20..28)
                .and_then(|s| s.try_into().ok())
                .ok_or_else(entry_oob)?,
        );
        let content_hash = u64::from_le_bytes(
            entry
                .get(28..36)
                .and_then(|s| s.try_into().ok())
                .ok_or_else(entry_oob)?,
        );

        let path = self.string_pool.resolve(path_off)?;

        // Resolve relative paths against the index root so that file I/O
        // works regardless of the caller's current working directory.
        let resolved_path = {
            let p = PathBuf::from(&path);
            if p.is_relative() {
                self.root.join(p)
            } else {
                p
            }
        };

        Ok(FileInfo {
            file_id,
            path: resolved_path,
            status,
            mtime_ns,
            size_bytes,
            content_hash,
        })
    }

    /// Check if a bloom filter for a file may contain a trigram.
    ///
    /// Returns `true` if the trigram may be present (conservative), if `file_id`
    /// belongs to the delta index (`>= header.file_count`), or if any error
    /// occurs reading the bloom data.
    #[must_use]
    pub fn bloom_may_contain(&self, file_id: u32, trigram: Trigram) -> bool {
        if !self.header.has_bloom() || file_id >= self.header.file_count {
            return true;
        }

        let Ok(file_table_off) = usize::try_from(self.header.file_table_offset) else {
            return true;
        };
        let Ok(fid_usize) = usize::try_from(file_id) else {
            return true;
        };
        let Some(entry_off) = fid_usize
            .checked_mul(FILE_ENTRY_SIZE)
            .and_then(|rel| file_table_off.checked_add(rel))
        else {
            return true;
        };
        let Some(off_start) = entry_off.checked_add(40) else {
            return true;
        };
        let Some(off_end) = entry_off.checked_add(44) else {
            return true;
        };
        let Some(bloom_bytes) = self.mmap.get(off_start..off_end) else {
            return true;
        };

        let bloom_rel_off = match bloom_bytes.try_into() {
            Ok(b) => u32::from_le_bytes(b),
            Err(_) => return true,
        };
        let Ok(bloom_base) = usize::try_from(self.header.bloom_offset) else {
            return true;
        };
        let Ok(bloom_rel) = usize::try_from(bloom_rel_off) else {
            return true;
        };
        let Some(bloom_abs_off) = bloom_base.checked_add(bloom_rel) else {
            return true;
        };

        let Some(hdr_end) = bloom_abs_off.checked_add(2) else {
            return true;
        };
        let Some(size_bytes) = self.mmap.get(bloom_abs_off..hdr_end) else {
            return true;
        };
        let size = match size_bytes.try_into() {
            Ok(b) => usize::from(u16::from_le_bytes(b)),
            Err(_) => return true,
        };

        let Some(hash_off) = bloom_abs_off.checked_add(2) else {
            return true;
        };
        let num_hashes = self.mmap.get(hash_off).copied().unwrap_or(0);
        let Some(bits_start) = bloom_abs_off.checked_add(4) else {
            return true;
        };
        let Some(bits_end) = bits_start.checked_add(size) else {
            return true;
        };
        let Some(bits) = self.mmap.get(bits_start..bits_end) else {
            return true;
        };

        BloomFilter::slice_contains(bits, num_hashes, trigram)
    }

    /// Retrieve high-level shard metadata without parsing the full header.
    #[must_use]
    pub const fn metadata(&self) -> ShardMetadata {
        ShardMetadata {
            shard_timestamp: self.header.created_at,
            file_count: self.header.file_count,
            trigram_count: self.header.trigram_count,
        }
    }

    /// Compute a stable 64-bit generation fingerprint for this shard.
    ///
    /// Hashes header generation metadata (`created_at`, `file_count`,
    /// `trigram_count`, `source_bytes_total`) together with the mmap byte
    /// length (and Unix inode when available) so shared caches can detect
    /// out-of-band shard swaps automatically.
    #[must_use]
    pub fn shard_fingerprint(&self) -> u64 {
        let mut buf = [0u8; 40];
        buf[0..8].copy_from_slice(&self.header.created_at.to_le_bytes());
        buf[8..12].copy_from_slice(&self.header.file_count.to_le_bytes());
        buf[12..16].copy_from_slice(&self.header.trigram_count.to_le_bytes());
        buf[16..24].copy_from_slice(&self.header.source_bytes_total.to_le_bytes());
        let mmap_len = u64::try_from(self.mmap.len()).unwrap_or(u64::MAX);
        buf[24..32].copy_from_slice(&mmap_len.to_le_bytes());
        #[cfg(unix)]
        let inode_bits = self.inode.unwrap_or(0);
        #[cfg(not(unix))]
        let inode_bits = 0u64;
        buf[32..40].copy_from_slice(&inode_bits.to_le_bytes());
        xxhash_rust::xxh64::xxh64(&buf, 0)
    }

    /// Detect whether the shard file on disk has been rebuilt under this live mmap.
    ///
    /// Returns `true` if the inode or file size differs, or if the file no longer exists.
    /// A stale reader should be dropped and reopened.
    ///
    /// On Unix: uses inode comparison (inode changes on atomic rename).
    /// On non-Unix: uses file size comparison only (Windows file locking prevents
    /// rebuild under live mmap, so size-only detection is sufficient).
    #[must_use]
    pub fn is_stale(&self, path: &Path) -> bool {
        let Ok(current) = std::fs::metadata(path) else {
            return true;
        };

        if u64::try_from(self.mmap.len()).ok() != Some(current.len()) {
            return true;
        }

        #[cfg(unix)]
        {
            if let Some(stored_inode) = self.inode
                && current.ino() != stored_inode
            {
                return true;
            }
        }

        false
    }
}

/// Metadata for a single file stored in the delta index.
///
/// Public because it appears in the public [`DeltaReader::id_to_fileinfo`] field.
#[derive(Debug, Clone)]
pub struct DeltaFileInfo {
    /// Absolute path to the file on disk.
    pub path: PathBuf,
    /// Last modification time in nanoseconds since the Unix epoch.
    pub mtime: u64,
    /// File size in bytes at index time.
    pub size: u64,
    /// XXH64 content hash computed at index time.
    pub hash: u64,
    /// Raw bloom filter bytes (260 bytes).
    pub bloom_bytes: Vec<u8>,
}

/// Delta index reader for incremental index updates.
#[derive(Default)]
pub struct DeltaReader {
    /// Set of tombstoned file IDs.
    pub tombstones: HashSet<u32>,
    /// Delta trigram postings (trigram to entries).
    pub postings: HashMap<u32, Vec<crate::posting::PostingEntry>>,
    /// Mapping from file path to file ID (path → id).
    pub path_to_id: HashMap<PathBuf, u32>,
    /// Mapping from file ID to file metadata (id → info).
    pub id_to_fileinfo: HashMap<u32, DeltaFileInfo>,
    /// Total number of file entries in this delta.
    pub total_file_entries: u32,
}

impl DeltaReader {
    /// Open and parse a delta index file.
    ///
    /// Returns `Ok(Default::default())` if the delta file does not exist (`NotFound`),
    /// and returns `Err(...)` if the file cannot be read, has an invalid header magic,
    /// contains truncated/corrupted records, or carries an unknown record tag.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read or is corrupted.
    pub fn open(path: &std::path::Path) -> crate::error::Result<Self> {
        // Cap for corrupt delta offsets_len to avoid OOM (defense-in-depth).
        const MAX_DELTA_OFFSETS: usize = 1 << 24;
        use std::io::Read;
        let mut file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };

        let mut reader = Self::default();
        let mut magic = [0u8; 4];
        file.read_exact(&mut magic)?;
        if magic != DELTA_MAGIC {
            return Err(Error::DeltaCorrupted("delta file magic mismatch".into()));
        }

        let mut type_buf = [0u8; 1];
        loop {
            match file.read_exact(&mut type_buf) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
            match type_buf[0] {
                DELTA_TOMBSTONE => {
                    let mut id_buf = [0u8; 4];
                    file.read_exact(&mut id_buf)?;
                    reader.tombstones.insert(u32::from_le_bytes(id_buf));
                }
                DELTA_FILE_ENTRY => {
                    let mut id_buf = [0u8; 4];
                    file.read_exact(&mut id_buf)?;
                    let file_id = u32::from_le_bytes(id_buf);

                    let mut len_buf = [0u8; 2];
                    file.read_exact(&mut len_buf)?;
                    let path_len = usize::from(u16::from_le_bytes(len_buf));

                    let mut path_buf = vec![0u8; path_len];
                    file.read_exact(&mut path_buf)?;
                    let path = PathBuf::from(String::from_utf8_lossy(&path_buf).into_owned());

                    let mut u64_buf = [0u8; 8];
                    file.read_exact(&mut u64_buf)?;
                    let mtime = u64::from_le_bytes(u64_buf);
                    file.read_exact(&mut u64_buf)?;
                    let size = u64::from_le_bytes(u64_buf);
                    file.read_exact(&mut u64_buf)?;
                    let hash = u64::from_le_bytes(u64_buf);

                    let mut bloom_buf = vec![0u8; 260];
                    file.read_exact(&mut bloom_buf)?;

                    reader.id_to_fileinfo.insert(
                        file_id,
                        DeltaFileInfo {
                            path: path.clone(),
                            mtime,
                            size,
                            hash,
                            bloom_bytes: bloom_buf,
                        },
                    );
                    reader.path_to_id.insert(path, file_id);
                    reader.total_file_entries =
                        reader.total_file_entries.checked_add(1).ok_or_else(|| {
                            Error::DeltaCorrupted("delta file entry count overflow".into())
                        })?;
                }
                DELTA_TRIGRAM_ENTRY => {
                    let mut buf32 = [0u8; 4];
                    file.read_exact(&mut buf32)?;
                    let trigram = u32::from_le_bytes(buf32);

                    file.read_exact(&mut buf32)?;

                    file.read_exact(&mut buf32)?;
                    let file_id = u32::from_le_bytes(buf32);

                    file.read_exact(&mut buf32)?;
                    let raw_offsets_len = u32::from_le_bytes(buf32);
                    let offsets_len = usize::try_from(raw_offsets_len).unwrap_or(usize::MAX);
                    if offsets_len > MAX_DELTA_OFFSETS {
                        return Err(Error::DeltaCorrupted(format!(
                            "delta offsets_len ({raw_offsets_len}) exceeds cap ({MAX_DELTA_OFFSETS})"
                        )));
                    }

                    let mut offsets = Vec::with_capacity(offsets_len);
                    for _ in 0..offsets_len {
                        file.read_exact(&mut buf32)?;
                        offsets.push(u32::from_le_bytes(buf32));
                    }

                    reader
                        .postings
                        .entry(trigram)
                        .or_default()
                        .push(crate::posting::PostingEntry { file_id, offsets });
                }
                tag => {
                    return Err(Error::DeltaCorrupted(format!(
                        "unknown delta record tag: 0x{tag:02x}"
                    )));
                }
            }
        }
        Ok(reader)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ── Rule 1: Error Path Tests ──────────────────────────────────────

    /// `Reader::open` on an empty file must return Err (`IndexTooSmall` or `Io`).
    #[test]
    fn test_reader_empty_file_error() {
        let mut tmp = tempfile::tempfile().expect("create tempfile");
        tmp.write_all(b"").expect("write");
        // Get the file path (on Unix we can read from /proc/self/fd)
        // Simpler: create a tempfile via tempfile crate
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("empty.ix");
        std::fs::write(&path, b"").expect("write empty");
        let result = Reader::open(&path);
        assert!(result.is_err(), "empty file should fail Reader::open");
        // Should be IndexTooSmall, but Io is also acceptable
        assert!(matches!(&result, Err(Error::IndexTooSmall | Error::Io(_))));
    }

    /// `DeltaReader::open` on a file with wrong magic must return Err.
    #[test]
    fn test_delta_reader_corrupt_magic_error() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("delta.ixd");
        // Write a file that exists but has wrong magic (not "IXDL")
        std::fs::write(&path, b"XXXXhello world extra bytes").expect("write corrupt delta");
        let result = DeltaReader::open(&path);
        assert!(result.is_err(), "wrong magic should fail DeltaReader::open");
        match &result {
            Err(Error::DeltaCorrupted(msg)) => {
                assert!(
                    msg.contains("magic"),
                    "expected magic mismatch message, got: {msg}"
                );
            }
            Err(other) => {
                panic!("expected DeltaCorrupted error about magic mismatch, got: {other:?}")
            }
            Ok(_) => panic!("expected DeltaCorrupted error about magic mismatch, got Ok"),
        }
    }

    /// `DeltaReader::open` returns Ok(default) when the file does not exist (not an error).
    #[test]
    fn test_delta_reader_missing_file_returns_default() {
        let path = std::path::PathBuf::from("/tmp/ix_test_nonexistent_delta_xyzzy.ixd");
        let result = DeltaReader::open(&path);
        assert!(
            result.is_ok(),
            "missing file should return default, not error"
        );
        let reader = result.expect("missing delta returns default");
        assert!(reader.tombstones.is_empty());
        assert!(reader.postings.is_empty());
        assert_eq!(reader.total_file_entries, 0);
    }

    #[test]
    fn test_bloom_may_contain_out_of_range_file_id_returns_true() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "hello world\n").expect("write");
        let mut builder = crate::builder::Builder::new(dir.path()).expect("builder");
        builder.build().expect("build");
        let reader = Reader::open(&dir.path().join(".ix/shard.ix")).expect("open");

        let bogus_tri = u32::from_le_bytes([b'x', b'y', b'z', 0]);
        assert!(
            reader.bloom_may_contain(reader.header.file_count, bogus_tri),
            "delta file_id >= header.file_count must conservatively pass bloom check"
        );
        assert!(
            reader.bloom_may_contain(u32::MAX, bogus_tri),
            "extreme out-of-range file_id must return true without OOB panic"
        );
    }

    #[test]
    fn test_decode_postings_offset_overflow_returns_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "alpha beta\n").expect("write");
        let mut builder = crate::builder::Builder::new(dir.path()).expect("builder");
        builder.build().expect("build");
        let reader = Reader::open(&dir.path().join(".ix/shard.ix")).expect("open");

        let info = TrigramInfo {
            posting_offset: u64::MAX - 4,
            posting_length: 16,
            doc_frequency: 1,
        };
        assert!(matches!(
            reader.decode_postings(&info),
            Err(Error::PostingOutOfBounds)
        ));
    }

    #[test]
    fn test_delta_reader_rejects_oversized_offsets_and_unknown_tag() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Unknown record tag must fail instead of truncating silently.
        let unknown_path = dir.path().join("unknown_tag.delta");
        let mut bad_tag = Vec::from(DELTA_MAGIC);
        bad_tag.push(0x7F);
        std::fs::write(&unknown_path, bad_tag).expect("write");
        assert!(DeltaReader::open(&unknown_path).is_err());

        // Oversized offsets_len (> MAX_DELTA_OFFSETS = 1 << 24) must fail loudly.
        let huge_offsets_path = dir.path().join("huge_offsets.delta");
        let mut buf = Vec::from(DELTA_MAGIC);
        buf.push(DELTA_TRIGRAM_ENTRY);
        buf.extend_from_slice(&0x0061_6263u32.to_le_bytes()); // trigram
        buf.extend_from_slice(&0u32.to_le_bytes()); // unused field
        buf.extend_from_slice(&0u32.to_le_bytes()); // file_id
        buf.extend_from_slice(&((1u32 << 24) + 1).to_le_bytes()); // > MAX_DELTA_OFFSETS
        std::fs::write(&huge_offsets_path, buf).expect("write");
        assert!(DeltaReader::open(&huge_offsets_path).is_err());
    }

    #[test]
    fn test_get_last_modified_excludes_codegraph_and_honors_ixd_toml() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).expect("mkdir src");
        std::fs::create_dir_all(root.join(".codegraph")).expect("mkdir .codegraph");
        std::fs::create_dir_all(root.join("outer")).expect("mkdir outer");

        std::fs::write(root.join(".ixd.toml"), "watch_roots = [\"src\"]\n").expect("cfg");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("src");
        let before = Reader::get_last_modified(root).expect("mtime before");

        std::thread::sleep(std::time::Duration::from_millis(25));
        std::fs::write(root.join(".codegraph/index.db"), "graph").expect("codegraph");
        std::fs::write(root.join("outer/ignored.txt"), "outer").expect("outer");
        let after = Reader::get_last_modified(root).expect("mtime after");
        assert_eq!(
            before, after,
            "excluded paths must not advance last_modified"
        );
    }
}
