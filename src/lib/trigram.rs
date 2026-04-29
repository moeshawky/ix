//! Trigram extraction — the core indexing primitive.
//!
//! Operates on raw bytes. UTF-8 self-synchronization makes this correct
//! for Unicode for free. No charset detection needed.

/// A trigram is a u32 with high byte always 0.
/// trigram("abc") = (0x61 << 16) | (0x62 << 8) | 0x63
pub type Trigram = u32;

/// Convert 3 bytes to a trigram key.
#[inline]
#[must_use]
pub fn from_bytes(a: u8, b: u8, c: u8) -> Trigram {
    (u32::from(a) << 16) | (u32::from(b) << 8) | u32::from(c)
}

/// Reusable trigram extractor. Pre-allocated, reused across files.
pub struct Extractor {
    trigram_offsets: Vec<(Trigram, u32)>,
}

impl Extractor {
    /// Create a new extractor with pre-allocated capacity.
    #[must_use]
    pub fn new() -> Self {
        Self {
            trigram_offsets: Vec::with_capacity(4096),
        }
    }

    /// Extract trigrams with byte offsets (for indexing).
    ///
    /// Returns a slice of `(trigram, offset)` pairs, sorted by trigram then offset.
    #[must_use]
    #[allow(clippy::indexing_slicing)] // loop bound: data.len()-3 guarantees i+2 in range
    #[allow(clippy::as_conversions)] // i < data.len() < u32::MAX for index files
    pub fn extract_with_offsets(&mut self, data: &[u8]) -> &[(Trigram, u32)] {
        const MAX_OFFSETS_PER_TRIGRAM: usize = 64;
        const SMALL_FILE_LIMIT: usize = 1_000_000;

        self.trigram_offsets.clear();

        if data.len() < 3 {
            return &self.trigram_offsets;
        }

        if data.len() <= SMALL_FILE_LIMIT {
            for i in 0..=(data.len() - 3) {
                if data[i] == 0 || data[i + 1] == 0 || data[i + 2] == 0 {
                    continue;
                }
                let tri = from_bytes(data[i], data[i + 1], data[i + 2]);
                self.trigram_offsets.push((tri, i as u32));
            }
        } else {
            let mut seen_counts: std::collections::HashMap<Trigram, usize> =
                std::collections::HashMap::new();
            for i in 0..=(data.len() - 3) {
                if data[i] == 0 || data[i + 1] == 0 || data[i + 2] == 0 {
                    continue;
                }
                let tri = from_bytes(data[i], data[i + 1], data[i + 2]);
                let count = seen_counts.entry(tri).or_insert(0);
                if *count < MAX_OFFSETS_PER_TRIGRAM {
                    self.trigram_offsets.push((tri, i as u32));
                    *count += 1;
                }
            }
        }

        self.trigram_offsets.sort_unstable();

        &self.trigram_offsets
    }

    /// Extract unique trigram set from a query (no offsets needed).
    #[must_use]
    #[allow(clippy::indexing_slicing)] // loop bound: query.len()-3 guarantees i+2 in range
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
    ///
    /// Returns one group per original trigram position. Each group contains all
    /// case variants for that position (max 8 per group for 3 ASCII alpha bytes).
    /// The executor should UNION within each group and INTERSECT across groups.
    #[must_use]
    #[allow(clippy::indexing_slicing)] // loop bound: query.len()-3 guarantees i+2 in range
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
#[allow(clippy::as_conversions, clippy::unwrap_used, clippy::indexing_slicing)]
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
        assert_eq!(result[0], (from_bytes(b'a', b'b', b'c'), 0));
    }

    #[test]
    fn deduplication() {
        let mut e = Extractor::new();
        let result = e.extract_with_offsets(b"abcabc");
        let abc = from_bytes(b'a', b'b', b'c');
        let bca = from_bytes(b'b', b'c', b'a');
        let cab = from_bytes(b'c', b'a', b'b');

        assert_eq!(result.len(), 4);
        assert_eq!(result[0], (abc, 0));
        assert_eq!(result[1], (abc, 3));
        assert_eq!(result[2], (bca, 1));
        assert_eq!(result[3], (cab, 2));
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
        assert_eq!(result[0], (tri, 0));
    }

    #[test]
    fn utf8_cafe() {
        let data = "caf\u{00e9}".as_bytes();
        let set = Extractor::extract_set(data);
        assert!(!set.is_empty());
        assert!(set.contains(&from_bytes(b'c', b'a', b'f')));
    }

    #[test]
    fn query_set_deduped() {
        let set = Extractor::extract_set(b"errorerror");
        let unique_count = set.len();
        assert!(unique_count <= 8);
    }
}
