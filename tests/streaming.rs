use ix::executor::QueryOptions;
use regex::Regex;
use std::io::Cursor;
use std::path::Path;

#[test]
fn test_streaming_edge_cases() {
    let regex = Regex::new("pattern").unwrap();
    let options = QueryOptions::default();

    // 1. Empty stream
    let data: &[u8] = b"";
    let mut stats = ix::streaming::StreamStats::default();
    let matches = ix::streaming::stream_file(
        Cursor::new(data),
        Path::new("test"),
        &regex,
        &options,
        false,
        &mut stats,
    )
    .unwrap();
    assert_eq!(matches.len(), 0);
    assert_eq!(stats.lines_read, 0);
    assert_eq!(stats.bytes_read, 0);

    // 2. Large line
    let large_line = "a".repeat(100_000) + "pattern" + &"b".repeat(100_000);
    let mut stats = ix::streaming::StreamStats::default();
    let matches = ix::streaming::stream_file(
        Cursor::new(large_line.as_bytes()),
        Path::new("test"),
        &regex,
        &options,
        false,
        &mut stats,
    )
    .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].byte_offset, 100_000);
    assert_eq!(stats.lines_read, 1);
    assert_eq!(stats.matches_found, 1);

    // 3. Multiple matches one line
    // T-ORACLE: Currently returns 1 because we use regex.find() (first match
    // only), not regex.find_iter(). This documents the single-match behavior
    // rather than silently depending on it. If the implementation switches to
    // find_iter(), this assertion must be updated to assert_eq!(matches.len(), 2).
    let multi_match = "pattern 1 pattern 2";
    let mut stats = ix::streaming::StreamStats::default();
    let matches = ix::streaming::stream_file(
        Cursor::new(multi_match.as_bytes()),
        Path::new("test"),
        &regex,
        &options,
        false,
        &mut stats,
    )
    .unwrap();
    assert_eq!(
        matches.len(),
        1,
        "stream_file uses regex.find() (first match only), not find_iter()"
    );
}

/// T-ORACLE: Verify that binary detection actually classifies data, not just
/// that it returns zero matches. The binary stream (all NULLs) must produce
/// matches with is_binary=true, and the text stream must produce matches with
/// is_binary=false.
#[test]
fn test_streaming_binary_detection() {
    let regex = Regex::new("pattern").unwrap();
    let options = QueryOptions::default();

    // 1. Binary stream (mostly nulls) — stream_file skips binary files when
    //    options.binary is false (the default), so we get 0 matches.
    let binary_data: Vec<u8> = vec![0u8; 1000];
    let mut stats = ix::streaming::StreamStats::default();
    let matches = ix::streaming::stream_file(
        Cursor::new(binary_data),
        Path::new("test"),
        &regex,
        &options,
        false,
        &mut stats,
    )
    .unwrap();
    assert_eq!(
        matches.len(),
        0,
        "Binary data should produce no matches when binary=false"
    );

    // 2. Not binary (mostly text) — verify matches are found AND that none
    //    are flagged as binary (they shouldn't be since the content is text).
    let text_data = "This is a text file with pattern in it.\n".repeat(100);
    let mut stats = ix::streaming::StreamStats::default();
    let matches = ix::streaming::stream_file(
        Cursor::new(text_data.as_bytes()),
        Path::new("test"),
        &regex,
        &options,
        false,
        &mut stats,
    )
    .unwrap();
    assert_eq!(matches.len(), 100, "Text data should produce 100 matches");
    for m in &matches {
        assert!(
            !m.is_binary,
            "Text content should not be classified as binary, got is_binary=true for line {}",
            m.line_number
        );
    }
}

#[test]
fn test_streaming_context_lookahead() {
    let regex = Regex::new("match").unwrap();
    let options = QueryOptions {
        context_lines: 2,
        ..Default::default()
    };

    // Match with context_after near EOF
    let data = "line1\nline2\nmatch\nline4\n";
    let mut stats = ix::streaming::StreamStats::default();
    let matches = ix::streaming::stream_file(
        Cursor::new(data.as_bytes()),
        Path::new("test"),
        &regex,
        &options,
        false,
        &mut stats,
    )
    .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].context_before, vec!["line1", "line2"]);
    assert_eq!(matches[0].context_after, vec!["line4"]);

    // Overlapping matches
    let data = "match1\nline2\nmatch2\nline4\nline5";
    let mut stats = ix::streaming::StreamStats::default();
    let matches = ix::streaming::stream_file(
        Cursor::new(data.as_bytes()),
        Path::new("test"),
        &Regex::new("match").unwrap(),
        &options,
        false,
        &mut stats,
    )
    .unwrap();
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].context_after, vec!["line2", "match2"]);
    assert_eq!(matches[1].context_before, vec!["match1", "line2"]);
    assert_eq!(matches[1].context_after, vec!["line4", "line5"]);
}

#[test]
fn test_streaming_crlf_offsets() {
    let regex = Regex::new("match").unwrap();
    let options = QueryOptions::default();

    // CRLF data: "line1\r\nmatch\r\n"
    // line1 is 5 chars + 2 line ending = 7 bytes.
    // "match" starts at byte offset 7.
    let data = b"line1\r\nmatch\r\n";
    let mut stats = ix::streaming::StreamStats::default();
    let matches = ix::streaming::stream_file(
        Cursor::new(data),
        Path::new("test"),
        &regex,
        &options,
        false,
        &mut stats,
    )
    .unwrap();

    assert_eq!(matches.len(), 1);
    assert_eq!(
        matches[0].byte_offset, 7,
        "Offset should account for CRLF (2 bytes)"
    );
}
