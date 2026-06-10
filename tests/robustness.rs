use ix::builder::Builder;
use ix::executor::{Executor, QueryOptions};
use ix::format;
use ix::planner::Planner;
use ix::reader::Reader;
use std::fs;
use std::io::Read;
use tempfile::tempdir;

#[test]
fn test_edge_case_file_sizes() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Empty file
    fs::write(root.join("empty.txt"), "").unwrap();
    // 1-byte file
    fs::write(root.join("one.txt"), "a").unwrap();
    // 2-byte file
    fs::write(root.join("two.txt"), "ab").unwrap();
    // 3-byte file (exactly one trigram)
    fs::write(root.join("three.txt"), "abc").unwrap();
    // 4-byte file (two overlapping trigrams)
    fs::write(root.join("four.txt"), "abcd").unwrap();

    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let mut executor = Executor::new(&reader);

    // Search for "abc"
    let plan = Planner::plan("abc", false).unwrap();
    let (matches, _) = executor.execute(&plan, &QueryOptions::default()).unwrap();

    // Should find in three.txt and four.txt
    assert_eq!(matches.len(), 2);
    let names: Vec<_> = matches
        .iter()
        .map(|m| m.file_path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert!(names.contains(&"three.txt"));
    assert!(names.contains(&"four.txt"));
}

#[test]
fn test_repetitive_data_explosion() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // 1000 lines, each with a match
    let content = "abc\n".repeat(1000);
    fs::write(root.join("repetitive.txt"), content).unwrap();

    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let mut executor = Executor::new(&reader);

    let plan = Planner::plan("abc", false).unwrap();
    let (matches, stats) = executor
        .execute(
            &plan,
            &QueryOptions {
                max_results: 100,
                ..Default::default()
            },
        )
        .unwrap();

    // Test explicit cap
    assert_eq!(matches.len(), 100);
    assert_eq!(stats.total_matches, 100);

    // Library default is unlimited (0)
    let (matches_default, _) = executor.execute(&plan, &QueryOptions::default()).unwrap();
    assert_eq!(matches_default.len(), 1000);
}

#[test]
fn test_context_merging_logic() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Matches on lines 1, 2, 3. With context 1, they should all merge into one block in the output logic,
    // though the executor returns individual matches with their own context.
    fs::write(
        root.join("context.txt"),
        "match\nmatch\nmatch\nother\nother\nmatch",
    )
    .unwrap();

    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let mut executor = Executor::new(&reader);

    let plan = Planner::plan("match", false).unwrap();
    let (matches, _) = executor
        .execute(
            &plan,
            &QueryOptions {
                context_lines: 1,
                ..Default::default()
            },
        )
        .unwrap();

    assert_eq!(matches.len(), 4);

    // First match (line 1)
    assert_eq!(matches[0].context_before.len(), 0);
    assert_eq!(matches[0].context_after, vec!["match".to_string()]);

    // Second match (line 2)
    assert_eq!(matches[1].context_before, vec!["match".to_string()]);
    assert_eq!(matches[1].context_after, vec!["match".to_string()]);
}

