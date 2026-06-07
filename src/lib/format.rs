//! Index file format constants and header parsing.
//!
//! All integers little-endian. All offsets absolute from file start.
//! Sections aligned to 8-byte boundaries.

/// Magic bytes identifying an ix index file (`b"IX01"`).
pub const MAGIC: [u8; 4] = [0x49, 0x58, 0x30, 0x31];

/// Major version of the on-disk format this library writes.
pub const VERSION_MAJOR: u16 = 1;

/// Minimum minor version required to read an index file.
pub const VERSION_MINOR: u16 = 3;

/// Size of the fixed header at the start of every index file (256 bytes).
pub const HEADER_SIZE: usize = 256;

/// On-disk size of one trigram-table entry (u32 key + 16-byte payload).
/// Legacy constant — unused when CDX index is present.
pub const TRIGRAM_ENTRY_SIZE: usize = 20;
/// Maximum number of trigram entries per CDX compressed block.
pub const CDX_BLOCK_SIZE: usize = 1024;

/// On-disk size of one file-table entry in the index.
pub const FILE_ENTRY_SIZE: usize = 48;

/// Magic bytes identifying a delta file (`b"IXDL"`).
pub const DELTA_MAGIC: [u8; 4] = [0x49, 0x58, 0x44, 0x4C];

/// Entry type byte for a tombstone record (`file_id` u32 LE follows).
pub const DELTA_TOMBSTONE: u8 = 0x01;
/// Entry type byte for a file entry (inline file table entry + path + bloom).
pub const DELTA_FILE_ENTRY: u8 = 0x02;
/// Entry type byte for a trigram posting record.
pub const DELTA_TRIGRAM_ENTRY: u8 = 0x03;

/// Bit-flag constants stored in the [`Header::flags`] field.
pub mod flags {
    /// The index contains per-trigram bloom filters.
    /// Always set by the current builder — verified via [`super::Header::has_bloom`].
    pub const HAS_BLOOM_FILTERS: u64 = 1 << 0;
    /// Per-file content hashes (`XXH64`) are stored in the file table.
    /// Set by the builder; verified if needed for delta deduplication.
    pub const HAS_CONTENT_HASHES: u64 = 1 << 1;
    /// Posting-list data is ZSTD-compressed.
    /// Always set by the current builder (postings are compressed in
    /// CDX blocks). Verified via [`super::Header::has_compressed_postings`].
    pub const POSTING_LISTS_COMPRESSED: u64 = 1 << 2;
    /// Each posting-list chunk carries an `XXHash64` checksum.
    /// Verified via [`super::Header::has_checksum`].
    pub const POSTING_LISTS_CHECKSUMMED: u64 = 1 << 3;
    /// Trigram table uses CDX (Concentrated Delta X) compression.
    /// Always-on since v1.3. Verified via [`super::Header::has_cdx`].
    pub const HAS_CDX_INDEX: u64 = 1 << 4;
}

/// Whether a file tracked by the index is current, out-of-date, or deleted.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    /// File exists on disk and is in sync with the index.
    Fresh = 0x00,
    /// File has been modified since it was last indexed.
    Stale = 0x01,
    /// File has been removed from the file system.
    Deleted = 0x02,
}

impl FileStatus {
    /// Decode a file status from its on-disk `u8` representation.
    ///
    /// Unknown values default to [`FileStatus::Stale`].
    #[must_use]
    pub const fn from_u8(v: u8) -> Self {
        match v {
            0x00 => Self::Fresh,
            0x02 => Self::Deleted,
            _ => Self::Stale, // unknown = treat as stale
        }
    }
}

