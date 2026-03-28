use ix::builder::Builder;
use ix::executor::{Executor, QueryOptions};
use ix::planner::Planner;
use ix::reader::Reader;
use std::fs;
use tempfile::tempdir;

#[test]
fn integration_search_literal() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create test files
    fs::write(
        root.join("test1.txt"),
        "hello world\nthis is a test\nix search tool",
    )
    .unwrap();
    fs::write(
        root.join("test2.txt"),
        "another file\nwith different content\nhello again",
    )
    .unwrap();
    fs::write(root.join("binary.dat"), b"some data\0with null byte").unwrap();

    // Build index
    let mut builder = Builder::new(root);
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    assert!(index_path.exists());

    // Search
    let reader = Reader::open(&index_path).unwrap();
    let executor = Executor::new(&reader);

    // Literal search
    let plan = Planner::plan("hello", false);
    let (matches, _) = executor.execute(&plan, &QueryOptions::default()).unwrap();
    assert_eq!(matches.len(), 2);
}

#[test]
fn integration_search_regex() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    fs::write(
        root.join("code.rs"),
        "fn main() {\n    let x = 42;\n    println!(\"{}\", x);\n}",
    )
    .unwrap();

    let mut builder = Builder::new(root);
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let executor = Executor::new(&reader);

    // Regex search
    let plan = Planner::plan("let [a-z] =", true);
    let (matches, _) = executor.execute(&plan, &QueryOptions::default()).unwrap();
    assert_eq!(matches.len(), 1);
    assert!(matches[0].line_content.contains("let x = 42"));
}
