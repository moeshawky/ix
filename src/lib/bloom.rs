//! Per-file bloom filters.
//!
//! 256 bytes bitset, 5 hashes, FPR ≈ 0.7% for 200 unique trigrams.
//! Eliminates candidate files before decoding posting lists.

use crate::trigram::Trigram;
use std::hash::Hasher;
use std::io::Write;
use xxhash_rust::xxh64::Xxh64;

/// Per-file bloom filter for fast trigram membership testing.
///
/// Uses a 256-byte bitset with 5 independent hashes, yielding
/// a false-positive rate of ~0.7% for up to 200 trigrams.
#[derive(Clone)]
pub struct BloomFilter {
    /// Size of the bit array in bytes.
    pub size: u16,
    /// Number of hash functions (k).
    pub num_hashes: u8,
    /// The bit array backing the filter.
    pub bits: Vec<u8>,
}

impl BloomFilter {
    /// Create a new bloom filter with the given size and number of hash functions.
    #[must_use]
    pub fn new(size: usize, num_hashes: u8) -> Self {
        Self {
            size: u16::try_from(size).unwrap_or(0),
            num_hashes,
            bits: vec![0u8; size],
        }
    }

    /// Insert a trigram into the bloom filter.
    pub fn insert(&mut self, trigram: Trigram) {
        let tri_bytes = trigram.to_le_bytes();
        let h1 = Self::hash(&tri_bytes, 0);
        let h2 = Self::hash(&tri_bytes, 1);
        let num_bits = usize::from(self.size) * 8;

        for i in 0..self.num_hashes {
            let bit_pos = (h1.wrapping_add(u64::from(i).wrapping_mul(h2)))
                % u64::try_from(num_bits).unwrap_or(0);
            let byte_idx = usize::try_from(bit_pos / 8).unwrap_or(0);
            let bit_idx = u8::try_from(bit_pos % 8).unwrap_or(0);
            if let Some(byte) = self.bits.get_mut(byte_idx) {
                *byte |= 1 << bit_idx;
            }
        }
    }

    /// Check if a trigram may be present in the filter.
    ///
    /// Returns `true` if the trigram might be present (could be a false positive).
    /// Returns `false` only if the trigram is definitely not present.
    #[must_use]
    pub fn contains(&self, trigram: Trigram) -> bool {
        let tri_bytes = trigram.to_le_bytes();
        let h1 = Self::hash(&tri_bytes, 0);
        let h2 = Self::hash(&tri_bytes, 1);
        let num_bits = usize::from(self.size) * 8;

        for i in 0..self.num_hashes {
            let bit_pos = (h1.wrapping_add(u64::from(i).wrapping_mul(h2)))
                % u64::try_from(num_bits).unwrap_or(0);
            let byte_idx = usize::try_from(bit_pos / 8).unwrap_or(0);
            let bit_idx = u8::try_from(bit_pos % 8).unwrap_or(0);
            if self
                .bits
                .get(byte_idx)
                .is_none_or(|&b| b & (1 << bit_idx) == 0)
            {
                return false;
            }
        }
        true
    }

    fn hash(data: &[u8], seed: u64) -> u64 {
        let mut hasher = Xxh64::new(seed);
        hasher.write(data);
        hasher.finish()
    }

    /// Serialize the bloom filter to a writer.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the writer fails.
    pub fn serialize<W: Write>(&self, mut w: W) -> std::io::Result<()> {
        w.write_all(&self.size.to_le_bytes())?;
        w.write_all(&[self.num_hashes, 0x00])?;
        w.write_all(&self.bits)?;
        Ok(())
    }

    /// Load from a slice (borrowed).
    #[must_use]
    pub fn from_slice(data: &[u8]) -> Option<(&[u8], usize)> {
        if data.len() < 4 {
            return None;
        }
        let size = data
            .get(0..2)?
            .try_into()
            .ok()
            .map_or(0, u16::from_le_bytes);
        let size = usize::from(size);
        let num_hashes = *data.get(2)?;
        let total_size = 4 + size;
        if data.len() < total_size {
            return None;
        }
        data.get(4..total_size)
            .map(|bits| (bits, usize::from(num_hashes)))
    }

    /// Check if a slice (from mmap) contains a trigram.
    #[must_use]
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
            let bit_pos = (h1.wrapping_add(u64::from(i).wrapping_mul(h2)))
                % u64::try_from(num_bits).unwrap_or(0);
            let byte_idx = usize::try_from(bit_pos / 8).unwrap_or(0);
            let bit_idx = u8::try_from(bit_pos % 8).unwrap_or(0);
            if bits.get(byte_idx).is_none_or(|&b| b & (1 << bit_idx) == 0) {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
#[allow(clippy::as_conversions, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        let mut bloom = BloomFilter::new(256, 5);
        let t1 = 0x0001_0203;
        let t2 = 0x0004_0506;
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
        assert!(fp < 20, "FPR too high: {fp}/1000");
    }
}
