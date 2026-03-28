//! Posting list encode/decode (delta + varint).
//!
//! Compact representation of (file_id, [offsets]) for a single trigram.

use crate::error::Result;
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
    /// Encode the posting list into a byte buffer.
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
        buf
    }

    /// Decode the posting list from a byte slice.
    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut pos = 0;
        let num_files = varint::decode(data, &mut pos)? as usize;
        let mut entries = Vec::with_capacity(num_files);

        let mut last_file_id = 0u32;
        for _ in 0..num_files {
            let file_id_delta = varint::decode(data, &mut pos)? as u32;
            let file_id = last_file_id + file_id_delta;
            last_file_id = file_id;

            let num_offsets = varint::decode(data, &mut pos)? as usize;
            let mut offsets = Vec::with_capacity(num_offsets);
            let mut last_offset = 0u32;
            for _ in 0..num_offsets {
                let offset_delta = varint::decode(data, &mut pos)? as u32;
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
    fn empty() {
        let list = PostingList { entries: vec![] };
        let encoded = list.encode();
        let decoded = PostingList::decode(&encoded).unwrap();
        assert_eq!(list, decoded);
    }
}
