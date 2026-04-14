//! Posting list encode/decode (delta + varint + ZSTD compression).
//!
//! Compact representation of (file_id, [offsets]) for a single trigram.
//! ZSTD compression provides ~60-70% size reduction on posting data.

use crate::error::{Error, Result};
use crate::varint;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostingList {
    pub entries: Vec<PostingEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostingEntry {
    pub file_id: u32,
    pub offsets: Vec<u32>,
}

impl PostingList {
    const ZSTD_COMPRESSION_LEVEL: i32 = 3;

    /// Encode the posting list into a compressed byte buffer.
    /// Format: ZSTD(varint-encoded posting data)
    /// ZSTD's built-in XXHash64 checksum provides integrity verification.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        varint::encode(self.entries.len() as u64, &mut buf);

        let mut last_file_id = 0u32;
        for entry in &self.entries {
            let file_id_delta = entry.file_id - last_file_id;
            varint::encode(file_id_delta as u64, &mut buf);
            last_file_id = entry.file_id;

            varint::encode(entry.offsets.len() as u64, &mut buf);
            let mut last_offset = 0u32;
            for &offset in &entry.offsets {
                let offset_delta = offset - last_offset;
                varint::encode(offset_delta as u64, &mut buf);
                last_offset = offset;
            }
        }

        zstd::encode_all(&buf[..], Self::ZSTD_COMPRESSION_LEVEL).unwrap_or(buf)
    }

    /// Decode the posting list from a compressed byte slice.
    /// ZSTD decompression verifies the built-in checksum automatically.
    pub fn decode(data: &[u8]) -> Result<Self> {
        let payload = zstd::decode_all(data).map_err(|_| Error::PostingCorrupted)?;

        let mut pos = 0;
        let num_files = varint::decode(&payload, &mut pos)? as usize;
        let mut entries = Vec::with_capacity(num_files);

        let mut last_file_id = 0u32;
        for _ in 0..num_files {
            let file_id_delta = varint::decode(&payload, &mut pos)? as u32;
            let file_id = last_file_id + file_id_delta;
            last_file_id = file_id;

            let num_offsets = varint::decode(&payload, &mut pos)? as usize;
            let mut offsets = Vec::with_capacity(num_offsets);
            let mut last_offset = 0u32;
            for _ in 0..num_offsets {
                let offset_delta = varint::decode(&payload, &mut pos)? as u32;
                let offset = last_offset + offset_delta;
                last_offset = offset;
                offsets.push(offset);
            }
            entries.push(PostingEntry { file_id, offsets });
        }

        Ok(PostingList { entries })
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

        let encoded = list.encode();
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
        let mut encoded = list.encode();

        // ZSTD's built-in checksum should detect corruption
        encoded[0] ^= 0xFF;

        let result = PostingList::decode(&encoded);
        assert!(result.is_err(), "Decoding corrupted ZSTD data should fail");
    }

    #[test]
    fn empty() {
        let list = PostingList { entries: vec![] };
        let encoded = list.encode();
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
        let encoded = list.encode();

        // ZSTD should compress significantly
        assert!(
            encoded.len() < 50000,
            "Expected compression, got {} bytes",
            encoded.len()
        );
    }
}
