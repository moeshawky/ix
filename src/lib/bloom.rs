//! Per-file bloom filters.
//!
//! 256 bytes bitset, 5 hashes, FPR ≈ 0.7% for 200 unique trigrams.
//! Eliminates candidate files before decoding posting lists.

use crate::trigram::Trigram;
use std::hash::Hasher;
use std::io::Write;
use xxhash_rust::xxh64::Xxh64;

#[derive(Clone)]
pub struct BloomFilter {
    pub size: u16,
    pub num_hashes: u8,
    pub bits: Vec<u8>,
}

impl BloomFilter {
    pub fn new(size: usize, num_hashes: u8) -> Self {
        Self {
            size: size as u16,
            num_hashes,
            bits: vec![0u8; size],
        }
    }

    pub fn insert(&mut self, trigram: Trigram) {
        let tri_bytes = trigram.to_le_bytes();
        let h1 = self.hash(&tri_bytes, 0);
        let h2 = self.hash(&tri_bytes, 1);
        let num_bits = (self.size as usize) * 8;

        for i in 0..self.num_hashes {
            let bit_pos = (h1.wrapping_add((i as u64).wrapping_mul(h2))) % (num_bits as u64);
            let byte_idx = (bit_pos / 8) as usize;
            let bit_idx = (bit_pos % 8) as u8;
            self.bits[byte_idx] |= 1 << bit_idx;
        }
    }

    pub fn contains(&self, trigram: Trigram) -> bool {
        let tri_bytes = trigram.to_le_bytes();
        let h1 = self.hash(&tri_bytes, 0);
        let h2 = self.hash(&tri_bytes, 1);
        let num_bits = (self.size as usize) * 8;

        for i in 0..self.num_hashes {
            let bit_pos = (h1.wrapping_add((i as u64).wrapping_mul(h2))) % (num_bits as u64);
            let byte_idx = (bit_pos / 8) as usize;
            let bit_idx = (bit_pos % 8) as u8;
            if self.bits[byte_idx] & (1 << bit_idx) == 0 {
                return false;
            }
        }
        true
    }

    fn hash(&self, data: &[u8], seed: u64) -> u64 {
        let mut hasher = Xxh64::new(seed);
        hasher.write(data);
        hasher.finish()
    }

    pub fn serialize<W: Write>(&self, mut w: W) -> std::io::Result<()> {
        w.write_all(&self.size.to_le_bytes())?;
        w.write_all(&[self.num_hashes, 0x00])?; // padding
        w.write_all(&self.bits)?;
        Ok(())
    }

    /// Load from a slice (borrowed).
    pub fn from_slice(data: &[u8]) -> Option<(&[u8], usize)> {
        if data.len() < 4 {
            return None;
        }
        let size = data[0..2].try_into().ok().map(u16::from_le_bytes).unwrap_or(0) as usize;
        let num_hashes = data[2];
        let total_size = 4 + size;
        if data.len() < total_size {
            return None;
        }
        Some((&data[4..total_size], num_hashes as usize))
    }

    /// Check if a slice (from mmap) contains a trigram.
    pub fn slice_contains(bits: &[u8], num_hashes: u8, trigram: Trigram) -> bool {
        let tri_bytes = trigram.to_le_bytes();
        let mut h1_hasher = Xxh64::new(0);
        h1_hasher.write(&tri_bytes);
        let h1 = h1_hasher.finish();

        let mut h2_hasher = Xxh64::new(1);
        h2_hasher.write(&tri_bytes);
        let h2 = h2_hasher.finish();

        let num_bits = bits.len() * 8;

        for i in 0..num_hashes {
            let bit_pos = (h1.wrapping_add((i as u64).wrapping_mul(h2))) % (num_bits as u64);
            let byte_idx = (bit_pos / 8) as usize;
            let bit_idx = (bit_pos % 8) as u8;
            if bits[byte_idx] & (1 << bit_idx) == 0 {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        let mut bloom = BloomFilter::new(256, 5);
        let t1 = 0x010203;
        let t2 = 0x040506;
        bloom.insert(t1);
        assert!(bloom.contains(t1));
        assert!(!bloom.contains(t2));
    }

    #[test]
    fn false_positives() {
        let mut bloom = BloomFilter::new(256, 5);
        for i in 0..200 {
            bloom.insert(i as u32);
        }
        let mut fp = 0;
        for i in 200..1200 {
            if bloom.contains(i as u32) {
                fp += 1;
            }
        }
        // Expect FPR < 1%
        assert!(fp < 20, "FPR too high: {}/1000", fp);
    }
}
