//! Regression tests for path admission parity between full builds and incremental updates.
//!
//! Enforces that files excluded by configuration (`exclude_patterns`), `watch_roots`,
//! or internal rules remain excluded when updated incrementally via file-system events.
//!
//! Note: .gitignore and .ixignore parity with incremental updates is currently
//! intentionally omitted, as notify events are not evaluated against ignore rules
//! in the daemon event loop without expanding scope. This semantic gap is known.

use ix::builder::Builder;
use ix::config::Config;
use ix::reader::Reader;
use std::fs;
use std::path::{Path, PathBuf};

/// Helper to get all indexed file paths from the current shard.
fn get_indexed_files(shard_path: &Path, root: &Path) -> Vec<PathBuf> {
    let reader = Reader::open(shard_path).expect("failed to open shard");
    let root_canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    let mut files = Vec::new();
    for i in 0..reader.header.file_count {
        if let Ok(entry) = reader.get_file(i) {
            let rel = entry
                .path
                .strip_prefix(&root_canonical)
                .or_else(|_| entry.path.strip_prefix(root))
                .unwrap_or(&entry.path);
            files.push(rel.to_path_buf());
        }
    }
    files.sort();
    files
}

#[test]
fn test_config_exclusion_parity() {
    let base =
        std::env::temp_dir().join(format!("ix_path_admission_exclude_{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();

    let excluded_dir = base.join("excluded_dir");
    let foo_dir = base.join("foo");
    fs::create_dir_all(&excluded_dir).unwrap();
    fs::create_dir_all(&foo_dir).unwrap();

    let file_txt = excluded_dir.join("file.txt");
    let nested_txt = foo_dir.join("excluded_dir").join("file.txt");
    let valid_txt = foo_dir.join("valid.txt");

    fs::create_dir_all(foo_dir.join("excluded_dir")).unwrap();

    fs::write(&file_txt, "hello excluded\n").unwrap();
    fs::write(&nested_txt, "hello nested excluded\n").unwrap();
    fs::write(&valid_txt, "hello valid\n").unwrap();

    let config = Config {
        watch_roots: vec![],
        exclude_patterns: vec!["excluded_dir".to_string()],
        debounce_ms: None,
        watch: None,
        build: None,
    };

    let mut builder = config.apply_to_builder(Builder::new(&base).unwrap());

    // 1. Full build excludes `excluded_dir/file.txt` and `foo/excluded_dir/file.txt`
    let shard_path = builder.build().expect("build failed");
    let files = get_indexed_files(&shard_path, &base);

    assert_eq!(files, vec![PathBuf::from("foo/valid.txt")]);

    // 2. Incremental update to `excluded_dir/file.txt` is ignored
    fs::write(&file_txt, "hello excluded changed\n").unwrap();
    let shard_path = builder
        .update(std::slice::from_ref(&file_txt))
        .expect("update failed");
    let files = get_indexed_files(&shard_path, &base);
    assert_eq!(
        files,
        vec![PathBuf::from("foo/valid.txt")],
        "Incremental update must ignore excluded_dir/file.txt"
    );

    // 3. Nested `foo/excluded_dir/file.txt` is ignored
    fs::write(&nested_txt, "hello nested excluded changed\n").unwrap();
    let shard_path = builder
        .update(std::slice::from_ref(&nested_txt))
        .expect("update failed");
    let files = get_indexed_files(&shard_path, &base);
    assert_eq!(
        files,
        vec![PathBuf::from("foo/valid.txt")],
        "Incremental update must ignore nested excluded_dir"
    );

    // 4. Non-excluded sibling file is indexed incrementally
    fs::write(&valid_txt, "hello valid changed\n").unwrap();
    let shard_path = builder
        .update(std::slice::from_ref(&valid_txt))
        .expect("update failed");
    let files = get_indexed_files(&shard_path, &base);
    assert_eq!(
        files,
        vec![PathBuf::from("foo/valid.txt")],
        "Incremental update must include valid.txt"
    );

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_internal_state_parity() {
    let base =
        std::env::temp_dir().join(format!("ix_path_admission_internal_{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();

    let valid_txt = base.join("valid.txt");
    fs::write(&valid_txt, "hello valid\n").unwrap();

    let mut builder = Builder::new(&base).unwrap();
    let _shard_path = builder.build().expect("build failed");
    let ix_dir = base.join(".ix");

    // `.ix/...` events remain ignored.
    let ix_file = ix_dir.join("some_internal_file.txt");
    fs::write(&ix_file, "internal\n").unwrap();

    let shard_path = builder
        .update(std::slice::from_ref(&ix_file))
        .expect("update failed");
    let files = get_indexed_files(&shard_path, &base);

    assert_eq!(
        files,
        vec![PathBuf::from("valid.txt")],
        ".ix files must be ignored incrementally"
    );

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_watch_roots_parity() {
    let base = std::env::temp_dir().join(format!("ix_path_admission_roots_{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();

    let src_dir = base.join("src");
    let out_dir = base.join("out");
    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&out_dir).unwrap();

    let src_txt = src_dir.join("main.rs");
    let out_txt = out_dir.join("bin.o");

    fs::write(&src_txt, "fn main() {}\n").unwrap();
    fs::write(&out_txt, "binary data\n").unwrap(); // though .o is ignored anyway, let's use .txt
    let out_txt = out_dir.join("notes.txt");
    fs::write(&out_txt, "some notes\n").unwrap();

    let config = Config {
        watch_roots: vec![src_dir.clone()],
        exclude_patterns: vec![],
        debounce_ms: None,
        watch: None,
        build: None,
    };

    let mut builder = config.apply_to_builder(Builder::new(&base).unwrap());
    let shard_path = builder.build().expect("build failed");

    let files = get_indexed_files(&shard_path, &base);
    assert_eq!(files, vec![PathBuf::from("src/main.rs")]);

    // File inside watch root updates
    fs::write(&src_txt, "fn main() { println!(); }\n").unwrap();
    let shard_path = builder
        .update(std::slice::from_ref(&src_txt))
        .expect("update failed");
    let files = get_indexed_files(&shard_path, &base);
    assert_eq!(files, vec![PathBuf::from("src/main.rs")]);

    // File outside watch root does not
    fs::write(&out_txt, "more notes\n").unwrap();
    let shard_path = builder
        .update(std::slice::from_ref(&out_txt))
        .expect("update failed");
    let files = get_indexed_files(&shard_path, &base);
    assert_eq!(files, vec![PathBuf::from("src/main.rs")]);

    let _ = fs::remove_dir_all(&base);
}
