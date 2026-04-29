//! Index reader — the mmap-based query-time interface.
//!
//! Fast, zero-copy access to the index data.

use crate::bloom::BloomFilter;
use crate::error::{Error, Result};
use crate::format::{FILE_ENTRY_SIZE, FileStatus, HEADER_SIZE, Header};
use crate::posting::PostingList;
use crate::string_pool::StringPoolReader;
use crate::trigram::Trigram;
use memmap2::Mmap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// Lightweight snapshot of shard-level metadata (no mmap needed).
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
pub struct CdxBlockEntry {
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
    inode: Option<u64>,
    cdx_blocks: Vec<CdxBlockEntry>,
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

#[allow(clippy::as_conversions)] // binary format: usize/u32/u64 casts for index decoding
#[allow(clippy::indexing_slicing)] // binary format: fixed-size buffer ops, length-checked
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

        if mmap.len() < HEADER_SIZE {
            return Err(Error::IndexTooSmall);
        }

        let header = Header::parse(&mmap[0..HEADER_SIZE])?;
        header.validate_bounds(mmap.len() as u64)?;

        #[cfg(unix)]
        let inode = Some(file.metadata()?.ino());

        #[cfg(not(unix))]
        let inode = None;

        // SAFETY: We transmute the slice lifetime to 'static. This is sound because:
        // INVARIANT: Reader owns the Mmap, which owns the underlying memory.
        // INVARIANT: Mmap's data remains valid for the entire lifetime of Reader.
        // INVARIANT: No mutable access to mmap occurs after construction.
        // INVARIANT: StringPoolReader<'static> cannot outlive Reader (it's a field).
        // This is the standard pattern for self-referential mmap structs in Rust.
        let string_pool_data: &'static [u8] = unsafe {
            let start = header.string_pool_offset as usize;
            let end = (header.string_pool_offset + header.string_pool_size) as usize;
            std::mem::transmute::<&[u8], &'static [u8]>(&mmap[start..end])
        };
        let string_pool = StringPoolReader::new(string_pool_data)?;

        let cdx_blocks = if header.has_cdx() && header.cdx_block_index_size > 0 {
            let idx_start = header.cdx_block_index_offset as usize;
            let idx_end = idx_start + header.cdx_block_index_size as usize;
            let idx_data = mmap
                .get(idx_start..idx_end)
                .ok_or(Error::SectionOutOfBounds {
                    section: "cdx_block_index",
                    offset: header.cdx_block_index_offset,
                    size: header.cdx_block_index_size,
                    file_len: mmap.len() as u64,
                })?;
            let mut blocks = Vec::new();
            let mut pos = 0;
            while pos + 12 <= idx_data.len() {
                let first_key = u32::from_le_bytes(
                    idx_data[pos..pos + 4]
                        .try_into()
                        .map_err(|_| Error::Config("bad cdx key".into()))?,
                );
                if first_key == u32::MAX {
                    break;
                }
                let block_offset = u64::from_le_bytes(
                    idx_data[pos + 4..pos + 12]
                        .try_into()
                        .map_err(|_| Error::Config("bad cdx offset".into()))?,
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

        Ok(Self {
            mmap,
            header,
            string_pool,
            inode,
            cdx_blocks,
        })
    }

    /// Get the last modification time among all source files in the tree.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory walk fails or metadata cannot be read.
    pub fn get_last_modified(root: &Path) -> Result<u64> {
        let mut last_modified = 0u64;
        let walker = ignore::WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(true)
            .require_git(false)
            .add_custom_ignore_filename(".ixignore")
            .filter_entry(move |entry| {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                if entry.file_type().is_some_and(|t| t.is_dir())
                    && matches!(
                        name,
                        "lost+found"
                            | ".git"
                            | "node_modules"
                            | "target"
                            | "__pycache__"
                            | ".tox"
                            | ".venv"
                            | "venv"
                            | ".ix"
                    )
                {
                    return false;
                }

                if entry.file_type().is_some_and(|t| t.is_file()) {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if matches!(
                        ext,
                        "so" | "o"
                            | "dylib"
                            | "a"
                            | "dll"
                            | "exe"
                            | "pyc"
                            | "jpg"
                            | "png"
                            | "gif"
                            | "mp4"
                            | "mp3"
                            | "pdf"
                            | "zip"
                            | "7z"
                            | "rar"
                            | "sqlite"
                            | "db"
                            | "bin"
                    ) || name.ends_with(".tar.gz")
                    {
                        return false;
                    }
                }
                true
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
                            .map_or(0, |d| d.as_micros() as u64);
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

    /// Binary search the trigram table. Returns `None` if the trigram
    /// is unknown.
    ///
    /// When CDX compression is active, performs a two-level search:
    /// first on the block index, then within the decompressed block.
    pub fn get_trigram(&self, trigram: Trigram) -> Option<TrigramInfo> {
        if self.header.has_cdx() && !self.cdx_blocks.is_empty() {
            return self.get_trigram_cdx(trigram);
        }

        // Legacy fallback (no CDX)
        let count = self.header.trigram_count as usize;
        let table_start = self.header.trigram_table_offset as usize;
        let entry_size = crate::format::TRIGRAM_ENTRY_SIZE;

        let mut low = 0;
        let mut high = count;

        while low < high {
            let mid = low + (high - low) / 2;
            let entry_off = table_start + mid * entry_size;

            let key_bytes = self.mmap.get(entry_off..entry_off + 4)?;
            let key = u32::from_le_bytes(key_bytes.try_into().ok()?);

            match key.cmp(&trigram) {
                std::cmp::Ordering::Equal => {
                    let entry = self.mmap.get(entry_off..entry_off + entry_size)?;

                    let mut off_bytes = [0u8; 8];
                    off_bytes[..6].copy_from_slice(&entry[4..10]);
                    let posting_offset = u64::from_le_bytes(off_bytes);

                    let posting_length = entry
                        .get(10..14)
                        .and_then(|s| s.try_into().ok())
                        .map(u32::from_le_bytes)?;

                    let doc_frequency = entry
                        .get(14..18)
                        .and_then(|s| s.try_into().ok())
                        .map(u32::from_le_bytes)?;

                    return Some(TrigramInfo {
                        posting_offset,
                        posting_length,
                        doc_frequency,
                    });
                }
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
            }
        }

        None
    }

    fn get_trigram_cdx(&self, trigram: Trigram) -> Option<TrigramInfo> {
        let mut block_idx = 0;
        for (i, entry) in self.cdx_blocks.iter().enumerate() {
            if entry.first_key > trigram {
                break;
            }
            block_idx = i;
        }

        let block_entry = self.cdx_blocks.get(block_idx)?;

        let block_end = self.cdx_blocks.get(block_idx + 1).map_or_else(
            || self.header.trigram_table_offset + self.header.trigram_table_size,
            |next| next.block_offset,
        );

        let block_start = block_entry.block_offset as usize;
        let block_end = block_end as usize;
        let block_data = self.mmap.get(block_start..block_end)?;

        let decompressed = match zstd::decode_all(block_data) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("ix: CDX block decompression failed: {e}");
                return None;
            }
        };

        let mut pos = 0;
        let num_entries =
            usize::try_from(crate::varint::decode(&decompressed, &mut pos).unwrap_or(0))
                .unwrap_or(0);

        let mut last_key = 0u32;
        for _ in 0..num_entries {
            let key_delta =
                u32::try_from(crate::varint::decode(&decompressed, &mut pos).unwrap_or(0))
                    .unwrap_or(0);
            let key = last_key + key_delta;
            last_key = key;

            let posting_offset = crate::varint::decode(&decompressed, &mut pos).unwrap_or(0);
            let posting_length =
                u32::try_from(crate::varint::decode(&decompressed, &mut pos).unwrap_or(0))
                    .unwrap_or(0);
            let doc_frequency =
                u32::try_from(crate::varint::decode(&decompressed, &mut pos).unwrap_or(0))
                    .unwrap_or(0);

            if key == trigram {
                return Some(TrigramInfo {
                    posting_offset,
                    posting_length,
                    doc_frequency,
                });
            }
            if key > trigram {
                break;
            }
        }

        None
    }

    /// Decode the posting list for a given trigram info.
    ///
    /// # Errors
    ///
    /// Returns an error if the posting data is out of bounds or corrupted.
    pub fn decode_postings(&self, info: &TrigramInfo) -> Result<PostingList> {
        let start = info.posting_offset as usize;
        let end = start + info.posting_length as usize;
        if end > self.mmap.len() {
            return Err(Error::PostingOutOfBounds);
        }
        PostingList::decode(&self.mmap[start..end])
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

        let entry_off = self.header.file_table_offset as usize + file_id as usize * FILE_ENTRY_SIZE;
        let entry = self
            .mmap
            .get(entry_off..entry_off + FILE_ENTRY_SIZE)
            .ok_or(Error::SectionOutOfBounds {
                section: "file_entry",
                offset: entry_off as u64,
                size: FILE_ENTRY_SIZE as u64,
                file_len: self.mmap.len() as u64,
            })?;

        let path_off = u32::from_le_bytes(
            entry[4..8]
                .try_into()
                .map_err(|_| Error::Config("invalid path offset".into()))?,
        );
        let status = FileStatus::from_u8(entry[10]);
        let mtime_ns = u64::from_le_bytes(
            entry[12..20]
                .try_into()
                .map_err(|_| Error::Config("invalid mtime".into()))?,
        );
        let size_bytes = u64::from_le_bytes(
            entry[20..28]
                .try_into()
                .map_err(|_| Error::Config("invalid size".into()))?,
        );
        let content_hash = u64::from_le_bytes(
            entry[28..36]
                .try_into()
                .map_err(|_| Error::Config("invalid hash".into()))?,
        );

        let path = self.string_pool.resolve(path_off)?;

        Ok(FileInfo {
            file_id,
            path: PathBuf::from(path),
            status,
            mtime_ns,
            size_bytes,
            content_hash,
        })
    }

    /// Check if a bloom filter for a file may contain a trigram.
    ///
    /// Returns `true` if the trigram may be present (conservative) or if
    /// any error occurs reading the bloom data (safe default: assume present).
    #[must_use]
    pub fn bloom_may_contain(&self, file_id: u32, trigram: Trigram) -> bool {
        if !self.header.has_bloom() {
            return true;
        }

        let entry_off = self.header.file_table_offset as usize + file_id as usize * FILE_ENTRY_SIZE;
        let Some(bloom_bytes) = self.mmap.get(entry_off + 40..entry_off + 44) else {
            return true;
        };

        let bloom_rel_off = match bloom_bytes.try_into() {
            Ok(b) => u32::from_le_bytes(b),
            Err(_) => return true,
        };
        let bloom_abs_off = self.header.bloom_offset as usize + bloom_rel_off as usize;

        let Some(size_bytes) = self.mmap.get(bloom_abs_off..bloom_abs_off + 2) else {
            return true;
        };
        let size = match size_bytes.try_into() {
            Ok(b) => u16::from_le_bytes(b),
            Err(_) => return true,
        } as usize;

        let num_hashes = self.mmap.get(bloom_abs_off + 2).copied().unwrap_or(0);
        let Some(bits) = self.mmap.get(bloom_abs_off + 4..bloom_abs_off + 4 + size) else {
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

        if current.len() as usize != self.mmap.len() {
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
