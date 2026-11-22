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
    ///
    /// Sizes larger than `u16::MAX` are clamped to `u16::MAX` so `size` always
    /// equals `bits.len()`. Use [`Self::try_new`] to reject oversized filters
    /// with an error instead of clamping.
    #[must_use]
    pub fn new(size: usize, num_hashes: u8) -> Self {
        let clamped = size.min(usize::from(u16::MAX));
        let size_u16 = u16::try_from(clamped).unwrap_or(u16::MAX);
        Self {
            size: size_u16,
            num_hashes,
            bits: vec![0u8; clamped],
        }
    }

    /// Create a new bloom filter, returning an error if `size` exceeds `u16::MAX`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::Error::Config`] when `size > u16::MAX as usize`.
    pub fn try_new(size: usize, num_hashes: u8) -> crate::error::Result<Self> {
        let size_u16 = u16::try_from(size).map_err(|_| {
            crate::error::Error::Config(format!(
                "bloom filter size {size} exceeds maximum {}",
                u16::MAX
            ))
        })?;
        Ok(Self {
            size: size_u16,
            num_hashes,
            bits: vec![0u8; usize::from(size_u16)],
        })
    }

    /// Insert a trigram into the bloom filter.
    ///
    /// If the filter has zero size or a truncated `bits` buffer, this is a no-op.
    pub fn insert(&mut self, trigram: Trigram) {
        // Guard: size=0 produces num_bits=0 which causes a division-by-zero
        // panic in the modulo operation below. An empty or truncated filter stores nothing.
        if self.size == 0 || self.bits.len() < usize::from(self.size) {
            return;
        }
        let tri_bytes = trigram.to_le_bytes();
        let h1 = Self::hash(&tri_bytes, 0);
        let h2 = Self::hash(&tri_bytes, 1);
        let num_bits = usize::from(self.size) * 8;

        for i in 0..self.num_hashes {
            let bit_pos = (h1.wrapping_add(u64::from(i).wrapping_mul(h2)))
                % u64::try_from(num_bits)
                    .unwrap_or_else(|_| unreachable!("num_bits > 0 guaranteed by size check"));
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
    ///
    /// If the filter has zero size or `bits.len() < self.size as usize`, returns
    /// `true` conservatively because membership cannot be disproved.
    #[must_use]
    pub fn contains(&self, trigram: Trigram) -> bool {
        // Guard: size=0 produces num_bits=0 which causes a division-by-zero
        // panic in the modulo operation below; truncated bits cannot disprove membership.
        if self.size == 0 || self.bits.len() < usize::from(self.size) {
            return true;
        }
        let tri_bytes = trigram.to_le_bytes();
        let h1 = Self::hash(&tri_bytes, 0);
        let h2 = Self::hash(&tri_bytes, 1);
        let num_bits = usize::from(self.size) * 8;

        for i in 0..self.num_hashes {
            let bit_pos = (h1.wrapping_add(u64::from(i).wrapping_mul(h2)))
                % u64::try_from(num_bits)
                    .unwrap_or_else(|_| unreachable!("num_bits > 0 guaranteed by size check"));
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
    /// Returns an `InvalidInput` error if `self.bits.len() != self.size as usize`,
    /// or an I/O error if the writer fails.
    pub fn serialize<W: Write>(&self, mut w: W) -> std::io::Result<()> {
        if self.bits.len() != usize::from(self.size) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "bloom filter length mismatch: size={} != bits.len()={}",
                    self.size,
                    self.bits.len()
                ),
            ));
        }
        w.write_all(&self.size.to_le_bytes())?;
        w.write_all(&[self.num_hashes, 0x00])?;
        w.write_all(&self.bits)?;
        Ok(())
    }

    /// Check if a slice (from mmap) contains a trigram.
    ///
    /// Returns `true` if the trigram may be present (conservative).
    /// Returns `false` only if the trigram is definitely not present.
    ///
    /// If `bits` is an empty slice (zero-size bloom filter), returns `true`
    /// (conservative: an empty filter cannot disprove membership). This
    /// prevents a division-by-zero panic in the bit-position modulo operation.
    #[must_use]
    pub fn slice_contains(bits: &[u8], num_hashes: u8, trigram: Trigram) -> bool {
        // Guard: empty bits → num_bits=0 → modulo-by-zero panic.
        // Conservative: return true (assume trigram may be present).
        if bits.is_empty() {
            return true;
        }
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
                % u64::try_from(num_bits)
                    .unwrap_or_else(|_| unreachable!("num_bits > 0 guaranteed by size check"));
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
        for i in 0..200u32 {
            bloom.insert(i);
        }
        let mut fp = 0;
        for i in 200u32..1200u32 {
            if bloom.contains(i) {
                fp += 1;
            }
        }
        assert!(fp < 20, "FPR too high: {fp}/1000");
    }

    #[test]
    fn test_bloom_new_clamps_oversized_size() {
        let mut bloom = BloomFilter::new(65_536, 5);
        assert_eq!(bloom.size, u16::MAX);
        assert_eq!(bloom.bits.len(), usize::from(u16::MAX));
        let t = 0x0001_0203;
        bloom.insert(t);
        assert!(bloom.contains(t));
        let mut out = Vec::new();
        bloom.serialize(&mut out).expect("serialize clamped bloom");
        assert_eq!(out.len(), 4 + usize::from(u16::MAX));
    }

    #[test]
    fn test_bloom_try_new_rejects_oversized() {
        assert!(BloomFilter::try_new(65_536, 5).is_err());
        let ok = BloomFilter::try_new(256, 5).expect("256 fits in u16");
        assert_eq!(ok.size, 256);
        assert_eq!(ok.bits.len(), 256);
    }

    #[test]
    fn test_bloom_mismatched_bits_conservative_and_serialize_err() {
        let bloom = BloomFilter {
            size: 256,
            num_hashes: 5,
            bits: vec![0u8; 8],
        };
        assert!(bloom.contains(0x0001_0203));
        let mut out = Vec::new();
        assert!(bloom.serialize(&mut out).is_err());
    }
}
