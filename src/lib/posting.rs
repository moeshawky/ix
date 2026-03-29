//! Posting list encode/decode (delta + varint).
//!
//! Compact representation of (file_id, [offsets]) for a single trigram.

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
    /// Encode the posting list into a byte buffer with a CRC32C footer.
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

        // Add CRC32C checksum
        let crc = crc32c::crc32c(&buf);
        buf.extend_from_slice(&crc.to_le_bytes());
        buf
    }

    /// Decode the posting list from a byte slice, verifying the CRC32C footer.
    pub fn decode(data: &[u8]) -> Result<Self> {
        if data.len() < 4 {
            return Err(Error::PostingCorrupted);
        }

        let (payload, crc_bytes) = data.split_at(data.len() - 4);
        let expected_crc = u32::from_le_bytes(crc_bytes.try_into().unwrap());
        let actual_crc = crc32c::crc32c(payload);

        if expected_crc != actual_crc {
            return Err(Error::PostingCorrupted);
        }

        let mut pos = 0;
        let num_files = varint::decode(payload, &mut pos)? as usize;
        let mut entries = Vec::with_capacity(num_files);

        let mut last_file_id = 0u32;
        for _ in 0..num_files {
            let file_id_delta = varint::decode(payload, &mut pos)? as u32;
            let file_id = last_file_id + file_id_delta;
            last_file_id = file_id;

            let num_offsets = varint::decode(payload, &mut pos)? as usize;
            let mut offsets = Vec::with_capacity(num_offsets);
            let mut last_offset = 0u32;
            for _ in 0..num_offsets {
                let offset_delta = varint::decode(payload, &mut pos)? as u32;
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
        
        // Flip a bit in the payload (not the CRC)
        encoded[0] ^= 0xFF;
        
        let result = PostingList::decode(&encoded);
        assert!(result.is_err(), "Decoding corrupted payload should fail");
        
        // Restore payload, flip a bit in CRC
        encoded[0] ^= 0xFF;
        let last_idx = encoded.len() - 1;
        encoded[last_idx] ^= 0xFF;
        
        let result = PostingList::decode(&encoded);
        assert!(result.is_err(), "Decoding with corrupted CRC should fail");
    }

    #[test]
    fn empty() {
        let list = PostingList { entries: vec![] };
        let encoded = list.encode();
        let decoded = PostingList::decode(&encoded).unwrap();
        assert_eq!(list, decoded);
    }
}
