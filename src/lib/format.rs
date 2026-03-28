//! Index file format constants and header parsing.
//!
//! All integers little-endian. All offsets absolute from file start.
//! Sections aligned to 8-byte boundaries.

/// Magic bytes: ASCII "IX01"
pub const MAGIC: [u8; 4] = [0x49, 0x58, 0x30, 0x31];

/// Current format version
pub const VERSION_MAJOR: u16 = 1;
pub const VERSION_MINOR: u16 = 0;

/// Fixed header size (256 bytes)
pub const HEADER_SIZE: usize = 256;

/// Trigram table: one entry per possible 3-byte combination
pub const TRIGRAM_SLOTS: usize = 16_777_216; // 256^3

/// Trigram entry size in bytes
pub const TRIGRAM_ENTRY_SIZE: usize = 16;

/// File entry size in bytes
pub const FILE_ENTRY_SIZE: usize = 48;

/// Header flags
pub mod flags {
    pub const HAS_BLOOM_FILTERS: u64 = 1 << 0;
    pub const HAS_CONTENT_HASHES: u64 = 1 << 1;
    pub const POSTING_LISTS_COMPRESSED: u64 = 1 << 2;
}

/// File status in the file table
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Fresh = 0x00,
    Stale = 0x01,
    Deleted = 0x02,
}

impl FileStatus {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x00 => Self::Fresh,
            0x01 => Self::Stale,
            0x02 => Self::Deleted,
            _ => Self::Stale, // unknown = treat as stale
        }
    }
}

/// Parsed index header.
#[derive(Debug, Clone)]
pub struct Header {
    pub version_major: u16,
    pub version_minor: u16,
    pub flags: u64,
    pub created_at: u64,
    pub source_bytes_total: u64,
    pub file_count: u32,
    pub trigram_count: u32,
    pub file_table_offset: u64,
    pub file_table_size: u64,
    pub trigram_table_offset: u64,
    pub trigram_table_size: u64,
    pub posting_data_offset: u64,
    pub posting_data_size: u64,
    pub bloom_offset: u64,
    pub bloom_size: u64,
    pub string_pool_offset: u64,
    pub string_pool_size: u64,
    pub name_index_offset: u64,
    pub name_index_size: u64,
}

impl Header {
    /// Parse header from the first 256 bytes of an index file.
    pub fn parse(data: &[u8]) -> crate::error::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(crate::error::Error::IndexTooSmall);
        }
        if data[0..4] != MAGIC {
            return Err(crate::error::Error::BadMagic);
        }

        let r = |off: usize| -> u64 { u64::from_le_bytes(data[off..off + 8].try_into().unwrap()) };
        let r16 = |off: usize| -> u16 {
            u16::from_le_bytes(data[off..off + 2].try_into().unwrap())
        };
        let r32 = |off: usize| -> u32 {
            u32::from_le_bytes(data[off..off + 4].try_into().unwrap())
        };

        let major = r16(0x04);
        let minor = r16(0x06);
        if major != VERSION_MAJOR {
            return Err(crate::error::Error::UnsupportedVersion {
                major,
                minor,
            });
        }

        // Validate CRC32C of header (bytes 0x00..0xF8)
        let expected_crc = r32(0xF8);
        let actual_crc = crc32c::crc32c(&data[0..0xF8]);
        if expected_crc != actual_crc {
            return Err(crate::error::Error::HeaderCorrupted {
                expected: expected_crc,
                actual: actual_crc,
            });
        }

        Ok(Header {
            version_major: major,
            version_minor: minor,
            flags: r(0x08),
            created_at: r(0x10),
            source_bytes_total: r(0x18),
            file_count: r32(0x20),
            trigram_count: r32(0x24),
            file_table_offset: r(0x28),
            file_table_size: r(0x30),
            trigram_table_offset: r(0x38),
            trigram_table_size: r(0x40),
            posting_data_offset: r(0x48),
            posting_data_size: r(0x50),
            bloom_offset: r(0x58),
            bloom_size: r(0x60),
            string_pool_offset: r(0x68),
            string_pool_size: r(0x70),
            name_index_offset: r(0x78),
            name_index_size: r(0x80),
        })
    }

    /// Validate all section offsets fit within the file.
    pub fn validate_bounds(&self, file_len: u64) -> crate::error::Result<()> {
        let check = |name: &'static str, off: u64, sz: u64| -> crate::error::Result<()> {
            if off + sz > file_len {
                Err(crate::error::Error::SectionOutOfBounds {
                    section: name,
                    offset: off,
                    size: sz,
                    file_len,
                })
            } else {
                Ok(())
            }
        };
        check("file_table", self.file_table_offset, self.file_table_size)?;
        check("trigram_table", self.trigram_table_offset, self.trigram_table_size)?;
        check("posting_data", self.posting_data_offset, self.posting_data_size)?;
        if self.bloom_size > 0 {
            check("bloom", self.bloom_offset, self.bloom_size)?;
        }
        check("string_pool", self.string_pool_offset, self.string_pool_size)?;
        if self.name_index_size > 0 {
            check("name_index", self.name_index_offset, self.name_index_size)?;
        }
        Ok(())
    }

    pub fn has_bloom(&self) -> bool {
        self.flags & flags::HAS_BLOOM_FILTERS != 0
    }
}
