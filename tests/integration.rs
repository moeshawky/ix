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

    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    assert!(index_path.exists());

    let reader = Reader::open(&index_path).unwrap();
    let mut executor = Executor::new(&reader);

    let plan = Planner::plan("hello", false);
    let (matches, _) = executor.execute(&plan, &QueryOptions::default()).unwrap();
    assert_eq!(matches.len(), 2, "Expected 2 files matching 'hello'");
    let file_names: std::collections::BTreeSet<&str> = matches
        .iter()
        .map(|m| m.file_path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(
        file_names,
        ["test1.txt", "test2.txt"]
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        "Should match test1.txt and test2.txt"
    );
}

#[test]
fn integration_search_regex() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    fs::write(
        root.join("code.rs"),
        "fn main() {\n let x = 42;\n println!(\"{}\", x);\n}",
    )
    .unwrap();

    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let mut executor = Executor::new(&reader);

    let plan = Planner::plan("let [a-z] =", true);
    let (matches, _) = executor.execute(&plan, &QueryOptions::default()).unwrap();
    assert_eq!(
        matches.len(),
        1,
        "Expected exactly 1 match for regex 'let [a-z] ='"
    );
    assert_eq!(
        matches[0].file_path.file_name().unwrap().to_str().unwrap(),
        "code.rs",
        "Should match code.rs"
    );
    assert_eq!(
        matches[0].line_content.trim(),
        "let x = 42;",
        "Should match exact line content"
    );
}