#[test]
fn test_type_filtering_robustness() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    fs::write(root.join("file.rs"), "findme").unwrap();
    fs::write(root.join("file.py"), "findme").unwrap();
    fs::write(root.join("file.txt"), "findme").unwrap();

    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let mut executor = Executor::new(&reader);

    let plan = Planner::plan("findme", false).unwrap();

    // Only Rust
    let (matches, _) = executor
        .execute(
            &plan,
            &QueryOptions {
                type_filter: vec!["rs".to_string()],
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert!(matches[0].file_path.to_str().unwrap().ends_with(".rs"));

    // Rust or Python
    let (matches, _) = executor
        .execute(
            &plan,
            &QueryOptions {
                type_filter: vec!["rs".to_string(), "py".to_string()],
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(matches.len(), 2);
}

#[test]
fn test_large_file_streaming() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create a 10MB file - large enough to prove streaming works
    let line = "This is a normal line with a pattern inside it.\n";
    let count = (10 * 1024 * 1024) / line.len();
    let content = line.repeat(count);
    let file_path = root.join("large.txt");
    fs::write(&file_path, content).unwrap();

    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let mut executor = Executor::new(&reader);

    let plan = Planner::plan("pattern", false).unwrap();
    let options = QueryOptions {
        max_results: 10,
        ..Default::default()
    };

    // This should succeed quickly and with constant memory
    let (matches, stats) = executor.execute(&plan, &options).unwrap();

    assert_eq!(matches.len(), 10);
    assert!(stats.files_verified >= 1);
}

#[test]
#[cfg(all(feature = "notify", unix))]
fn test_search_path_prefix_filtering() {
    use ix::daemon_sock::{SearchQuery, execute_search};

    let dir = tempdir().unwrap();
    let root = dir.path();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();

    fs::write(root.join("src/main.rs"), "fn main() { \n findme \n}").unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn lib() { \n findme \n}").unwrap();
    fs::write(root.join("tests/test.rs"), "#[test]\n findme \n").unwrap();
    fs::write(root.join("docs/guide.md"), "# Guide\n findme").unwrap();

    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    assert!(root.join(".ix/shard.ix").exists());

    let query = SearchQuery {
        id: 1,
        pattern: "findme".into(),
        is_regex: false,
        ignore_case: false,
        word_boundary: false,
        max_results: 0,
        context_lines: 0,
        file_types: vec![],
        decompress: false,
        multiline: false,
        archive: false,
        binary: false,
        search_path: Some(root.join("src")),
        progressive: false,
        chunk_size_bytes: 0,
        chunk_overlap_bytes: 0,
    };

    let results = execute_search(root, &query).expect("execute_search with path filter");
    assert_eq!(results.matches.len(), 2);
    assert_eq!(results.stats.total_matches, 2);
    for m in &results.matches {
        assert!(
            m.file_path.to_string_lossy().contains("/src/"),
            "expected path containing /src/, got {:?}",
            m.file_path
        );
    }

    // Test without path filter — should return all 4 matches
    let full_query = SearchQuery {
        search_path: None,
        ..query.clone()
    };
    let full_results = execute_search(root, &full_query).expect("full search");
    assert_eq!(full_results.matches.len(), 4);
    assert_eq!(full_results.stats.total_matches, 4);

    // Test with non-matching path — should return empty
    let empty_query = SearchQuery {
        search_path: Some(root.join("nonexistent").to_path_buf()),
        ..query.clone()
    };
    let empty_results = execute_search(root, &empty_query).expect("empty filter search");
    assert_eq!(empty_results.matches.len(), 0);
    assert_eq!(empty_results.stats.total_matches, 0);
}

#[test]
fn test_builder_rss_fallback_code_path() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create 300 tiny files to trigger the 250-file RSS check without
    // actually hitting the RSS threshold. This verifies the fallback
    // code path compiles and runs (the flush won't trigger since RSS
    // is well below any threshold for such a small workload).
    for i in 0..300 {
        fs::write(root.join(format!("f{i:04}.txt")), format!("line {i}")).unwrap();
    }

    let mut builder = Builder::new(root).unwrap();
    // Builder::new does NOT attach a ResourceGuard, so the RSS fallback
    // code path runs every 250 files.
    let output = builder.build().expect("build with RSS fallback path");
    assert!(output.exists());

    let reader = Reader::open(&output).unwrap();
    let mut executor = Executor::new(&reader);
    let plan = Planner::plan("line", false).unwrap();
    let (matches, _) = executor.execute(&plan, &QueryOptions::default()).unwrap();
    assert_eq!(matches.len(), 300);
}

#[test]
fn test_format_v13_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let root = dir.path();

    // ── Create known content in multiple files ──────────────────────
    fs::write(root.join("alice.txt"), "Hello from Alice\n")?;
    fs::write(root.join("bob.txt"), "Greetings from Bob\n")?;
    fs::create_dir(root.join("sub"))?;
    fs::write(
        root.join("sub/carol.txt"),
        "Carol says: needle in a haystack\n",
    )?;

    // ── Build the index ─────────────────────────────────────────────
    let mut builder = Builder::new(root)?;
    builder.build()?;

    // ── Open the index file directly to check raw header bytes ──────
    let index_path = root.join(".ix/shard.ix");
    assert!(index_path.exists(), "index file should exist after build");

    let mut raw_file = fs::File::open(&index_path)?;
    let mut header_bytes = [0u8; 256];
    raw_file.read_exact(&mut header_bytes)?;

    // Magic bytes "IX01" at offset 0
    assert_eq!(
        &header_bytes[0..4],
        &format::MAGIC,
        "magic bytes must be IX01"
    );

    // Version major = 1 at offset 4
    let version_major = u16::from_le_bytes(header_bytes[4..6].try_into()?);
    assert_eq!(
        version_major,
        format::VERSION_MAJOR,
        "version_major must equal VERSION_MAJOR"
    );

    // Version minor >= VERSION_MINOR at offset 6
    let version_minor = u16::from_le_bytes(header_bytes[6..8].try_into()?);
    assert!(
        version_minor >= format::VERSION_MINOR,
        "version_minor {version_minor} must be >= VERSION_MINOR {}",
        format::VERSION_MINOR
    );

    // Flags at offset 8: HAS_CDX_INDEX must be set (always-on since v1.3)
    let flags = u64::from_le_bytes(header_bytes[8..16].try_into()?);
    assert!(
        flags & format::flags::HAS_CDX_INDEX != 0,
        "CDX index flag must be set in v1.3+ index (flags=0x{flags:X})"
    );
    assert!(
        flags & format::flags::HAS_BLOOM_FILTERS != 0,
        "bloom filter flag must be set (flags=0x{flags:X})"
    );

    // ── Open via Reader and verify parsed header fields ─────────────
    let reader = Reader::open(&index_path)?;

    // File count: we created alice.txt, bob.txt, sub/carol.txt
    assert_eq!(
        reader.header.file_count, 3,
        "file_count should be 3, got {}",
        reader.header.file_count
    );

    // Trigram count: must be > 0 for any non-trivial content
    assert!(
        reader.header.trigram_count > 0,
        "trigram_count must be > 0 for files with content"
    );

    // Header version fields match raw bytes
    assert_eq!(reader.header.version_major, format::VERSION_MAJOR);
    assert!(reader.header.version_minor >= format::VERSION_MINOR);

    // Metadata convenience accessor
    let meta = reader.metadata();
    assert_eq!(meta.file_count, 3);
    assert_eq!(meta.trigram_count, reader.header.trigram_count);

    // ── Search for known literal content via Executor ───────────────
    let mut executor = Executor::new(&reader);

    // "Hello" should match only alice.txt
    let plan = Planner::plan("Hello", false)?;
    let (matches, stats) = executor.execute(&plan, &QueryOptions::default())?;
    assert_eq!(matches.len(), 1, "expected 1 match for 'Hello'");
    assert_eq!(stats.total_matches, 1);
    assert!(
        matches[0].file_path.to_string_lossy().contains("alice.txt"),
        "match should be in alice.txt"
    );

    // "needle" should match only sub/carol.txt
    let plan2 = Planner::plan("needle", false)?;
    let (matches2, stats2) = executor.execute(&plan2, &QueryOptions::default())?;
    assert_eq!(matches2.len(), 1, "expected 1 match for 'needle'");
    assert_eq!(stats2.total_matches, 1);
    assert!(
        matches2[0]
            .file_path
            .to_string_lossy()
            .contains("carol.txt"),
        "match should be in carol.txt"
    );

    // "from" should match both alice.txt and bob.txt
    let plan3 = Planner::plan("from", false)?;
    let (matches3, _) = executor.execute(&plan3, &QueryOptions::default())?;
    assert_eq!(matches3.len(), 2, "expected 2 matches for 'from'");
    let paths: Vec<_> = matches3
        .iter()
        .map(|m| m.file_path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert!(paths.contains(&"alice.txt"));
    assert!(paths.contains(&"bob.txt"));

    // ── Verify non-matching search returns empty gracefully ─────────
    let plan4 = Planner::plan("zzzznotpresent", false)?;
    let (matches4, stats4) = executor.execute(&plan4, &QueryOptions::default())?;
    assert!(matches4.is_empty());
    assert_eq!(stats4.total_matches, 0);

    Ok(())
}
