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
    /// Caps extraction to the first `u32::MAX` bytes so every byte offset
    /// fits in `u32` without truncation. Returns a slice of `(trigram, offset)`
    /// pairs, sorted by trigram then offset.
    #[must_use]
    pub fn extract_with_offsets(&mut self, data: &[u8]) -> &[(Trigram, u32)] {
        let max_len = usize::try_from(u32::MAX).unwrap_or(usize::MAX);
        self.extract_with_offsets_capped(data, max_len)
    }

    fn extract_with_offsets_capped(&mut self, data: &[u8], max_len: usize) -> &[(Trigram, u32)] {
        const MAX_OFFSETS_PER_TRIGRAM: usize = 64;
        const SMALL_FILE_LIMIT: usize = 1_000_000;

        self.trigram_offsets.clear();

        if data.len() > max_len {
            tracing::warn!(
                "input length {} exceeds max offset capacity {}; truncating trigram extraction",
                data.len(),
                max_len
            );
        }
        let bounded = data.get(..data.len().min(max_len)).unwrap_or(data);

        if bounded.len() < 3 {
            return &self.trigram_offsets;
        }

        if bounded.len() <= SMALL_FILE_LIMIT {
            for (i, window) in bounded.windows(3).enumerate() {
                if let (Ok(offset), [a, b, c]) = (u32::try_from(i), window) {
                    if *a == 0 || *b == 0 || *c == 0 {
                        continue;
                    }
                    self.trigram_offsets.push((from_bytes(*a, *b, *c), offset));
                }
            }
        } else {
            let mut seen_counts: std::collections::HashMap<Trigram, usize> =
                std::collections::HashMap::new();
            for (i, window) in bounded.windows(3).enumerate() {
                if let (Ok(offset), [a, b, c]) = (u32::try_from(i), window) {
                    if *a == 0 || *b == 0 || *c == 0 {
                        continue;
                    }
                    let tri = from_bytes(*a, *b, *c);
                    let count = seen_counts.entry(tri).or_insert(0);
                    if *count < MAX_OFFSETS_PER_TRIGRAM {
                        self.trigram_offsets.push((tri, offset));
                        *count += 1;
                    }
                }
            }
        }

        self.trigram_offsets.sort_unstable();

        &self.trigram_offsets
    }

    /// Extract unique trigram set from a query (no offsets needed).
    #[must_use]
    pub fn extract_set(query: &[u8]) -> Vec<Trigram> {
        if query.len() < 3 {
            return vec![];
        }
        let mut set = Vec::with_capacity(query.len() - 2);
        for window in query.windows(3) {
            if let [a, b, c] = window {
                set.push(from_bytes(*a, *b, *c));
            }
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
    pub fn extract_groups_case_insensitive(query: &[u8]) -> Vec<Vec<Trigram>> {
        if query.len() < 3 {
            return vec![];
        }
        let mut groups = Vec::with_capacity(query.len() - 2);
        for window in query.windows(3) {
            if let [a, b, c] = window {
                let mut variants = case_variants([*a, *b, *c]);
                variants.sort_unstable();
                variants.dedup();
                groups.push(variants);
            }
        }
        groups
    }
}

/// Generate all case permutations of a 3-byte trigram.
/// Only ASCII alpha bytes get toggled. Max 8 variants per trigram.
fn case_variants(bytes: [u8; 3]) -> Vec<Trigram> {
    let [b0, b1, b2] = bytes;
    let alts0 = byte_cases(b0);
    let alts1 = byte_cases(b1);
    let alts2 = byte_cases(b2);
    let mut out = Vec::with_capacity(alts0.len() * alts1.len() * alts2.len());
    for &a in &alts0 {
        for &b in &alts1 {
            for &c in &alts2 {
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

    #[test]
    fn extract_with_offsets_caps_at_bound() {
        let mut e = Extractor::new();
        let res = e.extract_with_offsets_capped(b"abcdef", 4);
        assert_eq!(
            res,
            &[
                (from_bytes(b'a', b'b', b'c'), 0),
                (from_bytes(b'b', b'c', b'd'), 1),
            ]
        );
        assert!(e.extract_with_offsets_capped(b"abcdef", 2).is_empty());
    }

    #[test]
    fn extract_with_offsets_large_file_caps_per_trigram() {
        let mut e = Extractor::new();
        let data = vec![b'a'; 1_000_005];
        let res = e.extract_with_offsets(&data);
        assert_eq!(res.len(), 64);
        assert_eq!(res[63].1, 63);
    }
}