/// Parsed contents of the fixed 256-byte index header.
///
/// All integer fields are stored little-endian in the file.
/// Offsets are absolute byte offsets from the start of the file.
#[derive(Debug, Clone)]
pub struct Header {
    /// Format major version (must equal [`VERSION_MAJOR`]).
    pub version_major: u16,
    /// Format minor version (must be ≥ [`VERSION_MINOR`]).
    pub version_minor: u16,
    /// Bit-field of feature flags (see [`flags`]).
    pub flags: u64,
    /// Unix timestamp in microseconds when the index was created.
    pub created_at: u64,
    /// Sum of byte-sizes of all source files when indexed.
    pub source_bytes_total: u64,
    /// Number of file entries in the file table.
    pub file_count: u32,
    /// Number of trigram entries in the trigram table.
    pub trigram_count: u32,
    /// Byte offset to the file table section.
    pub file_table_offset: u64,
    /// Byte length of the file table section.
    pub file_table_size: u64,
    /// Byte offset to the trigram lookup table.
    pub trigram_table_offset: u64,
    /// Byte length of the trigram lookup table.
    pub trigram_table_size: u64,
    /// Byte offset to the posting-list data blob.
    pub posting_data_offset: u64,
    /// Byte length of the posting-list data blob.
    pub posting_data_size: u64,
    /// Byte offset to the bloom-filter section (0 if absent).
    pub bloom_offset: u64,
    /// Byte length of the bloom-filter section (0 if absent).
    pub bloom_size: u64,
    /// Byte offset to the string pool section.
    pub string_pool_offset: u64,
    /// Byte length of the string pool section.
    pub string_pool_size: u64,
    /// Byte offset to the file-name index section (0 if absent).
    pub name_index_offset: u64,
    /// Byte length of the file-name index section (0 if absent).
    pub name_index_size: u64,
    /// Byte offset to the CDX block index (0 if absent).
    pub cdx_block_index_offset: u64,
    /// Byte length of the CDX block index (0 if absent).
    pub cdx_block_index_size: u64,
}

impl Header {
    /// Parse header from the first 256 bytes of an index file.
    ///
    /// # Errors
    ///
    /// Returns an error if the data is too small, has a bad magic number,
    /// unsupported version, or corrupted CRC.
    pub fn parse(data: &[u8]) -> crate::error::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(crate::error::Error::IndexTooSmall);
        }
        if data.get(0..4).ok_or(crate::error::Error::IndexTooSmall)? != MAGIC {
            return Err(crate::error::Error::BadMagic);
        }

        let r = |off: usize| -> u64 {
            data.get(off..off + 8)
                .and_then(|s| s.try_into().ok())
                .map_or(0, u64::from_le_bytes)
        };
        let r16 = |off: usize| -> u16 {
            data.get(off..off + 2)
                .and_then(|s| s.try_into().ok())
                .map_or(0, u16::from_le_bytes)
        };
        let r32 = |off: usize| -> u32 {
            data.get(off..off + 4)
                .and_then(|s| s.try_into().ok())
                .map_or(0, u32::from_le_bytes)
        };

        let major = r16(0x04);
        let minor = r16(0x06);
        if major != VERSION_MAJOR || minor < VERSION_MINOR {
            return Err(crate::error::Error::UnsupportedVersion { major, minor });
        }

        // Validate CRC32C of header (bytes 0x00..0xF8)
        let expected_crc = r32(0xF8);
        let actual_crc = crc32c::crc32c(
            data.get(0..0xF8)
                .ok_or(crate::error::Error::IndexTooSmall)?,
        );
        if expected_crc != actual_crc {
            return Err(crate::error::Error::HeaderCorrupted {
                expected: expected_crc,
                actual: actual_crc,
            });
        }

        Ok(Self {
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
            cdx_block_index_offset: r(0x88),
            cdx_block_index_size: r(0x90),
        })
    }

    /// Validate all section offsets fit within the file.
    ///
    /// # Errors
    ///
    /// Returns an error if any section extends beyond the file length.
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
        check(
            "trigram_table",
            self.trigram_table_offset,
            self.trigram_table_size,
        )?;
        check(
            "posting_data",
            self.posting_data_offset,
            self.posting_data_size,
        )?;
        if self.bloom_size > 0 {
            check("bloom", self.bloom_offset, self.bloom_size)?;
        }
        check(
            "string_pool",
            self.string_pool_offset,
            self.string_pool_size,
        )?;
        if self.name_index_size > 0 {
            check("name_index", self.name_index_offset, self.name_index_size)?;
        }
        if self.cdx_block_index_size > 0 {
            check(
                "cdx_block_index",
                self.cdx_block_index_offset,
                self.cdx_block_index_size,
            )?;
        }
        Ok(())
    }

    /// Returns `true` when the index includes per-trigram bloom filters.
    #[must_use]
    pub const fn has_bloom(&self) -> bool {
        self.flags & flags::HAS_BLOOM_FILTERS != 0
    }

    /// Returns `true` when the posting-list data is ZSTD-compressed
    /// ([`flags::POSTING_LISTS_COMPRESSED`]).
    #[must_use]
    pub const fn has_compressed_postings(&self) -> bool {
        self.flags & flags::POSTING_LISTS_COMPRESSED != 0
    }

    /// Returns `true` when posting-list chunks carry an `XXHash64` checksum
    /// ([`flags::POSTING_LISTS_CHECKSUMMED`]).
    #[must_use]
    pub const fn has_checksum(&self) -> bool {
        self.flags & flags::POSTING_LISTS_CHECKSUMMED != 0
    }

    /// Returns `true` when the trigram table uses CDX compression.
    #[must_use]
    pub const fn has_cdx(&self) -> bool {
        self.flags & flags::HAS_CDX_INDEX != 0
    }
}

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A heartbeat file written by the `ixd` daemon so other processes can
/// detect a running watcher and query its status.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Beacon {
    /// PID of the `ixd` daemon process.
    pub pid: i32,
    /// Canonical root directory being watched.
    pub root: PathBuf,
    /// Unix timestamp (seconds) when the daemon started.
    pub start_time: u64,
    /// Human-readable status (e.g. `"idle"`, `"indexing"`).
    pub status: String,
    /// Unix timestamp (seconds) of the last filesystem event.
    pub last_event_at: u64,
    /// Path to the Unix domain socket for real-time notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_path: Option<PathBuf>,
    /// Instance identifier (process start timestamp in nanos).
    /// Distinguishes multiple runs of the same PID.
    #[serde(default)]
    pub instance_id: u64,
}

