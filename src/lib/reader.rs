//! Index reader — the mmap-based query-time interface.
//!
//! Fast, zero-copy access to the index data.

use std::fs::File;
use std::path::{Path, PathBuf};
use memmap2::Mmap;
use crate::error::{Error, Result};
use crate::format::*;
use crate::trigram::Trigram;
use crate::posting::PostingList;
use crate::bloom::BloomFilter;
use crate::string_pool::StringPoolReader;

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
        
        let header = Header::parse(&mmap[0..HEADER_SIZE])?;
        header.validate_bounds(mmap.len() as u64)?;

        // Safety: we are extending the lifetime of the slice to 'static.
        // This is okay because 'Reader' owns the 'Mmap' which owns the data.
        let string_pool_data: &'static [u8] = unsafe {
            let slice = &mmap[header.string_pool_offset as usize .. (header.string_pool_offset + header.string_pool_size) as usize];
            std::mem::transmute(slice)
        };
        let string_pool = StringPoolReader::new(string_pool_data)?;

        Ok(Self {
            mmap,
            header,
            string_pool,
        })
    }

    pub fn get_trigram(&self, trigram: Trigram) -> Option<TrigramInfo> {
        let idx = trigram as usize;
        if idx >= TRIGRAM_SLOTS {
            return None;
        }

        let entry_off = self.header.trigram_table_offset as usize + idx * TRIGRAM_ENTRY_SIZE;
        let entry = &self.mmap[entry_off .. entry_off + TRIGRAM_ENTRY_SIZE];

        let mut off_bytes = [0u8; 8];
        off_bytes[..6].copy_from_slice(&entry[0..6]);
        let posting_offset = u64::from_le_bytes(off_bytes);

        if posting_offset == 0 {
            return None;
        }

        let posting_length = u32::from_le_bytes(entry[6..10].try_into().unwrap());
        let doc_frequency = u32::from_le_bytes(entry[10..14].try_into().unwrap());

        Some(TrigramInfo {
            posting_offset,
            posting_length,
            doc_frequency,
        })
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
        let entry = &self.mmap[entry_off .. entry_off + FILE_ENTRY_SIZE];

        let path_off = u32::from_le_bytes(entry[4..8].try_into().unwrap());
        let status = FileStatus::from_u8(entry[10]);
        let mtime_ns = u64::from_le_bytes(entry[12..20].try_into().unwrap());
        let size_bytes = u64::from_le_bytes(entry[20..28].try_into().unwrap());
        let content_hash = u64::from_le_bytes(entry[28..36].try_into().unwrap());

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
        let bloom_rel_off = u32::from_le_bytes(self.mmap[entry_off + 40 .. entry_off + 44].try_into().unwrap());
        
        let bloom_abs_off = self.header.bloom_offset as usize + bloom_rel_off as usize;
        if bloom_abs_off + 4 > self.mmap.len() {
            return true;
        }

        let size = u16::from_le_bytes(self.mmap[bloom_abs_off .. bloom_abs_off + 2].try_into().unwrap()) as usize;
        let num_hashes = self.mmap[bloom_abs_off + 2];
        let bits = &self.mmap[bloom_abs_off + 4 .. bloom_abs_off + 4 + size];

        BloomFilter::slice_contains(bits, num_hashes, trigram)
    }
}
