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
    #[allow(dead_code)]
    pub shard_timestamp: u64,
    /// Total number of files indexed in this shard.
    #[allow(dead_code)]
    pub file_count: u32,
    /// Total number of unique trigrams in this shard.
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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

        let root = path
            .parent()
            .and_then(|p| p.parent())
            .map_or_else(|| path.to_path_buf(), Path::to_path_buf);

        Ok(Self {
            mmap,
            header,
            string_pool,
            inode,
            cdx_blocks,
            root,
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
            .git_global(true)
            .git_exclude(true)
            .require_git(true) // within-repo .gitignore only; never ancestor ~/.gitignore (audit D4)
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
        let count = self.header.trigram_count as usize;
        let table_start = self.header.trigram_table_offset as usize;
        let entry_size = crate::format::TRIGRAM_ENTRY_SIZE;

        let mut low = 0;
        let mut high = count;

        while low < high {
            let mid = low + (high - low) / 2;
            let entry_off = table_start + mid * entry_size;

            let Some(key_bytes) = self.mmap.get(entry_off..entry_off + 4) else {
                return Ok(None);
            };
            let key_val = key_bytes
                .try_into()
                .map_err(|_| Error::Config("corrupt trigram table entry".into()))?;
            let key = u32::from_le_bytes(key_val);

            match key.cmp(&trigram) {
                std::cmp::Ordering::Equal => {
                    let Some(entry) = self.mmap.get(entry_off..entry_off + entry_size) else {
                        return Ok(None);
                    };

                    let mut off_bytes = [0u8; 8];
                    off_bytes[..6].copy_from_slice(&entry[4..10]);
                    let posting_offset = u64::from_le_bytes(off_bytes);

                    let Some(posting_length) = entry
                        .get(10..14)
                        .and_then(|s| s.try_into().ok())
                        .map(u32::from_le_bytes)
                    else {
                        tracing::warn!("corrupt trigram table entry: invalid posting_length");
                        return Ok(None);
                    };

                    let Some(doc_frequency) = entry
                        .get(14..18)
                        .and_then(|s| s.try_into().ok())
                        .map(u32::from_le_bytes)
                    else {
                        tracing::warn!("corrupt trigram table entry: invalid doc_frequency");
                        return Ok(None);
                    };

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

    fn get_trigram_cdx(&self, trigram: Trigram) -> Result<Option<TrigramInfo>> {
        let idx = self
            .cdx_blocks
            .partition_point(|entry| entry.first_key <= trigram);
        if idx == 0 {
            return Ok(None);
        }
        let block_idx = idx - 1;

        let Some(block_entry) = self.cdx_blocks.get(block_idx) else {
            return Ok(None);
        };

        let block_end = self.cdx_blocks.get(block_idx + 1).map_or_else(
            || self.header.trigram_table_offset + self.header.trigram_table_size,
            |next| next.block_offset,
        );

        let block_start = block_entry.block_offset as usize;
        let block_end = block_end as usize;
        let Some(block_data) = self.mmap.get(block_start..block_end) else {
            return Ok(None);
        };

        let decompressed = match zstd::decode_all(block_data) {
            Ok(d) => d,
            Err(e) => {
                return Err(Error::CdxBlockCorrupted(format!(
                    "zstd decompression failed: {e}"
                )));
            }
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
            let key = last_key + key_delta;
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
    #[allow(dead_code)]
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
    /// Returns `Default::default()` if the file does not exist or is invalid.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read.
    pub fn open(path: &std::path::Path) -> crate::error::Result<Self> {
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
            return Err(Error::Config("delta file magic mismatch".into()));
        }

        let mut type_buf = [0u8; 1];
        while file.read_exact(&mut type_buf).is_ok() {
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
                    let path_len = u16::from_le_bytes(len_buf) as usize;

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
                    reader.total_file_entries += 1;
                }
                DELTA_TRIGRAM_ENTRY => {
                    let mut buf32 = [0u8; 4];
                    file.read_exact(&mut buf32)?;
                    let trigram = u32::from_le_bytes(buf32);

                    file.read_exact(&mut buf32)?;

                    file.read_exact(&mut buf32)?;
                    let file_id = u32::from_le_bytes(buf32);

                    file.read_exact(&mut buf32)?;
                    let offsets_len = u32::from_le_bytes(buf32) as usize;

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
                _ => break,
            }
        }
        Ok(reader)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
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
            Err(Error::Config(msg)) => {
                assert!(
                    msg.contains("magic"),
                    "expected magic mismatch message, got: {msg}"
                );
            }
            _ => panic!("expected Config error about magic mismatch"),
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
        let reader = result.unwrap();
        assert!(reader.tombstones.is_empty());
        assert!(reader.postings.is_empty());
        assert_eq!(reader.total_file_entries, 0);
    }
}
