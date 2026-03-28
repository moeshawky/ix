use ix::executor::{Executor, QueryOptions};
use ix::reader::Reader;
use regex::Regex;
use std::io::Cursor;
use std::path::PathBuf;
use tempfile::tempdir;
use std::fs;

// Mock Reader for Executor::new
fn create_empty_index(path: &std::path::Path) {
    let mut builder = ix::builder::Builder::new(path);
    builder.build().unwrap();
}

#[test]
fn test_streaming_edge_cases() {
    let dir = tempdir().unwrap();
    let index_path = dir.path().join(".ix/shard.ix");
    create_empty_index(dir.path());
    let reader = Reader::open(&index_path).unwrap();
    let executor = Executor::new(&reader);
    let regex = Regex::new("pattern").unwrap();
    let options = QueryOptions::default();

    // 1. Empty stream
    let data: &[u8] = b"";
    let matches = executor.verify_stream_for_test(Cursor::new(data), PathBuf::from("test"), &regex, &options).unwrap();
    assert_eq!(matches.len(), 0);

    // 2. Large line
    let large_line = "a".repeat(100_000) + "pattern" + &"b".repeat(100_000);
    let matches = executor.verify_stream_for_test(Cursor::new(large_line.as_bytes()), PathBuf::from("test"), &regex, &options).unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].byte_offset, 100_000);

    // 3. Multiple matches one line
    let multi_match = "pattern 1 pattern 2";
    let matches = executor.verify_stream_for_test(Cursor::new(multi_match.as_bytes()), PathBuf::from("test"), &regex, &options).unwrap();
    // Currently returns 1 because we use regex.find()
    assert_eq!(matches.len(), 1);
}

#[test]
fn test_streaming_binary_detection() {
    let dir = tempdir().unwrap();
    let index_path = dir.path().join(".ix/shard.ix");
    create_empty_index(dir.path());
    let reader = Reader::open(&index_path).unwrap();
    let executor = Executor::new(&reader);
    let regex = Regex::new("pattern").unwrap();
    let options = QueryOptions::default();

    // 1. Binary stream (mostly nulls)
    let binary_data: Vec<u8> = vec![0u8; 1000];
    let matches = executor.verify_stream_for_test(Cursor::new(binary_data), PathBuf::from("test"), &regex, &options).unwrap();
    assert_eq!(matches.len(), 0);

    // 2. Not binary (mostly text)
    let text_data = "This is a text file with pattern in it.\n".repeat(100);
    let matches = executor.verify_stream_for_test(Cursor::new(text_data.as_bytes()), PathBuf::from("test"), &regex, &options).unwrap();
    assert_eq!(matches.len(), 100);
}

#[test]
fn test_streaming_context_lookahead() {
    let dir = tempdir().unwrap();
    let index_path = dir.path().join(".ix/shard.ix");
    create_empty_index(dir.path());
    let reader = Reader::open(&index_path).unwrap();
    let executor = Executor::new(&reader);
    let regex = Regex::new("match").unwrap();
    let mut options = QueryOptions::default();
    options.context_lines = 2;

    // Match with context_after near EOF
    let data = "line1\nline2\nmatch\nline4\n";
    let matches = executor.verify_stream_for_test(Cursor::new(data.as_bytes()), PathBuf::from("test"), &regex, &options).unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].context_before, vec!["line1", "line2"]);
    assert_eq!(matches[0].context_after, vec!["line4"]);

    // Overlapping matches
    let data = "match1\nline2\nmatch2\nline4\nline5";
    let matches = executor.verify_stream_for_test(Cursor::new(data.as_bytes()), PathBuf::from("test"), &Regex::new("match").unwrap(), &options).unwrap();
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].context_after, vec!["line2", "match2"]);
    assert_eq!(matches[1].context_before, vec!["match1", "line2"]);
    assert_eq!(matches[1].context_after, vec!["line4", "line5"]);
}
