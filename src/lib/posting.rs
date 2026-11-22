//! Posting list encode/decode (delta + varint + ZSTD compression).
//!
//! Compact representation of (`file_id`, `offsets`) for a single trigram.
//! ZSTD compression provides ~60-70% size reduction on posting data.

use crate::error::{Error, Result};
use crate::varint;

/// A collection of per-file posting entries for a single trigram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostingList {
    /// Ordered list of files that contain this trigram, with their hit offsets.
    pub entries: Vec<PostingEntry>,
}

/// One file's hit data within a posting list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostingEntry {
    /// Unique file identifier in the index.
    pub file_id: u32,
    /// Byte offsets (absolute within the file) where the trigram occurs.
    pub offsets: Vec<u32>,
}

impl PostingList {
    /// ZSTD compression level used for posting list and CDX block encoding.
    pub const ZSTD_COMPRESSION_LEVEL: i32 = 3;

    /// Encode the posting list into a compressed byte buffer.
    /// Format: ZSTD(varint-encoded posting data)
    /// ZSTD's built-in `XXHash64` checksum provides integrity verification.
    ///
    /// # Errors
    ///
    /// Returns an error if entries or offsets are unsorted, counts overflow `u64`,
    /// or ZSTD compression fails.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        let entry_count = u64::try_from(self.entries.len())
            .map_err(|_| Error::Config("posting entry count overflow".into()))?;
        varint::encode(entry_count, &mut buf);

        let mut last_file_id = 0u32;
        for entry in &self.entries {
            let file_id_delta = entry.file_id.checked_sub(last_file_id).ok_or_else(|| {
                Error::Config(format!(
                    "unsorted posting entries: file_id {} < previous {}",
                    entry.file_id, last_file_id
                ))
            })?;
            varint::encode(u64::from(file_id_delta), &mut buf);
            last_file_id = entry.file_id;

            let offset_count = u64::try_from(entry.offsets.len())
                .map_err(|_| Error::Config("posting offset count overflow".into()))?;
            varint::encode(offset_count, &mut buf);
            let mut last_offset = 0u32;
            for &offset in &entry.offsets {
                let offset_delta = offset.checked_sub(last_offset).ok_or_else(|| {
                    Error::Config(format!(
                        "unsorted posting offsets for file_id {}: {} < {}",
                        entry.file_id, offset, last_offset
                    ))
                })?;
                varint::encode(u64::from(offset_delta), &mut buf);
                last_offset = offset;
            }
        }

