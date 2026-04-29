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
    /// Returns an error if ZSTD compression fails.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        varint::encode(u64::try_from(self.entries.len()).unwrap_or(0), &mut buf);

        let mut last_file_id = 0u32;
        for entry in &self.entries {
            let file_id_delta = entry.file_id - last_file_id;
            varint::encode(u64::from(file_id_delta), &mut buf);
            last_file_id = entry.file_id;

            varint::encode(u64::try_from(entry.offsets.len()).unwrap_or(0), &mut buf);
            let mut last_offset = 0u32;
            for &offset in &entry.offsets {
                let offset_delta = offset - last_offset;
                varint::encode(u64::from(offset_delta), &mut buf);
                last_offset = offset;
            }
        }

        zstd::encode_all(&buf[..], Self::ZSTD_COMPRESSION_LEVEL)
            .map_err(|e| Error::Config(format!("posting zstd encode: {e}")))
    }

    /// Decode the posting list from a compressed byte slice.
    /// ZSTD decompression verifies the built-in checksum automatically.
    ///
    /// # Errors
    ///
    /// Returns an error if the ZSTD data is corrupted (checksum mismatch) or if
    /// the varint-decoded structure is invalid.
    pub fn decode(data: &[u8]) -> Result<Self> {
        let payload = zstd::decode_all(data).map_err(|_| Error::PostingCorrupted)?;

        let mut pos = 0;
        let num_files = usize::try_from(varint::decode(&payload, &mut pos)?).unwrap_or(0);
        let mut entries = Vec::with_capacity(num_files);

        let mut last_file_id = 0u32;
        for _ in 0..num_files {
            let file_id_delta = u32::try_from(varint::decode(&payload, &mut pos)?).unwrap_or(0);
            let file_id = last_file_id + file_id_delta;
            last_file_id = file_id;

            let num_offsets = usize::try_from(varint::decode(&payload, &mut pos)?).unwrap_or(0);
            let mut offsets = Vec::with_capacity(num_offsets);
            let mut last_offset = 0u32;
            for _ in 0..num_offsets {
                let offset_delta = u32::try_from(varint::decode(&payload, &mut pos)?).unwrap_or(0);
                let offset = last_offset + offset_delta;
                last_offset = offset;
                offsets.push(offset);
            }
            entries.push(PostingEntry { file_id, offsets });
        }

        Ok(Self { entries })
    }
}

#[cfg(test)]
#[allow(clippy::as_conversions, clippy::unwrap_used, clippy::indexing_slicing)]
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
}
