//! Convenience API for search operations.
//!
//! Thin wrappers around the planner + executor pipeline for simple
//! single-call searches.

use std::path::Path;

use crate::error::Result;
use crate::executor::{Executor, Match, QueryOptions, QueryStats};
use crate::planner::Planner;
use crate::reader::Reader;

/// Execute a search against an open index with a single call.
///
/// Plans the query with the given planner options, creates an executor,
/// attaches an optional delta path for recently-changed files, and runs
/// the search. Returns all matches and query statistics.
///
/// # Arguments
///
/// * `reader` - Open index reader (mmap-backed).
/// * `pattern` - Search pattern string (literal or regex).
/// * `planner_opts` - Query planning options: `is_regex`, `ignore_case`,
///   `multiline`, `word_boundary`.
/// * `exec_opts` - Execution options: context lines, type filter, result
///   caps, etc.
/// * `delta_path` - Optional path to `.ix/shard.ix.delta` for searching
///   files changed since the last full build.
///
/// # Errors
///
/// Returns an error if the index cannot be read, posting data is
/// corrupted, or file content cannot be accessed during verification.
pub fn execute(
    reader: &Reader,
    pattern: &str,
    planner_opts: crate::planner::QueryOptions,
    exec_opts: &QueryOptions,
    delta_path: Option<&Path>,
) -> Result<(Vec<Match>, QueryStats)> {
    let plan = Planner::plan_with_options(pattern, planner_opts)?;
    let mut executor = Executor::new(reader);
    if let Some(dp) = delta_path {
        executor.set_delta_path(dp.to_path_buf());
    }
    executor.execute(&plan, exec_opts)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::builder::Builder;
    use crate::executor;
    use crate::planner;
    use crate::reader::Reader;

    /// Helper: build an index in a temp dir, return the reader and root path.
    /// Writes one file with `content` into `filename`, builds the index, and
    /// opens the reader.
    fn build_index(root: &std::path::Path, filename: &str, content: &str) -> Reader {
        fs::write(root.join(filename), content).unwrap();
        let mut builder = Builder::new(root).unwrap();
        builder.build().unwrap();
        let index_path = root.join(".ix").join("shard.ix");
        Reader::open(&index_path).unwrap()
    }

    // ── literal search ──────────────────────────────────────────────

    #[test]
    fn execute_literal_search() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let reader = build_index(root, "hello.txt", "hello world\nneedle in haystack\n");

        let (matches, stats) = super::execute(
            &reader,
            "needle",
            planner::QueryOptions::default(),
            &executor::QueryOptions::default(),
            None,
        )
        .unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(stats.total_matches, 1);
        assert!(stats.files_verified >= 1);
        assert!(matches[0].line_content.contains("needle"));
        // Verify the file path ends with "hello.txt"
        assert!(
            matches[0]
                .file_path
                .to_string_lossy()
                .ends_with("hello.txt")
        );
    }

    // ── regex search ────────────────────────────────────────────────

    #[test]
    fn execute_regex_search() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let reader = build_index(root, "data.txt", "foo\nbar\nbaz\n");

        let (matches, _) = super::execute(
            &reader,
            r"ba[rz]",
            planner::QueryOptions {
                is_regex: true,
                ..Default::default()
            },
            &executor::QueryOptions::default(),
            None,
        )
        .unwrap();

        assert_eq!(
            matches.len(),
            2,
            "regex ba[rz] should match 'bar' and 'baz'"
        );
        let lines: Vec<&str> = matches.iter().map(|m| m.line_content.trim()).collect();
        assert!(lines.contains(&"bar"));
        assert!(lines.contains(&"baz"));
    }

    // ── ignore_case ─────────────────────────────────────────────────

    #[test]
    fn execute_ignore_case() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let reader = build_index(root, "case.txt", "UPPERCASE\nlowercase\nMixedCase\n");

        let (matches, _) = super::execute(
            &reader,
            "mixedcase",
            planner::QueryOptions {
                ignore_case: true,
                ..Default::default()
            },
            &executor::QueryOptions::default(),
            None,
        )
        .unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line_content.trim(), "MixedCase");
    }

    // ── no matches ──────────────────────────────────────────────────

    #[test]
    fn execute_no_matches() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let reader = build_index(root, "only.txt", "only this content exists\n");

        let (matches, stats) = super::execute(
            &reader,
            "nonexistent",
            planner::QueryOptions::default(),
            &executor::QueryOptions::default(),
            None,
        )
        .unwrap();

        assert!(matches.is_empty());
        assert_eq!(stats.total_matches, 0);
    }

    // ── context_lines ───────────────────────────────────────────────

    #[test]
    fn execute_with_context_lines() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let reader = build_index(root, "ctx.txt", "before1\nbefore2\nHIT\nafter1\nafter2\n");

        let (matches, _) = super::execute(
            &reader,
            "HIT",
            planner::QueryOptions::default(),
            &executor::QueryOptions {
                context_lines: 2,
                ..Default::default()
            },
            None,
        )
        .unwrap();

        assert_eq!(matches.len(), 1);
        let m = &matches[0];
        assert_eq!(m.line_content.trim(), "HIT");
        assert_eq!(
            m.context_before,
            vec!["before1".to_string(), "before2".to_string()]
        );
        // context_after may have fewer lines if match is near EOF
        assert!(!m.context_after.is_empty());
        assert_eq!(m.context_after[0], "after1");
    }

    // ── max_results ─────────────────────────────────────────────────

    #[test]
    fn execute_max_results() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // File with 5 lines, each containing "match"
        let content = "match\nmatch\nmatch\nmatch\nmatch\n";
        let reader = build_index(root, "caps.txt", content);

        let (matches, stats) = super::execute(
            &reader,
            "match",
            planner::QueryOptions::default(),
            &executor::QueryOptions {
                max_results: 3,
                ..Default::default()
            },
            None,
        )
        .unwrap();

        assert_eq!(matches.len(), 3, "should cap at 3 results");
        assert_eq!(stats.total_matches, 3);
    }
}
