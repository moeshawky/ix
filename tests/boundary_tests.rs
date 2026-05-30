//! C-attack boundary tests — verify compound bug prevention across index format
//! boundaries. Each test crosses at least one CBP Phase 0 boundary.
//!
//! C1: Delta format roundtrip (B4 Persistence — Builder → DeltaFile → Executor)
//! C3: CDX block index semantic preservation (B2 Memory — trigram key → block → key)
//! C4: Idempotent build ordering (B6 Time — build sequence independence)

use ix::builder::Builder;
use ix::executor::{Executor, QueryOptions};
use ix::planner::Planner;
use ix::reader::Reader;
use std::fs;
use tempfile::tempdir;

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// C1: Contract Mismatch — Delta format roundtrip
//
// Builder writes .ix/.delta (Entry → DeltaFile), Executor reads it through
// DeltaReader. The format contract is: DeltaReader::open() must parse every
// entry Builder::build() writes, preserving file mappings, tombstones, and
// posting entries exactly.
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[test]
fn test_c1_delta_roundtrip_after_rebuild() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    fs::write(root.join("a.txt"), "needle in haystack").unwrap();
    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let mut executor = Executor::new(&reader);

    let plan = Planner::plan("needle", false);
    let (matches, _) = executor.execute(&plan, &QueryOptions::default()).unwrap();
    assert_eq!(
        matches.len(),
        1,
        "C1: Initial build should find needle in a.txt"
    );

    // Rebuild: Builder writes delta entries for the changed file
    fs::write(root.join("a.txt"), "needle transformed").unwrap();
    fs::write(root.join("b.txt"), "another needle here").unwrap();
    let mut builder2 = Builder::new(root).unwrap();
    builder2.build().unwrap();

    let reader2 = Reader::open(&index_path).unwrap();
    let mut executor2 = Executor::new(&reader2);
    let (matches2, _) = executor2.execute(&plan, &QueryOptions::default()).unwrap();

    assert_eq!(
        matches2.len(),
        2,
        "C1: Rebuild should find needle in both files"
    );
}

#[test]
fn test_c1_delta_tombstone_removes_file() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    fs::write(root.join("explicit.rs"), "fn remove_me() {}").unwrap();
    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");

    // Verify initial match
    let reader = Reader::open(&index_path).unwrap();
    let mut executor = Executor::new(&reader);
    let plan = Planner::plan("remove_me", false);
    let (matches, _) = executor.execute(&plan, &QueryOptions::default()).unwrap();
    assert_eq!(matches.len(), 1, "C1: File must exist before tombstone");

    // Delete file and rebuild — causes tombstone in delta
    fs::remove_file(root.join("explicit.rs")).unwrap();
    let mut builder2 = Builder::new(root).unwrap();
    builder2.build().unwrap();

    let reader2 = Reader::open(&index_path).unwrap();
    let mut executor2 = Executor::new(&reader2);
    let (matches2, _) = executor2.execute(&plan, &QueryOptions::default()).unwrap();
    assert_eq!(
        matches2.len(),
        0,
        "C1: Tombstone should remove deleted file from search results"
    );
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// C3: Reclassification — CDX block index trigram identity
//
// CDX stores trigram keys delta-encoded within ZSTD-compressed blocks.
// The block index maps (first_key, block_offset). The Reader decompresses
// a block and finds the exact trigram by key. The key must NOT be
// reclassified across the encode→decode boundary.
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[test]
fn test_c3_cdx_trigram_identity_at_block_boundaries() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create files with trigrams that span the CDX block boundaries.
    // CDX blocks hold 1024 entries each. A sparse set of trigrams that
    // crosses block boundaries exercises the two-level lookup path.
    let mut content = String::new();
    // Generate trigrams at predictable offsets using byte patterns
    for i in 0u8..=255u8 {
        content.push_str(&format!("{i:03} "));
        content.push_str("abc"); // trigram "abc" at known offsets
    }
    fs::write(root.join("cdx_test.txt"), &content).unwrap();

    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let mut executor = Executor::new(&reader);

    let plan = Planner::plan("abc", false);
    let (matches, stats) = executor.execute(&plan, &QueryOptions::default()).unwrap();

    assert!(
        !matches.is_empty(),
        "C3: Trigrams must survive CDX roundtrip"
    );
    assert!(
        stats.trigrams_queried > 0,
        "C3: CDX lookup must be exercised"
    );
    assert_eq!(
        stats.files_failed_verify, 0,
        "C3: No files should fail verification during CDX lookup"
    );
}

