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

fn search_trigram(shard_path: &Path, query_str: &str) -> bool {
    let reader = Reader::open(shard_path).unwrap();
    let planner_opts = ix::planner::QueryOptions::default();
    let exec_opts = ix::executor::QueryOptions {
        max_results: 10,
        ..Default::default()
    };

    let delta_path = shard_path.parent().unwrap().join("shard.ix.delta");
    let dp = if delta_path.exists() {
        Some(delta_path.as_path())
    } else {
        None
    };

    let res = ix::api::execute(&reader, query_str, planner_opts, &exec_opts, dp).unwrap();
    !res.0.is_empty()
}

/// Helper to get all indexed file paths from the current shard.
fn get_indexed_files(shard_path: &Path, root: &Path) -> Vec<PathBuf> {
    let reader = Reader::open(shard_path).expect("failed to open shard");
    let root_canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    let mut files = Vec::new();
    for i in 0..reader.header.file_count {
        if let Ok(entry) = reader.get_file(i) {
            if entry.status == ix::format::FileStatus::Deleted {
                continue;
            }
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
    fs::write(&valid_txt, "hello valid changed unique_string_123\n").unwrap();
    let shard_path = builder
        .update(std::slice::from_ref(&valid_txt))
        .expect("update failed");
    let files = get_indexed_files(&shard_path, &base);
    assert_eq!(
        files,
        vec![PathBuf::from("foo/valid.txt")],
        "Incremental update must include valid.txt"
    );
    assert!(
        search_trigram(&shard_path, "unique_string_123"),
        "Incremental update must actually index new content"
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
    fs::write(&src_txt, "fn main() { println!(\"unique_string_456\"); }\n").unwrap();
    let shard_path = builder
        .update(std::slice::from_ref(&src_txt))
        .expect("update failed");
    let files = get_indexed_files(&shard_path, &base);
    assert_eq!(files, vec![PathBuf::from("src/main.rs")]);
    assert!(
        search_trigram(&shard_path, "unique_string_456"),
        "Incremental update must actually index new content"
    );

    // File outside watch root does not
    fs::write(&out_txt, "more notes\n").unwrap();
    let shard_path = builder
        .update(std::slice::from_ref(&out_txt))
        .expect("update failed");
    let files = get_indexed_files(&shard_path, &base);
    assert_eq!(files, vec![PathBuf::from("src/main.rs")]);

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_ancestor_dir_exclusion() {
    // e.g. /tmp/target/my-repo/... where `target` is excluded.
    // The repo should still be indexed.
    let base = std::env::temp_dir().join(format!(
        "ix_path_admission_target_{}/target/my-repo",
        std::process::id()
    ));
    fs::create_dir_all(&base).unwrap();

    let valid_txt = base.join("valid.txt");
    fs::write(&valid_txt, "hello valid\n").unwrap();

    let config = Config {
        watch_roots: vec![],
        exclude_patterns: vec!["target".to_string()],
        debounce_ms: None,
        watch: None,
        build: None,
    };

    let mut builder = config.apply_to_builder(Builder::new(&base).unwrap());
    let shard_path = builder.build().expect("build failed");
    let files = get_indexed_files(&shard_path, &base);
    assert_eq!(
        files,
        vec![PathBuf::from("valid.txt")],
        "Ancestor dir 'target' should not exclude repo"
    );

    // Incremental update should also work
    fs::write(&valid_txt, "hello valid unique_string_789\n").unwrap();
    let shard_path = builder
        .update(std::slice::from_ref(&valid_txt))
        .expect("update failed");
    assert!(
        search_trigram(&shard_path, "unique_string_789"),
        "Incremental update must actually index new content despite ancestor name"
    );

    let _ = fs::remove_dir_all(base.parent().unwrap().parent().unwrap());
}

#[test]
fn test_watcher_collect_paths_admission() {
    let mut map = std::collections::HashMap::new();
    let root = PathBuf::from("/repo");
    let watch_roots = vec![PathBuf::from("/repo/src")];
    let exclude_patterns = vec!["node_modules".to_string(), "target".to_string()];

    // Create event inside watch root
    let event = notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
        .add_path(PathBuf::from("/repo/src/main.rs"));
    ix::watcher::Watcher::collect_paths(&mut map, event, &root, &watch_roots, &exclude_patterns);
    assert!(
        map.contains_key(&PathBuf::from("/repo/src/main.rs")),
        "Should admit valid path inside watch root"
    );

    // Modify event inside excluded directory
    let event = notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Data(
        notify::event::DataChange::Any,
    )))
    .add_path(PathBuf::from("/repo/src/node_modules/pkg/index.js"));
    ix::watcher::Watcher::collect_paths(&mut map, event, &root, &watch_roots, &exclude_patterns);
    assert!(
        !map.contains_key(&PathBuf::from("/repo/src/node_modules/pkg/index.js")),
        "Should reject excluded directory"
    );

    // Create event for binary file
    let event = notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
        .add_path(PathBuf::from("/repo/src/lib.so"));
    ix::watcher::Watcher::collect_paths(&mut map, event, &root, &watch_roots, &exclude_patterns);
    assert!(
        !map.contains_key(&PathBuf::from("/repo/src/lib.so")),
        "Should reject binary extension"
    );

    // Rename event into valid path
    let event = notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Name(
        notify::event::RenameMode::To,
    )))
    .add_path(PathBuf::from("/repo/src/new_file.rs"));
    ix::watcher::Watcher::collect_paths(&mut map, event, &root, &watch_roots, &exclude_patterns);
    assert!(
        map.contains_key(&PathBuf::from("/repo/src/new_file.rs")),
        "Should admit rename to valid path"
    );
}

