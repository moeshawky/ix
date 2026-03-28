use ix::builder::Builder;
use ix::executor::{Executor, QueryOptions};
use ix::planner::Planner;
use ix::reader::Reader;
use std::fs;
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

    let mut builder = Builder::new(root);
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let executor = Executor::new(&reader);

    // Search for "abc"
    let plan = Planner::plan("abc", false);
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

    let mut builder = Builder::new(root);
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let executor = Executor::new(&reader);

    let plan = Planner::plan("abc", false);
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

    let mut builder = Builder::new(root);
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let executor = Executor::new(&reader);

    let plan = Planner::plan("match", false);
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

    let mut builder = Builder::new(root);
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let executor = Executor::new(&reader);

    let plan = Planner::plan("findme", false);

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
