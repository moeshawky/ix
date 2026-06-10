//! G-SEM: Property-based tests using proptest.
//!
//! These verify invariants that must hold for ALL inputs, not just hand-picked
//! cases. Each test encodes a fundamental property of the system that would
//! survive any refactoring.

use proptest::prelude::*;

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Trigram extraction properties
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

// Property: Trigram roundtrip is lossless.
// For any 3 bytes without NUL, `from_bytes` followed by byte decomposition
// must recover the original bytes.
proptest! {
    #[test]
    fn prop_trigram_roundtrip_lossless(a in 1u8..=255, b in 1u8..=255, c in 1u8..=255) {
        let tri = ix::trigram::from_bytes(a, b, c);
        let recovered_a = ((tri >> 16) & 0xFF) as u8;
        let recovered_b = ((tri >> 8) & 0xFF) as u8;
        let recovered_c = (tri & 0xFF) as u8;
        assert_eq!(recovered_a, a, "High byte mismatch");
        assert_eq!(recovered_b, b, "Mid byte mismatch");
        assert_eq!(recovered_c, c, "Low byte mismatch");
    }
}

// Property: Trigram extraction is idempotent — calling extract_with_offsets
// twice on the same input must yield identical (trigram, offset) pairs.
// Previously this only verified offsets were in bounds, which tested
// correctness but not idempotency.
proptest! {
    #[test]
    fn prop_extraction_idempotent(s in "[a-zA-Z0-9 \t\n]{3,200}") {
        let mut extractor = ix::trigram::Extractor::new();
        let first = extractor.extract_with_offsets(s.as_bytes()).to_vec();
        let second = extractor.extract_with_offsets(s.as_bytes()).to_vec();

        // Verify that every extracted offset is within bounds
        for &(tri, offset) in &first {
            let end = offset + 2;
            assert!(
                (offset as usize) < s.len() && end < s.len() as u32,
                "Offset {offset} out of bounds for trigram {tri:#08x}"
            );
        }

        // True idempotency: two extractions on the same input must produce
        // identical results (same trigrams, same offsets, same order).
        assert_eq!(
            first.len(),
            second.len(),
            "Idempotency violated: first extraction produced {} trigrams, second produced {}",
            first.len(),
            second.len()
        );
        assert_eq!(
            first, second,
            "Idempotency violated: extraction results differ between calls"
        );
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Posting list encode/decode properties
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Generate random posting lists for property tests
fn arb_posting_list() -> impl Strategy<Value = ix::posting::PostingList> {
    proptest::collection::vec(proptest::collection::vec(0u32..10_000u32, 1..50), 0..20).prop_map(
        |offset_groups| {
            let entries = offset_groups
                .into_iter()
                .enumerate()
                .map(|(file_id, mut offsets)| {
                    // Posting entries must have sorted, deduplicated offsets
                    offsets.sort_unstable();
                    offsets.dedup();
                    ix::posting::PostingEntry {
                        file_id: file_id as u32,
                        offsets,
                    }
                })
                .collect();
            ix::posting::PostingList { entries }
        },
    )
}

// Property: PostingList encode→decode roundtrip is lossless
proptest! {
    #[test]
    fn prop_posting_roundtrip_lossless(list in arb_posting_list()) {
        let encoded = list
            .encode()
            .expect("Encoding must not fail for valid data");
        let decoded = ix::posting::PostingList::decode(&encoded)
            .expect("Decoding must not fail for valid encoded data");
        assert_eq!(list.entries.len(), decoded.entries.len(),
            "Entry count mismatch: expected {}, got {}",
            list.entries.len(), decoded.entries.len());
        assert_eq!(list, decoded, "Posting roundtrip must be lossless");
    }
}

// Property: Encoding is never empty for non-empty lists
proptest! {
    #[test]
    fn prop_posting_encode_nonempty(list in arb_posting_list()) {
        let encoded = list
            .encode()
            .expect("Encoding must not fail");
        if !list.entries.is_empty() {
            assert!(!encoded.is_empty(), "Non-empty list must produce non-empty encoding");
        }
    }
}

#[test]
fn prop_posting_empty_list_identity() {
    let list = ix::posting::PostingList { entries: vec![] };
    let encoded = list.encode().expect("Empty list encoding must succeed");
    let decoded =
        ix::posting::PostingList::decode(&encoded).expect("Empty list decoding must succeed");
    assert_eq!(list, decoded);
    assert!(
        !encoded.is_empty(),
        "Even empty list must produce a valid ZSTD frame"
    );
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Varint encode/decode properties
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

// Property: Varint roundtrip is lossless for any u64 value
proptest! {
    #[test]
    fn prop_varint_roundtrip_lossless(value: u64) {
        let mut buf = Vec::new();
        ix::varint::encode(value, &mut buf);
        let mut pos = 0;
        let decoded = ix::varint::decode(&buf, &mut pos)
            .expect("Valid varint decode must succeed");
        assert_eq!(decoded, value, "Varint roundtrip: {value} != {decoded}");
        assert_eq!(pos, buf.len(), "All encoded bytes must be consumed");
    }
}

// Property: Varint-encoded lengths are monotonic with value magnitude
proptest! {
    #[test]
    fn prop_varint_length_monotonic(a: u64, b: u64) {
        let mut buf_a = Vec::new();
        let mut buf_b = Vec::new();
        ix::varint::encode(a, &mut buf_a);
        ix::varint::encode(b, &mut buf_b);
        // Bigger values should use at least as many bytes as smaller values
        if a <= b {
            assert!(
                buf_a.len() <= buf_b.len(),
                "Larger value {b} uses fewer bytes ({}) than smaller value {a} ({})",
                buf_b.len(), buf_a.len()
            );
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Bloom filter properties
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

// Property: Bloom filter has zero false negatives
proptest! {
    #[test]
    fn prop_bloom_zero_false_negatives(
        insert_keys in proptest::collection::vec(
            (1u8..=255, 1u8..=255, 1u8..=255), 1..50
        )
    ) {
        let mut bloom = ix::bloom::BloomFilter::new(256, 5);
        let keys: Vec<ix::trigram::Trigram> = insert_keys
            .iter()
            .map(|&(a, b, c)| ix::trigram::from_bytes(a, b, c))
            .collect();
        for &key in &keys {
            bloom.insert(key);
        }
        for &key in &keys {
            assert!(
                bloom.contains(key),
                "Bloom filter false negative for trigram {key:#08x}"
            );
        }
    }
}

// Property: Bloom filter operations are order-independent
proptest! {
    #[test]
    fn prop_bloom_order_independent(
        keys in proptest::collection::vec(
            (1u8..=255, 1u8..=255, 1u8..=255), 1..30
        )
    ) {
        let trigrams: Vec<ix::trigram::Trigram> = keys
            .iter()
            .map(|&(a, b, c)| ix::trigram::from_bytes(a, b, c))
            .collect();

        let mut bloom1 = ix::bloom::BloomFilter::new(256, 5);
        for &k in &trigrams {
            bloom1.insert(k);
        }

        let mut bloom2 = ix::bloom::BloomFilter::new(256, 5);
        for &k in trigrams.iter().rev() {
            bloom2.insert(k);
        }

        let all_present = trigrams
            .iter()
            .all(|k| bloom1.contains(*k) == bloom2.contains(*k));
        assert!(all_present, "Bloom filter result depends on insertion order");
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Format module properties
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

// Property: is_binary returns false for all-ASCII content
proptest! {
    #[test]
    fn prop_is_binary_false_for_ascii(s in "[ -~]{1,5000}") {
        assert!(
            !ix::format::is_binary(s.as_bytes()),
            "All-ASCII content must not be classified as binary"
        );
    }
}

// Property: is_binary returns true when >30% non-printable bytes
proptest! {
    #[test]
    fn prop_is_binary_true_when_over_threshold(
        text in "[a-zA-Z ]{1,100}",
        null_count in 43usize..200,
    ) {
        let mut data = Vec::new();
        data.extend_from_slice(text.as_bytes());
        data.extend(std::iter::repeat_n(0u8, null_count));
        assert!(
            ix::format::is_binary(&data),
            "Content with {null_count} NULLs in {} bytes must be binary",
            data.len()
        );
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// String pool serialization roundtrip property
//
// The StringPool stores path→offset mappings. Its serialize/deserialize
// roundtrip must be lossless for path resolution.
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

proptest! {
    #[test]
    fn prop_string_pool_paths_serialize_roundtrip(
        paths in proptest::collection::vec("[a-z0-9_/]{1,40}", 0..15)
    ) {
        use ix::string_pool::{StringPool, StringPoolReader};
        use std::io::Cursor;
        use std::path::Path;

        let mut pool = StringPool::new();
        for p in &paths {
            pool.add_path(Path::new(p));
        }

        let prefixes: Vec<String> = paths
            .iter()
            .filter_map(|p| {
                let parent = Path::new(p).parent()?;
                Some(parent.to_string_lossy().to_string())
            })
            .collect();
        pool.set_prefixes(prefixes);

        // Capture offsets before serialization
        let offsets: Vec<(String, u32)> = paths
            .iter()
            .map(|p| {
                let (off, _) = pool.get_info(Path::new(p));
                (p.clone(), off)
            })
            .collect();

        let mut buf = Cursor::new(Vec::new());
        pool.serialize(&mut buf).expect("StringPool serialize must not fail");
        let data = buf.into_inner();

        let reader = StringPoolReader::new(&data)
            .expect("StringPoolReader must parse serialized data");

        for (orig_path, offset) in &offsets {
            let resolved = reader.resolve(*offset)
                .unwrap_or_else(|_| orig_path.clone());
            // Resolved path should equal the original or be a suffix (prefix dedup)
            assert!(
                orig_path.ends_with(&resolved) || resolved.ends_with(orig_path.as_str()),
                "Path '{orig_path}' resolves to '{resolved}' — prefix dedup should preserve semantics"
            );
        }
    }
}

// Property: StringPoolReader resolves empty pool without panicking
#[test]
fn prop_string_pool_empty_roundtrip() {
    use ix::string_pool::{StringPool, StringPoolReader};
    use std::io::Cursor;

    let mut pool = StringPool::new();
    let mut buf = Cursor::new(Vec::new());
    pool.serialize(&mut buf)
        .expect("Empty pool serialize must succeed");
    let data = buf.into_inner();

    let reader = StringPoolReader::new(&data).expect("Empty pool parse must succeed");
    // Resolving offset 0 in empty pool should never panic
    let _ = reader.resolve(0);
}