#[test]
fn test_outside_root_admission() {
    let base =
        std::env::temp_dir().join(format!("ix_path_admission_outside_{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();
    let root = base.join("repo");
    fs::create_dir_all(&root).unwrap();
    let outside = base.join("outside");
    fs::create_dir_all(&outside).unwrap();

    let valid_txt = root.join("valid.txt");
    fs::write(&valid_txt, "hello valid\n").unwrap();
    let outside_txt = outside.join("outside.txt");
    fs::write(&outside_txt, "hello outside\n").unwrap();

    let mut builder = Builder::new(&root).unwrap();
    let shard_path = builder.build().expect("build failed");

    let files = get_indexed_files(&shard_path, &root);
    assert_eq!(files, vec![PathBuf::from("valid.txt")]);

    // incremental update on outside file
    fs::write(&outside_txt, "hello outside changed unique_string_999\n").unwrap();
    let shard_path = builder
        .update(std::slice::from_ref(&outside_txt))
        .expect("update failed");
    let files = get_indexed_files(&shard_path, &root);
    assert_eq!(
        files,
        vec![PathBuf::from("valid.txt")],
        "Incremental update must ignore files outside root"
    );
    assert!(
        !search_trigram(&shard_path, "unique_string_999"),
        "Outside file content must not be indexed"
    );

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_file_named_like_excluded_dir() {
    let base = std::env::temp_dir().join(format!(
        "ix_path_admission_file_named_{}",
        std::process::id()
    ));
    fs::create_dir_all(&base).unwrap();

    // `excluded_dir` is a file
    let excluded_dir_file = base.join("excluded_dir");
    fs::write(&excluded_dir_file, "I am a file unique_string_abc\n").unwrap();

    let config = Config {
        watch_roots: vec![],
        exclude_patterns: vec!["excluded_dir".to_string()],
        debounce_ms: None,
        watch: None,
        build: None,
    };

    let mut builder = config.apply_to_builder(Builder::new(&base).unwrap());
    let shard_path = builder.build().expect("build failed");

    let files = get_indexed_files(&shard_path, &base);
    assert_eq!(
        files,
        vec![PathBuf::from("excluded_dir")],
        "File named like excluded directory should be indexed"
    );
    assert!(search_trigram(&shard_path, "unique_string_abc"));

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_dir_named_like_excluded_file_extension() {
    let base = std::env::temp_dir().join(format!(
        "ix_path_admission_dir_named_{}",
        std::process::id()
    ));
    fs::create_dir_all(&base).unwrap();

    let cache_so_dir = base.join("cache.so");
    fs::create_dir_all(&cache_so_dir).unwrap();

    let inside_rs = cache_so_dir.join("inside.rs");
    fs::write(&inside_rs, "hello cache.so inside unique_string_def\n").unwrap();

    let mut builder = Builder::new(&base).unwrap();
    let shard_path = builder.build().expect("build failed");

    let files = get_indexed_files(&shard_path, &base);
    assert_eq!(
        files,
        vec![PathBuf::from("cache.so/inside.rs")],
        "Directory named like binary extension should be traversed"
    );
    assert!(search_trigram(&shard_path, "unique_string_def"));

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_removal_event_coverage() {
    let base =
        std::env::temp_dir().join(format!("ix_path_admission_removal_{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();

    let valid_txt = base.join("valid.txt");
    fs::write(
        &valid_txt,
        "hello valid unique_string_ghi
",
    )
    .unwrap();

    let config = Config {
        watch_roots: vec![],
        exclude_patterns: vec!["excluded_dir".to_string()],
        debounce_ms: None,
        watch: None,
        build: None,
    };

    let mut builder = config.apply_to_builder(Builder::new(&base).unwrap());
    let shard_path = builder.build().expect("build failed");

    assert!(search_trigram(&shard_path, "unique_string_ghi"));

    // Remove valid.txt and run update
    fs::remove_file(&valid_txt).unwrap();
    let shard_path = builder
        .update(std::slice::from_ref(&valid_txt))
        .expect("update failed");
    assert!(
        !search_trigram(&shard_path, "unique_string_ghi"),
        "Removed file should be tombstoned in delta and not found"
    );

    // Removed file underneath excluded dir
    let excluded_dir = base.join("excluded_dir");
    let nested_txt = excluded_dir.join("file.txt");
    // it was never indexed, but let's say an event comes for it
    let _shard_path = builder
        .update(std::slice::from_ref(&nested_txt))
        .expect("update failed");
    // we just want to ensure it doesn't crash or index it.

    // Removed leaf file whose filename equals an excluded-directory pattern
    let excluded_dir_file = base.join("excluded_dir");
    fs::write(
        &excluded_dir_file,
        "I am a file unique_string_jkl
",
    )
    .unwrap();
    let shard_path = builder
        .update(std::slice::from_ref(&excluded_dir_file))
        .expect("update failed");
    assert!(
        search_trigram(&shard_path, "unique_string_jkl"),
        "Added file named like excluded dir should be indexed"
    );

    fs::remove_file(&excluded_dir_file).unwrap();
    let shard_path = builder
        .update(std::slice::from_ref(&excluded_dir_file))
        .expect("update failed");
    assert!(
        !search_trigram(&shard_path, "unique_string_jkl"),
        "Removed file named like excluded dir should be tombstoned"
    );

    let _ = fs::remove_dir_all(&base);
}
