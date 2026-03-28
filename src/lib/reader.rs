//! Index reader — the mmap-based query-time interface.
//!
//! Fast, zero-copy access to the index data.

use crate::bloom::BloomFilter;
use crate::error::{Error, Result};
use crate::format::*;
use crate::posting::PostingList;
use crate::string_pool::StringPoolReader;
use crate::trigram::Trigram;
use memmap2::Mmap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub struct Reader {
    mmap: Mmap,
    pub header: Header,
    string_pool: StringPoolReader<'static>,
}

#[derive(Debug)]
pub struct TrigramInfo {
    pub posting_offset: u64,
    pub posting_length: u32,
    pub doc_frequency: u32,
}

#[derive(Debug)]
pub struct FileInfo {
    pub file_id: u32,
    pub path: PathBuf,
    pub status: FileStatus,
    pub mtime_ns: u64,
    pub size_bytes: u64,
    pub content_hash: u64,
}

impl Reader {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };

        if mmap.len() < HEADER_SIZE {
            return Err(Error::IndexTooSmall);
        }

        let header = Header::parse(&mmap[0..HEADER_SIZE])?;
        header.validate_bounds(mmap.len() as u64)?;

        // Safety: we are extending the lifetime of the slice to 'static.
        // This is okay because 'Reader' owns the 'Mmap' which owns the data.
        let string_pool_data: &'static [u8] = unsafe {
            let slice = &mmap[header.string_pool_offset as usize
                ..(header.string_pool_offset + header.string_pool_size) as usize];
            std::mem::transmute(slice)
        };
        let string_pool = StringPoolReader::new(string_pool_data)?;

        Ok(Self {
            mmap,
            header,
            string_pool,
        })
    }

    pub fn get_last_modified(root: &Path) -> Result<u64> {
        let mut last_modified = 0u64;
        let walker = ignore::WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(true)
            .require_git(false)
            .add_custom_ignore_filename(".ixignore")
            .filter_entry(move |entry| {
                let path = entry.path();
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name == "target" || name == ".git" || name == "node_modules" || name == ".ix"
                    {
                        return false;
                    }
                }
                true
            })
            .build();

        for result in walker {
            let entry = result.map_err(|e| Error::Config(e.to_string()))?;
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                let metadata = entry.metadata().map_err(|e| Error::Config(e.to_string()))?;
                let mtime = metadata
                    .modified()
                    .and_then(|t| {
                        t.duration_since(UNIX_EPOCH)
                            .map_err(|_| std::io::Error::other("time went backwards"))
                    })
                    .map(|d| d.as_micros() as u64)
                    .unwrap_or(0);
                if mtime > last_modified {
                    last_modified = mtime;
                }
            }
        }
        Ok(last_modified)
    }

    pub fn get_trigram(&self, trigram: Trigram) -> Option<TrigramInfo> {
        let count = self.header.trigram_count as usize;
        let table_start = self.header.trigram_table_offset as usize;

        let mut low = 0;
        let mut high = count;

        while low < high {
            let mid = low + (high - low) / 2;
            let entry_off = table_start + mid * TRIGRAM_ENTRY_SIZE;

            // Read trigram key (first 4 bytes)
            let key_bytes = self.mmap.get(entry_off..entry_off + 4)?;
            let key = u32::from_le_bytes(key_bytes.try_into().ok()?);

            if key == trigram {
                let entry = self.mmap.get(entry_off..entry_off + TRIGRAM_ENTRY_SIZE)?;

                // Read posting_offset (u48, bytes 4..10)
                let mut off_bytes = [0u8; 8];
                off_bytes[..6].copy_from_slice(&entry[4..10]);
                let posting_offset = u64::from_le_bytes(off_bytes);

                let posting_length = entry
                    .get(10..14)
                    .and_then(|s| s.try_into().ok())
                    .map(u32::from_le_bytes)
                    .unwrap_or(0);
                let doc_frequency = entry
                    .get(14..18)
                    .and_then(|s| s.try_into().ok())
                    .map(u32::from_le_bytes)
                    .unwrap_or(0);

                return Some(TrigramInfo {
                    posting_offset,
                    posting_length,
                    doc_frequency,
                });
            } else if key < trigram {
                low = mid + 1;
            } else {
                high = mid;
            }
        }

        None
    }

    pub fn decode_postings(&self, info: &TrigramInfo) -> Result<PostingList> {
        let start = info.posting_offset as usize;
        let end = start + info.posting_length as usize;
        if end > self.mmap.len() {
            return Err(Error::PostingOutOfBounds);
        }
        PostingList::decode(&self.mmap[start..end])
    }

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

    pub fn bloom_may_contain(&self, file_id: u32, trigram: Trigram) -> bool {
        if !self.header.has_bloom() {
            return true;
        }

        let entry_off = self.header.file_table_offset as usize + file_id as usize * FILE_ENTRY_SIZE;
        let bloom_rel_off_bytes = self.mmap.get(entry_off + 40..entry_off + 44);
        if bloom_rel_off_bytes.is_none() {
            return true;
        }

        let bloom_rel_off = bloom_rel_off_bytes
            .and_then(|b| b.try_into().ok())
            .map(u32::from_le_bytes)
            .unwrap_or(0);

        let bloom_abs_off = self.header.bloom_offset as usize + bloom_rel_off as usize;
        if bloom_abs_off + 4 > self.mmap.len() {
            return true;
        }

        let size = u16::from_le_bytes(
            self.mmap[bloom_abs_off..bloom_abs_off + 2]
                .try_into()
                .unwrap_or([0u8; 2]),
        ) as usize;
        let num_hashes = self.mmap[bloom_abs_off + 2];
        let bits = match self.mmap.get(bloom_abs_off + 4..bloom_abs_off + 4 + size) {
            Some(b) => b,
            None => return true,
        };

        BloomFilter::slice_contains(bits, num_hashes, trigram)
    }
}