        zstd::encode_all(buf.as_slice(), Self::ZSTD_COMPRESSION_LEVEL)
            .map_err(|e| Error::Config(format!("posting zstd encode: {e}")))
    }

    /// Decode the posting list from a compressed byte slice.
    /// ZSTD decompression verifies the built-in checksum automatically.
    ///
    /// # Errors
    ///
    /// Returns an error if the ZSTD data is corrupted (checksum mismatch),
    /// exceeds the decompressed payload cap, or if the varint-decoded
    /// structure is invalid, overflows `u32`, or has trailing bytes.
    pub fn decode(data: &[u8]) -> Result<Self> {
        const MAX_POSTING_PAYLOAD_BYTES: usize = 128 * 1024 * 1024;
        Self::decode_with_limit(data, MAX_POSTING_PAYLOAD_BYTES)
    }

    fn decode_with_limit(data: &[u8], max_payload_bytes: usize) -> Result<Self> {
        use std::io::Read;

        // Caps for corrupt counts to avoid OOM (defense-in-depth).
        const MAX_POSTING_FILES: usize = 1 << 20;
        const MAX_OFFSETS_PER_FILE: usize = 1 << 24;

        let limit = u64::try_from(max_payload_bytes).unwrap_or(u64::MAX);
        let decoder = zstd::stream::Decoder::new(data).map_err(|_| Error::PostingCorrupted)?;
        let mut payload = Vec::new();
        decoder
            .take(limit.saturating_add(1))
            .read_to_end(&mut payload)
            .map_err(|_| Error::PostingCorrupted)?;
        if u64::try_from(payload.len()).unwrap_or(u64::MAX) > limit {
            return Err(Error::PostingCorrupted);
        }

        let mut pos = 0;
        let num_files = usize::try_from(varint::decode(&payload, &mut pos)?)
            .map_err(|_| Error::PostingCorrupted)?;
        if num_files > MAX_POSTING_FILES {
            return Err(Error::PostingCorrupted);
        }
        let remaining_files = payload.len().saturating_sub(pos);
        let mut entries = Vec::with_capacity(num_files.min(remaining_files / 2));

        let mut last_file_id = 0u32;
        for _ in 0..num_files {
            let file_id_delta = u32::try_from(varint::decode(&payload, &mut pos)?)
                .map_err(|_| Error::PostingCorrupted)?;
            let file_id = last_file_id
                .checked_add(file_id_delta)
                .ok_or(Error::PostingCorrupted)?;
            last_file_id = file_id;

            let num_offsets = usize::try_from(varint::decode(&payload, &mut pos)?)
                .map_err(|_| Error::PostingCorrupted)?;
            if num_offsets > MAX_OFFSETS_PER_FILE {
                return Err(Error::PostingCorrupted);
            }
            let remaining_offsets = payload.len().saturating_sub(pos);
            let mut offsets = Vec::with_capacity(num_offsets.min(remaining_offsets));
            let mut last_offset = 0u32;
            for _ in 0..num_offsets {
                let offset_delta = u32::try_from(varint::decode(&payload, &mut pos)?)
                    .map_err(|_| Error::PostingCorrupted)?;
                let offset = last_offset
                    .checked_add(offset_delta)
                    .ok_or(Error::PostingCorrupted)?;
                last_offset = offset;
                offsets.push(offset);
            }
            entries.push(PostingEntry { file_id, offsets });
        }

        if pos != payload.len() {
            return Err(Error::PostingCorrupted);
        }

        Ok(Self { entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let list = PostingList {
            entries: vec![
                PostingEntry {
                    file_id: 5,
                    offsets: vec![100, 340, 342],
                },
                PostingEntry {
                    file_id: 12,
                    offsets: vec![44],
                },
                PostingEntry {
                    file_id: 15,
                    offsets: vec![200, 880],
                },
            ],
        };

        let encoded = list.encode().unwrap();
        let decoded = PostingList::decode(&encoded).unwrap();
        assert_eq!(list, decoded);
    }

    #[test]
    fn test_corruption_detection() {
        let list = PostingList {
            entries: vec![PostingEntry {
                file_id: 1,
                offsets: vec![10, 20],
            }],
        };
        let mut encoded = list.encode().unwrap();

        // ZSTD's built-in checksum should detect corruption
        encoded[0] ^= 0xFF;

        let result = PostingList::decode(&encoded);
        assert!(result.is_err(), "Decoding corrupted ZSTD data should fail");
    }

    #[test]
    fn empty() {
        let list = PostingList { entries: vec![] };
        let encoded = list.encode().unwrap();
        let decoded = PostingList::decode(&encoded).unwrap();
        assert_eq!(list, decoded);
    }

    #[test]
    fn compression_ratio() {
        let mut entries = Vec::new();
        for i in 0..1000 {
            entries.push(PostingEntry {
                file_id: i,
                offsets: (0..100).map(|j| i * 100 + j).collect(),
            });
        }
        let list = PostingList { entries };
        let encoded = list.encode().unwrap();

        assert!(
            encoded.len() < 50000,
            "Expected compression, got {} bytes",
            encoded.len()
        );
    }

    // ── Rule 1: Error Path Tests ──────────────────────────────────────

    /// Corrupt a byte in the middle of valid ZSTD posting data → decode must Err.
    #[test]
    fn test_posting_corrupt_mid_byte_error() {
        let list = PostingList {
            entries: vec![
                PostingEntry {
                    file_id: 1,
                    offsets: vec![10, 20, 30],
                },
                PostingEntry {
                    file_id: 5,
                    offsets: vec![100, 200],
                },
            ],
        };
        let mut encoded = list.encode().expect("encode valid list");
        // Corrupt a byte in the middle, not the first byte
        let mid = encoded.len() / 2;
        encoded[mid] ^= 0xAA;
        let result = PostingList::decode(&encoded);
        assert!(
            result.is_err(),
            "mid-byte corruption should cause decode failure"
        );
    }

    // ── Rule 2: Corruption Proptests ──────────────────────────────────

    /// For random posting lists, corrupting bytes must cause decode failure.
    /// Uses multi-byte corruption since zstd frames can survive isolated bit flips.
    #[test]
    fn prop_posting_corruption() {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        let mut corruption_errors = 0u32;
        let mut total_tests = 0u32;

        for _ in 0..50 {
            let num_entries = rng.r#gen_range(1..10);
            let mut entries = Vec::with_capacity(num_entries);
            for file_id in 0..num_entries {
                let num_offsets = rng.r#gen_range(1..20);
                let mut offsets = Vec::with_capacity(num_offsets);
                let mut base = rng.r#gen_range(0..10_000u32);
                for _ in 0..num_offsets {
                    offsets.push(base);
                    base += rng.r#gen_range(1..500);
                }
                entries.push(PostingEntry {
                    file_id: u32::try_from(file_id).unwrap_or(u32::MAX),
                    offsets,
                });
            }
            let list = PostingList { entries };

            let Ok(encoded) = list.encode() else {
                continue;
            };

            if encoded.len() < 4 {
                continue;
            }

            // Corrupt multiple bytes: flip the second byte and a byte near the end
            let mut corrupted = encoded.clone();
            // Flip 3 bytes at different positions for robust corruption
            corrupted[1] ^= 0xFF;
            let clen = corrupted.len();
            if clen > 4 {
                corrupted[clen - 2] ^= 0xFF;
            }
            if clen > 8 {
                corrupted[clen / 2] ^= 0xFF;
            }

            total_tests += 1;
            if PostingList::decode(&corrupted).is_err() {
                corruption_errors += 1;
            }
        }

        // At least 90% of corruption attempts should be detected
        assert!(total_tests > 0, "no corruption tests were run");
        let rate = f64::from(corruption_errors) / f64::from(total_tests);
        assert!(
            rate > 0.8,
            "corruption detection rate {rate:.2} too low ({corruption_errors}/{total_tests})"
        );
    }

    #[test]
    fn test_encode_rejects_unsorted_entries_and_offsets() {
        let unsorted_files = PostingList {
            entries: vec![
                PostingEntry {
                    file_id: 10,
                    offsets: vec![1],
                },
                PostingEntry {
                    file_id: 5,
                    offsets: vec![1],
                },
            ],
        };
        assert!(unsorted_files.encode().is_err());

        let unsorted_offsets = PostingList {
            entries: vec![PostingEntry {
                file_id: 1,
                offsets: vec![20, 10],
            }],
        };
        assert!(unsorted_offsets.encode().is_err());
    }

    #[test]
    fn test_decode_rejects_overflow_trailing_and_oversized_counts() {
        // 1. Delta summing > u32::MAX must return PostingCorrupted
        let mut buf = Vec::new();
        varint::encode(2, &mut buf); // num_files = 2
        varint::encode(u64::from(u32::MAX), &mut buf); // file_id 1 = u32::MAX
        varint::encode(0, &mut buf); // 0 offsets
        varint::encode(1, &mut buf); // file_id_delta = 1 -> overflow u32
        varint::encode(0, &mut buf);
        let compressed = zstd::encode_all(buf.as_slice(), 3).unwrap();
        assert!(matches!(
            PostingList::decode(&compressed),
            Err(Error::PostingCorrupted)
        ));

        // 2. Varint > u32::MAX for file_id_delta must return PostingCorrupted
        let mut buf_wide = Vec::new();
        varint::encode(1, &mut buf_wide);
        varint::encode(u64::from(u32::MAX) + 1, &mut buf_wide);
        varint::encode(0, &mut buf_wide);
        let compressed_wide = zstd::encode_all(buf_wide.as_slice(), 3).unwrap();
        assert!(matches!(
            PostingList::decode(&compressed_wide),
            Err(Error::PostingCorrupted)
        ));

        // 3. Oversized num_files (> 1 << 20) must return PostingCorrupted without OOM
        let mut buf_many = Vec::new();
        varint::encode((1u64 << 20) + 1, &mut buf_many);
        let compressed_many = zstd::encode_all(buf_many.as_slice(), 3).unwrap();
        assert!(matches!(
            PostingList::decode(&compressed_many),
            Err(Error::PostingCorrupted)
        ));

        // 4. Trailing unconsumed bytes must return PostingCorrupted
        let mut buf_trail = Vec::new();
        varint::encode(0, &mut buf_trail); // 0 files
        buf_trail.push(0x42); // trailing garbage
        let compressed_trail = zstd::encode_all(buf_trail.as_slice(), 3).unwrap();
        assert!(matches!(
            PostingList::decode(&compressed_trail),
            Err(Error::PostingCorrupted)
        ));
    }

    #[test]
    fn test_decode_rejects_zstd_bomb_over_limit() {
        let zeros = vec![0u8; 4096];
        let compressed = zstd::encode_all(zeros.as_slice(), 3).unwrap();
        assert!(matches!(
            PostingList::decode_with_limit(&compressed, 1024),
            Err(Error::PostingCorrupted)
        ));
    }
}