#[test]
fn test_c3_cdx_block_index_binary_search_correct() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create many distinct files to generate a dense trigram table that
    // exercises the block index binary search (partition_point).
    let keywords: Vec<String> = (0..64).map(|i| format!("key_{i:04}")).collect::<Vec<_>>();

    for kw in &keywords {
        fs::write(
            root.join(format!("{kw}.txt")),
            format!("line with {kw} inside"),
        )
        .unwrap();
    }

    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let mut executor = Executor::new(&reader);

    for kw in ["key_0000", "key_0063"] {
        let plan = Planner::plan(kw, false);
        let (matches, _) = executor.execute(&plan, &QueryOptions::default()).unwrap();
        assert_eq!(
            matches.len(),
            1,
            "C3: Binary search must locate edge key '{kw}' exactly"
        );
    }

    // Search for a non-existent key near the end
    let plan = Planner::plan("zzz_not_found", false);
    let (matches, _) = executor.execute(&plan, &QueryOptions::default()).unwrap();
    assert!(
        matches.is_empty(),
        "C3: Non-existent key must return empty results"
    );
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// C4: Ordering Dependency — Build sequence independence
//
// The index must produce the same search results regardless of whether
// files are built all at once or incrementally added. This is a C4
// temporal ordering check: the set of (file → trigram) mappings must
// be order-independent.
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[test]
fn test_c4_build_sequence_independent_results() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    let file_a = "alpha appears in file_a.txt";
    let file_b = "beta appears in file_b.txt";

    // Build all at once
    fs::write(root.join("a.txt"), file_a).unwrap();
    fs::write(root.join("b.txt"), file_b).unwrap();
    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let mut executor = Executor::new(&reader);

    let plan_a = Planner::plan("alpha", false);
    let (matches_a, _) = executor.execute(&plan_a, &QueryOptions::default()).unwrap();
    let names_a: std::collections::BTreeSet<&str> = matches_a
        .iter()
        .map(|m| m.file_path.file_name().unwrap().to_str().unwrap())
        .collect();

    let plan_b = Planner::plan("beta", false);
    let (matches_b, _) = executor.execute(&plan_b, &QueryOptions::default()).unwrap();
    let names_b: std::collections::BTreeSet<&str> = matches_b
        .iter()
        .map(|m| m.file_path.file_name().unwrap().to_str().unwrap())
        .collect();

    assert_eq!(names_a, ["a.txt"].iter().copied().collect());
    assert_eq!(names_b, ["b.txt"].iter().copied().collect());

    // Now build incrementally: add c.txt after initial build
    fs::write(root.join("c.txt"), "alpha and beta in new file").unwrap();
    let mut builder2 = Builder::new(root).unwrap();
    builder2.build().unwrap();

    let reader2 = Reader::open(&index_path).unwrap();
    let mut executor2 = Executor::new(&reader2);

    let plan_alpha = Planner::plan("alpha", false);
    let (matches, _) = executor2
        .execute(&plan_alpha, &QueryOptions::default())
        .unwrap();
    assert_eq!(
        matches.len(),
        2,
        "C4: Incremental build should find alpha in a.txt and c.txt"
    );
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// C1: Contract Mismatch — Empty index integrity
//
// The empty index (no files scanned) must still be a valid format that
// Reader::open() accepts and Executor can query without panicking.
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[test]
fn test_c1_empty_index_contract() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    assert!(index_path.exists(), "C1: Empty index file must exist");

    let reader = Reader::open(&index_path).unwrap();
    let mut executor = Executor::new(&reader);

    let plan = Planner::plan("anything", false);
    let (matches, _) = executor.execute(&plan, &QueryOptions::default()).unwrap();
    assert!(
        matches.is_empty(),
        "C1: Empty index must return zero matches"
    );

    // Rebuild empty index — must be idempotent
    let mut builder2 = Builder::new(root).unwrap();
    builder2.build().unwrap();
    let reader2 = Reader::open(&index_path).unwrap();
    let mut executor2 = Executor::new(&reader2);
    let (matches2, _) = executor2.execute(&plan, &QueryOptions::default()).unwrap();
    assert!(
        matches2.is_empty(),
        "C1: Empty index rebuild must be idempotent"
    );
}