impl Beacon {
    /// Create a new beacon for the current process, anchored at the given root.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self::with_instance_id(root, instance_id_now())
    }

    /// Create a new beacon with an explicit instance identifier.
    ///
    /// Use this when multiple roots are watched by the same process so that
    /// the concurrent-instance guard can distinguish stale beacons (same PID,
    /// different `instance_id`) from live duplicates (same PID, same
    /// `instance_id`).
    #[must_use]
    pub fn with_instance_id(root: &Path, instance_id: u64) -> Self {
        let pid = i32::try_from(std::process::id()).unwrap_or(0);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            pid,
            root: root.to_path_buf(),
            start_time: now,
            status: "idle".to_string(),
            last_event_at: now,
            socket_path: None,
            instance_id,
        }
    }

    /// Check whether the daemon described by this beacon is still running.
    ///
    /// Verifies the recorded PID still exists, belongs to an `ixd` binary,
    /// and the watched root directory is still accessible.
    #[must_use]
    pub fn is_live(&self) -> bool {
        #[cfg(unix)]
        {
            use nix::sys::signal::kill;
            use nix::unistd::Pid;

            if kill(Pid::from_raw(self.pid), None).is_err() {
                return false;
            }

            let comm_path = format!("/proc/{}/comm", self.pid);
            if let Ok(comm) = std::fs::read_to_string(&comm_path) {
                let comm = comm.trim();
                if comm != "ixd" {
                    return false;
                }
            } else {
                return false;
            }

            self.root.exists()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    /// Write the beacon to `beacon.json` in the given folder.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be created or serialization fails.
    pub fn write_to(&self, folder: &Path) -> crate::error::Result<()> {
        let path = folder.join("beacon.json");
        let f = std::fs::File::create(path)?;
        serde_json::to_writer_pretty(f, self).map_err(std::io::Error::other)?;
        Ok(())
    }

    /// Read a beacon from `beacon.json` in the given folder.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or deserialization fails.
    pub fn read_from(folder: &Path) -> crate::error::Result<Self> {
        let path = folder.join("beacon.json");
        let f = std::fs::File::open(path)?;
        let beacon = serde_json::from_reader(f).map_err(std::io::Error::other)?;
        Ok(beacon)
    }
}

/// Generate a per-run instance identifier from the process start time.
///
/// The value is `SystemTime::now()` in nanoseconds. It is stable within a
/// single `ixd` invocation and unique enough that two runs of the same PID
/// will produce different values, resolving the PID-reuse ambiguity in the
/// concurrent-instance beacon guard.
#[must_use]
pub fn instance_id_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// Centralized binary file detection.
///
/// Uses a heuristic based on the ratio of non-text bytes in the first 512 bytes.
/// Valid UTF-8 multi-byte sequences (2-4 bytes) are counted as text, not binary,
/// so files containing emoji or CJK characters are not falsely flagged.
#[must_use]
#[allow(clippy::cast_precision_loss, clippy::as_conversions)]
pub fn is_binary(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    let check_len = data.len().min(512);
    let slice = data.get(..check_len).unwrap_or(&[]);

    let mut non_text = 0usize;
    let mut i = 0;
    while i < slice.len() {
        let b = slice[i];
        if matches!(b, 0x09 | 0x0A | 0x0D | 0x20..=0x7E) {
            // ASCII printable/control — text
        } else if b & 0xC0 == 0xC0 {
            // Potential UTF-8 lead byte: decode the sequence
            let seq_len = if b & 0xE0 == 0xC0 {
                2
            } else if b & 0xF0 == 0xE0 {
                3
            } else if b & 0xF8 == 0xF0 {
                4
            } else {
                0
            };

            if seq_len > 0 && i + seq_len <= slice.len() {
                let seq = &slice[i..i + seq_len];
                if is_valid_utf8_sequence(seq) {
                    i += seq_len;
                    continue;
                }
            }
            non_text += 1;
        } else if b & 0xC0 == 0x80 {
            // Stray continuation byte — likely binary
            non_text += 1;
        } else {
            // 0x00-0x08, 0x0B, 0x0C, 0x0E-0x1F — control chars → binary
            non_text += 1;
        }
        i += 1;
    }

    (non_text as f32 / check_len as f32) > 0.3
}

#[inline]
#[allow(clippy::indexing_slicing)]
fn is_valid_utf8_sequence(seq: &[u8]) -> bool {
    match seq.len() {
        2 => seq[0] >= 0xC2 && (seq[1] & 0xC0) == 0x80,
        3 => {
            let valid = (seq[1] & 0xC0) == 0x80 && (seq[2] & 0xC0) == 0x80;
            if !valid {
                return false;
            }
            if seq[0] == 0xE0 {
                seq[1] >= 0xA0
            } else if seq[0] == 0xED {
                seq[1] <= 0x9F
            } else {
                seq[0] >= 0xE1 && seq[0] <= 0xEC || seq[0] >= 0xEE
            }
        }
        4 => {
            let valid =
                (seq[1] & 0xC0) == 0x80 && (seq[2] & 0xC0) == 0x80 && (seq[3] & 0xC0) == 0x80;
            if !valid {
                return false;
            }
            if seq[0] == 0xF0 {
                seq[1] >= 0x90
            } else if seq[0] == 0xF4 {
                seq[1] <= 0x8F
            } else {
                seq[0] >= 0xF1 && seq[0] <= 0xF3
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_binary_empty() {
        assert!(!is_binary(&[]));
    }

    #[test]
    fn test_is_binary_pure_ascii() {
        assert!(!is_binary(b"Hello, world! This is a normal text file.\n"));
    }

    #[test]
    fn test_is_binary_null_bytes() {
        assert!(is_binary(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03]));
    }

    #[test]
    fn test_is_binary_emoji_heavy() {
        let emoji: &[u8] = &[
            0x23, 0x20, 0xF0, 0x9F, 0x9A, 0xA8, 0xF0, 0x9F, 0x9A, 0xA8, 0xF0, 0x9F, 0x9A, 0xA8,
            0x20, 0x41, 0x4C, 0x45, 0x52, 0x54,
        ];
        assert!(
            !is_binary(emoji),
            "emoji-heavy file should NOT be flagged as binary"
        );
    }

    #[test]
    fn test_is_binary_cjk() {
        let cjk: &[u8] = "\u{4f60}\u{597d}\u{4e16}\u{754c}\u{3053}\u{308c}\u{306f}\u{30c6}\u{30b9}\u{30c8}\u{3067}\u{3059}\u{d55c}\u{ad6d}\u{c5b4}".as_bytes();
        assert!(!is_binary(cjk), "CJK text should NOT be flagged as binary");
    }

    #[test]
    fn test_is_binary_mixed_utf8_ascii() {
        let mut data = Vec::new();
        data.extend_from_slice(b"def hello():\n    ");
        data.extend_from_slice("print('{1f680}')".as_bytes());
        data.extend_from_slice(b"\n    return 42\n");
        assert!(
            !is_binary(&data),
            "Python with emoji should NOT be flagged as binary"
        );
    }

    #[test]
    fn test_is_binary_truly_binary() {
        let mut binary_data = vec![0u8; 512];
        for (i, b) in binary_data.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        assert!(
            is_binary(&binary_data),
            "random byte data should be flagged as binary"
        );
    }

    #[test]
    fn test_is_binary_short_data() {
        assert!(!is_binary(b"hi"), "very short text should not be binary");
        assert!(!is_binary(&[0x0A]), "single newline is not binary");
    }

    #[test]
    fn test_is_binary_utf8_truncated_at_boundary() {
        let emoji: &[u8] = &[0xF0, 0x9F, 0x9A];
        let mut data = Vec::new();
        data.extend_from_slice(b"some text ");
        data.extend_from_slice(emoji);
        data.extend_from_slice(b" more text");
        assert!(
            !is_binary(&data),
            "truncated UTF-8 at boundary should not flip to binary"
        );
    }

    #[test]
    fn test_is_binary_control_chars() {
        let mut data = vec![0x0B; 200];
        data.extend_from_slice(b"normal text padding");
        assert!(
            is_binary(&data),
            "vertical tabs (0x0B) should be flagged as binary"
        );
    }

    #[test]
    fn test_is_binary_mixed_realistic_python() {
        let mut emoji_line = Vec::new();
        emoji_line.extend_from_slice(b"# ");
        for _ in 0..16 {
            emoji_line.extend_from_slice("\u{1f6a8}".as_bytes());
        }
        emoji_line.extend_from_slice(b" WARNING");
        let mut data = Vec::new();
        data.extend_from_slice(&emoji_line);
        data.extend_from_slice(b"\n\ndef process(data):\n    return data.strip()\n");
        assert!(
            !is_binary(&data),
            "realistic Python file with emoji header should NOT be binary"
        );
    }

    #[test]
    fn test_is_binary_exactly_30_percent() {
        let mut data = Vec::new();
        let total = 100;
        #[allow(clippy::cast_precision_loss)]
        let non_text_count = (total as f32 * 0.29) as usize;
        data.extend(std::iter::repeat_n(0x01, non_text_count));
        data.extend(std::iter::repeat_n(b'x', total - non_text_count));
        assert!(!is_binary(&data), "29% non-text should NOT be flagged");
        let mut data_over = Vec::new();
        #[allow(clippy::cast_precision_loss)]
        let non_text_over = (total as f32 * 0.31) as usize;
        data_over.extend(std::iter::repeat_n(0x01, non_text_over));
        data_over.extend(std::iter::repeat_n(b'x', total - non_text_over));
        assert!(is_binary(&data_over), "31% non-text should be flagged");
    }

    #[test]
    fn test_is_valid_utf8_sequence() {
        assert!(is_valid_utf8_sequence(&[0xC3, 0xA9]));
        assert!(is_valid_utf8_sequence(&[0xE4, 0xBD, 0xA0]));
        assert!(
            is_valid_utf8_sequence(&[0xF0, 0x9F, 0x9A, 0xA8]),
            "\u{1f6a8} should be valid 4-byte UTF-8"
        );
        assert!(
            !is_valid_utf8_sequence(&[0xC0, 0x80]),
            "overlong 2-byte encoding (C0)"
        );
        assert!(
            !is_valid_utf8_sequence(&[0xC1, 0x80]),
            "overlong 2-byte encoding (C1)"
        );
        assert!(
            !is_valid_utf8_sequence(&[0xE0, 0x80, 0x80]),
            "overlong 3-byte encoding"
        );
        assert!(
            !is_valid_utf8_sequence(&[0xF0, 0x80, 0x80, 0x80]),
            "overlong 4-byte encoding"
        );
        assert!(
            !is_valid_utf8_sequence(&[0xED, 0xA0, 0x80]),
            "surrogate pair (ED A0)"
        );
        assert!(
            !is_valid_utf8_sequence(&[0xF4, 0x90, 0x80, 0x80]),
            "above U+10FFFF"
        );
        assert!(!is_valid_utf8_sequence(&[0xC2, 0x00]), "bad continuation");
        assert!(!is_valid_utf8_sequence(&[]));
        assert!(!is_valid_utf8_sequence(&[0xFF]));
    }

    #[test]
    fn test_is_binary_stray_continuation_bytes() {
        let data = vec![
            0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x8B, 0x8C, 0x8D,
            0x8E, 0x8F, 0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0x9B,
            0x9C, 0x9D, 0x9E, 0x9F, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9,
            0xAA, 0xAB, 0xAC, 0xAD, 0xAE, 0xAF, 0xB0, 0xB1, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7,
            0xB8, 0xB9, 0xBA, 0xBB, 0xBC, 0xBD, 0xBE, 0xBF, b' ', b' ', b' ', b' ', b' ', b' ',
        ];
        assert!(
            is_binary(&data),
            "stray continuation bytes should be flagged as binary"
        );
    }
}
