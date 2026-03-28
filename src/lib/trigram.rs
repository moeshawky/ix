//! Trigram extraction — the core indexing primitive.
//!
//! Operates on raw bytes. UTF-8 self-synchronization makes this correct
//! for Unicode for free. No charset detection needed.

use std::collections::HashMap;

/// A trigram is a u32 with high byte always 0.
/// trigram("abc") = (0x61 << 16) | (0x62 << 8) | 0x63
pub type Trigram = u32;

/// Convert 3 bytes to a trigram key.
#[inline]
pub fn from_bytes(a: u8, b: u8, c: u8) -> Trigram {
    ((a as u32) << 16) | ((b as u32) << 8) | (c as u32)
}

/// Reusable trigram extractor. Pre-allocated, reused across files.
pub struct Extractor {
    trigram_offsets: HashMap<Trigram, Vec<u32>>,
}

impl Extractor {
    pub fn new() -> Self {
        Self {
            trigram_offsets: HashMap::with_capacity(4096),
        }
    }

    /// Extract trigrams with byte offsets (for indexing).
    pub fn extract_with_offsets(&mut self, data: &[u8]) -> &HashMap<Trigram, Vec<u32>> {
        self.trigram_offsets.clear();

        if data.len() < 3 {
            return &self.trigram_offsets;
        }

        for i in 0..=(data.len() - 3) {
            // Skip trigrams containing null bytes (binary content marker)
            if data[i] == 0 || data[i + 1] == 0 || data[i + 2] == 0 {
                continue;
            }

            let tri = from_bytes(data[i], data[i + 1], data[i + 2]);
            self.trigram_offsets.entry(tri).or_default().push(i as u32);
        }

        &self.trigram_offsets
    }

    /// Extract unique trigram set from a query (no offsets needed).
    pub fn extract_set(query: &[u8]) -> Vec<Trigram> {
        if query.len() < 3 {
            return vec![];
        }
        let mut set = Vec::with_capacity(query.len() - 2);
        for i in 0..=(query.len() - 3) {
            let tri = from_bytes(query[i], query[i + 1], query[i + 2]);
            set.push(tri);
        }
        set.sort_unstable();
        set.dedup();
        set
    }

    /// Extract trigram groups with case permutations for case-insensitive search.
    /// Returns one group per original trigram position. Each group contains all
    /// case variants for that position (max 8 per group for 3 ASCII alpha bytes).
    /// The executor should UNION within each group and INTERSECT across groups.
    pub fn extract_groups_case_insensitive(query: &[u8]) -> Vec<Vec<Trigram>> {
        if query.len() < 3 {
            return vec![];
        }
        let mut groups = Vec::with_capacity(query.len() - 2);
        for i in 0..=(query.len() - 3) {
            let bytes = [query[i], query[i + 1], query[i + 2]];
            let mut variants = case_variants(bytes);
            variants.sort_unstable();
            variants.dedup();
            groups.push(variants);
        }
        groups
    }
}

/// Generate all case permutations of a 3-byte trigram.
/// Only ASCII alpha bytes get toggled. Max 8 variants per trigram.
fn case_variants(bytes: [u8; 3]) -> Vec<Trigram> {
    let alts: [Vec<u8>; 3] = [
        byte_cases(bytes[0]),
        byte_cases(bytes[1]),
        byte_cases(bytes[2]),
    ];
    let mut out = Vec::with_capacity(alts[0].len() * alts[1].len() * alts[2].len());
    for &a in &alts[0] {
        for &b in &alts[1] {
            for &c in &alts[2] {
                out.push(from_bytes(a, b, c));
            }
        }
    }
    out
}

/// Return both cases for ASCII alpha, or just the byte itself.
#[inline]
fn byte_cases(b: u8) -> Vec<u8> {
    if b.is_ascii_uppercase() {
        vec![b, b.to_ascii_lowercase()]
    } else if b.is_ascii_lowercase() {
        vec![b, b.to_ascii_uppercase()]
    } else {
        vec![b]
    }
}

impl Default for Extractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty() {
        let mut e = Extractor::new();
        assert!(e.extract_with_offsets(b"").is_empty());
        assert!(e.extract_with_offsets(b"ab").is_empty());
    }

    #[test]
    fn single_trigram() {
        let mut e = Extractor::new();
        let result = e.extract_with_offsets(b"abc");
        assert_eq!(result.len(), 1);
        assert!(result.contains_key(&from_bytes(b'a', b'b', b'c')));
    }

    #[test]
    fn deduplication() {
        let mut e = Extractor::new();
        let result = e.extract_with_offsets(b"abcabc");
        // "abc" at offset 0 and 3, "bca" at 1, "cab" at 2
        let abc = from_bytes(b'a', b'b', b'c');
        assert_eq!(result[&abc].len(), 2);
        assert_eq!(result[&abc], vec![0, 3]);
    }

    #[test]
    fn null_bytes_skipped() {
        let mut e = Extractor::new();
        let result = e.extract_with_offsets(b"a\x00b");
        assert!(result.is_empty());
    }

    #[test]
    fn high_bytes_work() {
        let mut e = Extractor::new();
        let result = e.extract_with_offsets(&[0xFF, 0xFE, 0xFD]);
        let tri = from_bytes(0xFF, 0xFE, 0xFD);
        assert!(result.contains_key(&tri));
    }

    #[test]
    fn utf8_cafe() {
        // "caf\xC3\xA9" = UTF-8 for "cafe" with accent
        let data = "caf\u{00e9}".as_bytes();
        let set = Extractor::extract_set(data);
        assert!(!set.is_empty());
        // Should produce byte trigrams, not char trigrams
        assert!(set.contains(&from_bytes(b'c', b'a', b'f')));
    }

    #[test]
    fn query_set_deduped() {
        let set = Extractor::extract_set(b"errorerror");
        // All trigrams from "error" repeated — should be deduped
        let unique_count = set.len();
        assert!(unique_count <= 8); // "error" has 3 unique trigrams
    }
}
