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
    /// CDX blocks).
    pub const POSTING_LISTS_COMPRESSED: u64 = 1 << 2;
    /// Each posting-list chunk carries an `XXHash64` checksum.
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

        let r = |off: usize| -> crate::error::Result<u64> {
            data.get(off..off + 8)
                .and_then(|s| s.try_into().ok())
                .map(u64::from_le_bytes)
                .ok_or_else(|| crate::error::Error::HeaderCorrupted {
                    expected: u32::try_from(HEADER_SIZE).unwrap_or(256),
                    actual: u32::try_from(data.len()).unwrap_or(0),
                })
        };
        let r16 = |off: usize| -> crate::error::Result<u16> {
            data.get(off..off + 2)
                .and_then(|s| s.try_into().ok())
                .map(u16::from_le_bytes)
                .ok_or_else(|| crate::error::Error::HeaderCorrupted {
                    expected: u32::try_from(HEADER_SIZE).unwrap_or(256),
                    actual: u32::try_from(data.len()).unwrap_or(0),
                })
        };
        let r32 = |off: usize| -> crate::error::Result<u32> {
            data.get(off..off + 4)
                .and_then(|s| s.try_into().ok())
                .map(u32::from_le_bytes)
                .ok_or_else(|| crate::error::Error::HeaderCorrupted {
                    expected: u32::try_from(HEADER_SIZE).unwrap_or(256),
                    actual: u32::try_from(data.len()).unwrap_or(0),
                })
        };

        let major = r16(0x04)?;
        let minor = r16(0x06)?;
        if major != VERSION_MAJOR || minor < VERSION_MINOR {
            return Err(crate::error::Error::UnsupportedVersion { major, minor });
        }

        // Validate CRC32C of header (bytes 0x00..0xF8)
        let expected_crc = r32(0xF8)?;
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
            flags: r(0x08)?,
            created_at: r(0x10)?,
            source_bytes_total: r(0x18)?,
            file_count: r32(0x20)?,
            trigram_count: r32(0x24)?,
            file_table_offset: r(0x28)?,
            file_table_size: r(0x30)?,
            trigram_table_offset: r(0x38)?,
            trigram_table_size: r(0x40)?,
            posting_data_offset: r(0x48)?,
            posting_data_size: r(0x50)?,
            bloom_offset: r(0x58)?,
            bloom_size: r(0x60)?,
            string_pool_offset: r(0x68)?,
            string_pool_size: r(0x70)?,
            name_index_offset: r(0x78)?,
            name_index_size: r(0x80)?,
            cdx_block_index_offset: r(0x88)?,
            cdx_block_index_size: r(0x90)?,
        })
    }

    /// Validate all section offsets fit within the file.
    ///
    /// # Errors
    ///
    /// Returns an error if any section extends beyond the file length, starts
    /// inside the fixed header, or has insufficient `file_table` capacity for
    /// `file_count` entries.
    pub fn validate_bounds(&self, file_len: u64) -> crate::error::Result<()> {
        let min_off = u64::try_from(HEADER_SIZE).unwrap_or(256);
        let check = |name: &'static str, off: u64, sz: u64| -> crate::error::Result<()> {
            // Use checked arithmetic so a corrupt header with overflowing
            // offset+size cannot wrap past this guard (defense-in-depth;
            // downstream reads also use .get()).
            let end = off
                .checked_add(sz)
                .ok_or(crate::error::Error::SectionOutOfBounds {
                    section: name,
                    offset: off,
                    size: sz,
                    file_len,
                })?;
            if off < min_off || end > file_len {
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

        let file_entry_size = u64::try_from(FILE_ENTRY_SIZE).unwrap_or(48);
        let required_file_table = u64::from(self.file_count).checked_mul(file_entry_size);
        if required_file_table.is_none_or(|req| req > self.file_table_size) {
            return Err(crate::error::Error::SectionOutOfBounds {
                section: "file_table",
                offset: self.file_table_offset,
                size: self.file_table_size,
                file_len,
            });
        }

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
        } else if self.has_bloom() && self.bloom_offset > file_len {
            return Err(crate::error::Error::SectionOutOfBounds {
                section: "bloom",
                offset: self.bloom_offset,
                size: self.bloom_size,
                file_len,
            });
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
    /// Verifies the recorded PID is positive and still exists, belongs to an
    /// `ixd` binary, and the watched root directory is still accessible.
    #[must_use]
    pub fn is_live(&self) -> bool {
        #[cfg(unix)]
        {
            use nix::sys::signal::kill;
            use nix::unistd::Pid;

            if self.pid <= 0 {
                return false;
            }

            if kill(Pid::from_raw(self.pid), None).is_err() {
                return false;
            }

            #[cfg(target_os = "linux")]
            {
                let comm_path = format!("/proc/{}/comm", self.pid);
                if let Ok(comm) = std::fs::read_to_string(&comm_path) {
                    let comm = comm.trim();
                    if comm != "ixd" {
                        return false;
                    }
                } else {
                    return false;
                }
            }

            self.root.exists()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    /// Atomically write the beacon to `beacon.json` in the given folder.
    ///
    /// Writes to a temporary file inside `folder`, flushes and syncs it, then
    /// renames it onto `beacon.json` so concurrent readers never see a
    /// truncated file.
    ///
    /// # Errors
    ///
    /// Returns an error if the temporary file cannot be created, serialization
    /// fails, or the atomic rename fails.
    pub fn write_to(&self, folder: &Path) -> crate::error::Result<()> {
        use std::io::Write;
        use std::sync::atomic::{AtomicU64, Ordering};

        struct TempFileGuard {
            path: PathBuf,
            armed: bool,
        }

        impl TempFileGuard {
            const fn new(path: PathBuf) -> Self {
                Self { path, armed: true }
            }
            const fn disarm(&mut self) {
                self.armed = false;
            }
        }

        impl Drop for TempFileGuard {
            fn drop(&mut self) {
                if self.armed {
                    let _ = std::fs::remove_file(&self.path);
                }
            }
        }

        static SEQ: AtomicU64 = AtomicU64::new(0);

        let path = folder.join("beacon.json");
        let pid = std::process::id();
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp_path = folder.join(format!("beacon.json.tmp.{pid}.{seq}"));
        let mut tmp_guard = TempFileGuard::new(tmp_path.clone());

        let mut f = std::fs::File::create(&tmp_path)?;
        serde_json::to_writer_pretty(&mut f, self).map_err(std::io::Error::other)?;
        f.flush()?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp_path, &path)?;
        tmp_guard.disarm();
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
        let beacon = serde_json::from_reader(f)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
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
pub fn is_binary(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    let check_len = data.len().min(512);
    let slice = data.get(..check_len).unwrap_or(&[]);

    let mut non_text = 0usize;
    let mut i = 0;
    while i < slice.len() {
        let Some(&b) = slice.get(i) else {
            break;
        };
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

            if seq_len > 0
                && let Some(end) = i.checked_add(seq_len)
                && let Some(seq) = slice.get(i..end)
                && is_valid_utf8_sequence(seq)
            {
                i = end;
                continue;
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

    let non_text_u32 = u32::try_from(non_text).unwrap_or(u32::MAX);
    let check_len_u32 = u32::try_from(check_len).unwrap_or(512);
    (f64::from(non_text_u32) / f64::from(check_len_u32)) > 0.3
}

#[inline]
const fn is_valid_utf8_sequence(seq: &[u8]) -> bool {
    match seq {
        [a, b] => *a >= 0xC2 && (*b & 0xC0) == 0x80,
        [a, b, c] => {
            let valid = (*b & 0xC0) == 0x80 && (*c & 0xC0) == 0x80;
            if !valid {
                return false;
            }
            if *a == 0xE0 {
                *b >= 0xA0
            } else if *a == 0xED {
                *b <= 0x9F
            } else {
                *a >= 0xE1 && *a <= 0xEF
            }
        }
        [a, b, c, d] => {
            let valid = (*b & 0xC0) == 0x80 && (*c & 0xC0) == 0x80 && (*d & 0xC0) == 0x80;
            if !valid {
                return false;
            }
            if *a == 0xF0 {
                *b >= 0x90
            } else if *a == 0xF4 {
                *b <= 0x8F
            } else {
                *a >= 0xF1 && *a <= 0xF3
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
        data.extend_from_slice("print('\u{1f680}')".as_bytes());
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
            *b = u8::try_from(i % 256).unwrap_or(0);
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
        let total = 100usize;
        let non_text_count = 29usize;
        data.extend(std::iter::repeat_n(0x01, non_text_count));
        data.extend(std::iter::repeat_n(b'x', total - non_text_count));
        assert!(!is_binary(&data), "29% non-text should NOT be flagged");
        let mut data_over = Vec::new();
        let non_text_over = 31usize;
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

    // ── Rule 1: Error Path Tests ──────────────────────────────────────

    /// Pass <256 bytes to `Header::parse` → must return Err(IndexTooSmall).
    #[test]
    fn test_header_truncated_input_error() {
        let data = vec![0u8; 100];
        let result = Header::parse(&data);
        assert!(result.is_err(), "truncated input should fail");
        match result {
            Err(crate::error::Error::IndexTooSmall) => {}
            other => panic!("expected IndexTooSmall, got {other:?}"),
        }
    }

    /// Pass exactly 256 bytes with wrong magic → must return Err(BadMagic).
    #[test]
    fn test_header_bad_magic_error() {
        let mut data = vec![0u8; 256];
        // Write "XXXX" instead of "IX01" as magic
        data[0..4].copy_from_slice(b"XXXX");
        let result = Header::parse(&data);
        assert!(result.is_err(), "bad magic should fail");
        match result {
            Err(crate::error::Error::BadMagic) => {}
            other => panic!("expected BadMagic, got {other:?}"),
        }
    }

    /// Header with correct magic and version but corrupted CRC32C
    /// must return Err(HeaderCorrupted).
    #[test]
    fn test_header_crc_mismatch_error() {
        let mut data = [0u8; 256];
        data[0..4].copy_from_slice(&MAGIC); // "IX01"
        // Write version 1.3 (identical to actual version constants)
        data[4..6].copy_from_slice(&1u16.to_le_bytes());
        data[6..8].copy_from_slice(&3u16.to_le_bytes());
        // flags = 0 (no features)
        data[8..16].copy_from_slice(&0u64.to_le_bytes());
        // Leave CRC field as zeros → CRC32C will mismatch
        let result = Header::parse(&data);
        assert!(result.is_err(), "bad CRC should fail");
        match result {
            Err(crate::error::Error::HeaderCorrupted { .. }) => {}
            other => panic!("expected HeaderCorrupted, got {other:?}"),
        }
    }

    // ── Rule 2: Corruption Proptests ──────────────────────────────────

    /// Random 256-byte buffers must never parse as valid headers.
    #[test]
    fn prop_random_header_always_err() {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        for _ in 0..100 {
            let data: Vec<u8> = (0..256).map(|_| rng.r#gen()).collect();
            let result = Header::parse(&data);
            assert!(
                result.is_err(),
                "random 256-byte buffer should never parse as valid header"
            );
        }
    }

    fn valid_empty_header() -> Header {
        let header_off = u64::try_from(HEADER_SIZE).unwrap_or(256);
        Header {
            version_major: VERSION_MAJOR,
            version_minor: VERSION_MINOR,
            flags: flags::HAS_BLOOM_FILTERS,
            created_at: 0,
            source_bytes_total: 0,
            file_count: 0,
            trigram_count: 0,
            file_table_offset: header_off,
            file_table_size: 0,
            trigram_table_offset: header_off,
            trigram_table_size: 0,
            posting_data_offset: header_off,
            posting_data_size: 0,
            bloom_offset: header_off,
            bloom_size: 0,
            string_pool_offset: header_off,
            string_pool_size: 10,
            name_index_offset: header_off + 10,
            name_index_size: 0,
            cdx_block_index_offset: 0,
            cdx_block_index_size: 0,
        }
    }

    #[test]
    fn test_beacon_is_live_rejects_non_positive_pid() {
        let dir = tempfile::tempdir().expect("tempdir");
        for pid in [0, -1, -42] {
            let mut beacon = Beacon::new(dir.path());
            beacon.pid = pid;
            assert!(!beacon.is_live(), "pid={pid} must never be considered live");
        }
    }

    #[test]
    fn test_beacon_write_to_atomic_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut beacon = Beacon::new(dir.path());
        beacon.status = "indexing".to_string();
        beacon.write_to(dir.path()).expect("write_to first");

        beacon.status = "idle".to_string();
        beacon.write_to(dir.path()).expect("write_to overwrite");

        let loaded = Beacon::read_from(dir.path()).expect("read_from");
        assert_eq!(loaded.status, "idle");
        assert_eq!(loaded.pid, beacon.pid);
        assert_eq!(loaded.instance_id, beacon.instance_id);

        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read_dir")
            .filter_map(std::result::Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["beacon.json".to_string()]);
    }

    #[test]
    fn test_header_validate_bounds_checks_capacity_and_offsets() {
        let mut hdr = valid_empty_header();
        assert!(hdr.validate_bounds(512).is_ok());

        // file_count * 48 > file_table_size must be rejected
        hdr.file_count = 10;
        hdr.file_table_size = 48;
        assert!(matches!(
            hdr.validate_bounds(512),
            Err(crate::error::Error::SectionOutOfBounds {
                section: "file_table",
                ..
            })
        ));

        // Overflowing file_count * 48 is safe and rejected
        hdr.file_count = u32::MAX;
        hdr.file_table_size = 48;
        assert!(hdr.validate_bounds(u64::MAX).is_err());

        // Mandatory section offset inside fixed header (< HEADER_SIZE) rejected
        let mut hdr_small_off = valid_empty_header();
        hdr_small_off.posting_data_offset = 64;
        assert!(matches!(
            hdr_small_off.validate_bounds(512),
            Err(crate::error::Error::SectionOutOfBounds {
                section: "posting_data",
                ..
            })
        ));

        // HAS_BLOOM_FILTERS with bloom_size == 0 still validates bloom_offset <= file_len
        let mut hdr_bad_bloom = valid_empty_header();
        hdr_bad_bloom.bloom_offset = u64::MAX;
        hdr_bad_bloom.bloom_size = 0;
        assert!(matches!(
            hdr_bad_bloom.validate_bounds(512),
            Err(crate::error::Error::SectionOutOfBounds {
                section: "bloom",
                ..
            })
        ));
    }
}
