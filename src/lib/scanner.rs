//! Fallback scanner (no index, competitive with ripgrep).
//!
//! Used when .ix index is missing or explicitly disabled.

use crate::decompress::maybe_decompress;
use crate::error::Result;
use crate::executor::{Match, QueryOptions};
use ignore::WalkBuilder;
use memmap2::Mmap;
use rayon::prelude::*;
use regex::Regex;
use std::fs::File;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Partiality and skip counters collected during a raw filesystem scan.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScanStats {
    /// Files skipped due to I/O, permission, or decompression errors.
    pub files_skipped_io: u64,
    /// Files skipped because they were detected as binary.
    pub files_skipped_binary: u64,
    /// Archive entries skipped due to corruption or archive safety limits.
    pub archive_entries_skipped: u64,
    /// Whether results were truncated due to `options.max_results`.
    pub truncated: bool,
}

#[derive(Default)]
struct ScanStatsAccum {
    files_skipped_io: AtomicU64,
    files_skipped_binary: AtomicU64,
    archive_entries_skipped: AtomicU64,
}

/// Maximum uncompressed file size admitted during directory walk (`10 MiB`).
const MAX_WALK_FILE_BYTES: u64 = 10 * 1024 * 1024;
/// Maximum compressed/archive file size admitted for mmap or decompression (`100 MiB`).
const MAX_COMPRESSED_FILE_BYTES: u64 = 100 * 1024 * 1024;

/// Fallback scanner that reads files directly (no index).
///
/// Used when the `.ix` index is missing or explicitly disabled. Walks the
/// filesystem and applies regex matching in parallel via Rayon.
pub struct Scanner {
    root: PathBuf,
}

impl Scanner {
    /// Create a new scanner rooted at `root`.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
        }
    }

    /// Scan files in the scanner's root directory for `pattern`.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The scanner root path does not exist
    /// - The regex is invalid
    /// - File I/O fails during the walk or content reading
    pub fn scan(
        &self,
        pattern: &str,
        is_regex: bool,
        ignore_case: bool,
        options: &QueryOptions,
    ) -> Result<Vec<Match>> {
        self.scan_with_stats(pattern, is_regex, ignore_case, options)
            .map(|(matches, _)| matches)
    }

    /// Scan files in the scanner's root directory for `pattern`, returning both
    /// matches and partiality/skip statistics.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The scanner root path does not exist
    /// - The regex is invalid
    /// - File I/O fails during the walk or content reading
    pub fn scan_with_stats(
        &self,
        pattern: &str,
        is_regex: bool,
        ignore_case: bool,
        options: &QueryOptions,
    ) -> Result<(Vec<Match>, ScanStats)> {
        // Early-exit: surface a clear error when the root directory does not
        // exist, rather than silently returning an empty result set from
        // walk-iterator errors.
        if !self.root.exists() {
            return Err(crate::error::Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("scanner root does not exist: {}", self.root.display()),
            )));
        }

        let regex = build_search_regex(pattern, is_regex, ignore_case, options)?;
        let include_archives = options.archive && cfg!(feature = "archive");
        let paths = collect_candidate_paths(&self.root, include_archives, options.decompress);

        let matches_found = AtomicU32::new(0);
        let accum = ScanStatsAccum::default();
        let mut matches: Vec<Match> = paths
            .into_par_iter()
            .filter_map(|path| scan_single_path(&path, &regex, options, &matches_found, &accum))
            .flatten()
            .collect();

        let truncated = options.max_results > 0 && matches.len() > options.max_results;
        if truncated {
            matches.truncate(options.max_results);
        }

        let stats = ScanStats {
            files_skipped_io: accum.files_skipped_io.load(Ordering::Relaxed),
            files_skipped_binary: accum.files_skipped_binary.load(Ordering::Relaxed),
            archive_entries_skipped: accum.archive_entries_skipped.load(Ordering::Relaxed),
            truncated,
        };

        Ok((matches, stats))
    }

    fn scan_file(path: &Path, regex: &Regex, options: &QueryOptions) -> Result<Vec<Match>> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        if metadata.len() > MAX_COMPRESSED_FILE_BYTES {
            // Bound mmap size even when decompress is enabled to avoid huge mappings in parallel.
            return Ok(vec![]);
        }

        let owned_opts;
        let effective_options = if options.files_only {
            owned_opts = QueryOptions {
                max_results: 1,
                ..options.clone()
            };
            &owned_opts
        } else {
            options
        };

        // SAFETY: The file is opened read-only and the mmap is used only
        // within this function via a Cursor, which treats the mapped memory
        // as an immutable byte slice. No concurrent modification to the
        // underlying file is expected during scanning.
        let mmap = unsafe { Mmap::map(&file)? };
        let mut stats = crate::streaming::StreamStats::default();

        if effective_options.decompress
            && let Some(reader) = maybe_decompress(path, &mmap)?
        {
            return crate::streaming::stream_file(
                reader,
                path,
                regex,
                effective_options,
                true,
                &mut stats,
            );
        }

        crate::streaming::stream_file(
            Cursor::new(&*mmap),
            path,
            regex,
            effective_options,
            false,
            &mut stats,
        )
    }
}

fn build_search_regex(
    pattern: &str,
    is_regex: bool,
    ignore_case: bool,
    options: &QueryOptions,
) -> Result<Regex> {
    let i_prefix = if ignore_case { "(?i)" } else { "" };
    let s_prefix = if options.multiline { "(?s)" } else { "" };

    let regex_pat = if is_regex {
        let (leading_flags, body) = crate::planner::split_leading_flags(pattern);
        let effective_i = if ignore_case && !leading_flags.starts_with("(?i)") {
            "(?i)"
        } else {
            ""
        };
        let wrapped = if options.word_boundary {
            format!("\\b(?:{body})\\b")
        } else {
            body.to_string()
        };
        format!("{effective_i}{s_prefix}{leading_flags}{wrapped}")
    } else {
        let escaped = regex::escape(pattern);
        let wrapped = if options.word_boundary {
            format!("\\b{escaped}\\b")
        } else {
            escaped
        };
        format!("{i_prefix}{s_prefix}{wrapped}")
    };

    Ok(Regex::new(&regex_pat)?)
}

fn is_archive_path(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let is_tar_gz = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|s| s.ends_with(".tar.gz"));
    ext == "zip" || is_tar_gz
}

fn is_compressed_path(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(ext, "gz" | "zst" | "bz2" | "xz")
}

fn collect_candidate_paths(root: &Path, include_archives: bool, decompress: bool) -> Vec<PathBuf> {
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .require_git(true) // within-repo .gitignore only; never ancestor ~/.gitignore (audit D4)
        .add_custom_ignore_filename(".ixignore")
        .filter_entry(move |entry| should_admit_entry(entry, include_archives, decompress))
        .build();

    walker
        .filter_map(|result| match result {
            Ok(entry) => Some(entry),
            Err(e) => {
                eprintln!("ix: warning: scanner skipping path: {e}");
                None
            }
        })
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .map(|entry| entry.path().to_owned())
        .collect()
}

fn should_admit_entry(entry: &ignore::DirEntry, include_archives: bool, decompress: bool) -> bool {
    let path = entry.path();
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    if entry.file_type().is_some_and(|t| t.is_dir()) {
        return !matches!(
            name,
            "lost+found"
                | ".git"
                | "node_modules"
                | "target"
                | "__pycache__"
                | ".tox"
                | ".venv"
                | "venv"
                | ".ix"
        );
    }

    if !entry.file_type().is_some_and(|t| t.is_file()) {
        return true;
    }

    let is_archive = is_archive_path(path);
    let size_limit = if (include_archives && is_archive) || (decompress && is_compressed_path(path))
    {
        MAX_COMPRESSED_FILE_BYTES
    } else {
        MAX_WALK_FILE_BYTES
    };

    if let Ok(metadata) = entry.metadata()
        && metadata.len() > size_limit
    {
        return false;
    }

    if matches!(
        name,
        "Cargo.lock" | "package-lock.json" | "pnpm-lock.yaml" | "shard.ix" | "shard.ix.tmp"
    ) {
        return false;
    }

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if matches!(
        ext,
        "so" | "o"
            | "dylib"
            | "a"
            | "dll"
            | "exe"
            | "pyc"
            | "jpg"
            | "png"
            | "gif"
            | "mp4"
            | "mp3"
            | "pdf"
            | "7z"
            | "rar"
            | "sqlite"
            | "db"
            | "bin"
    ) {
        return false;
    }

    if !include_archives && is_archive {
        return false;
    }

    true
}

fn scan_single_path(
    path: &Path,
    regex: &Regex,
    options: &QueryOptions,
    matches_found: &AtomicU32,
    accum: &ScanStatsAccum,
) -> Option<Vec<Match>> {
    if options.max_results > 0
        && matches_found.load(Ordering::Relaxed)
            >= u32::try_from(options.max_results).unwrap_or(u32::MAX)
    {
        return None;
    }

    let is_archive = options.archive && is_archive_path(path);

    // Filter regular files by extension; archive entries filter inside archive.rs.
    if !is_archive && !options.type_filter.is_empty() {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !options.type_filter.iter().any(|e| e == ext) {
            return None;
        }
    }

    if is_archive {
        #[cfg(feature = "archive")]
        {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let archive_res = if ext == "zip" {
                crate::archive::scan_zip_with_stats(path, regex, options)
            } else {
                crate::archive::scan_tar_gz_with_stats(path, regex, options)
            };
            return match archive_res {
                Ok((archive_matches, skipped_entries)) => {
                    if skipped_entries > 0 {
                        accum
                            .archive_entries_skipped
                            .fetch_add(skipped_entries, Ordering::Relaxed);
                    }
                    matches_found.fetch_add(
                        u32::try_from(archive_matches.len()).unwrap_or(u32::MAX),
                        Ordering::Relaxed,
                    );
                    Some(archive_matches)
                }
                Err(e) => {
                    accum.files_skipped_io.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!("scanner: cannot read archive {}: {e}", path.display());
                    None
                }
            };
        }
    }

    let file_matches = match Scanner::scan_file(path, regex, options) {
        Ok(m) => m,
        Err(e) => {
            accum.files_skipped_io.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("scanner: cannot read {}: {e}", path.display());
            return None;
        }
    };
    matches_found.fetch_add(
        u32::try_from(file_matches.len()).unwrap_or(u32::MAX),
        Ordering::Relaxed,
    );
    Some(file_matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::QueryOptions;
    use std::fmt::Write as _;
    use std::fs;
    use tempfile::tempdir;

    /// Helper: create a temp dir, write named files, return the tempdir handle.
    fn setup_temp_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        for (name, content) in files {
            fs::write(dir.path().join(name), content).unwrap();
        }
        dir
    }

    #[test]
    fn scanner_new_creates_with_valid_root() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "needle in haystack").unwrap();

        let scanner = Scanner::new(dir.path());
        let matches = scanner
            .scan("needle", false, false, &QueryOptions::default())
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].file_path.ends_with("a.txt"));
        assert_eq!(matches[0].line_number, 1);
    }

    #[test]
    fn scanner_scan_finds_matches_across_files() {
        let dir = setup_temp_dir(&[
            (
                "one.txt",
                "line 1: needle here\nline 2: nothing\nline 3: needle again",
            ),
            ("two.txt", "no pattern here\njust filler"),
            ("three.txt", "solo needle line"),
        ]);

        let scanner = Scanner::new(dir.path());
        let matches = scanner
            .scan("needle", false, false, &QueryOptions::default())
            .unwrap();

        // two files should match (one.txt, three.txt); two.txt has no needle
        assert_eq!(matches.len(), 3);
        let matched_files: std::collections::BTreeSet<&str> = matches
            .iter()
            .map(|m| m.file_path.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(
            matched_files,
            ["one.txt", "three.txt"]
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
        );
    }

    #[test]
    fn scanner_scan_respects_type_filter() {
        let dir = setup_temp_dir(&[
            ("code.rs", "// TODO: implement scanner\nfn main() {}"),
            ("notes.md", "# Notes\n\nTODO: write docs\n\nDone."),
            ("readme.txt", "TODO: update readme"),
        ]);

        let scanner = Scanner::new(dir.path());

        // Filter to only .rs files
        let opts = QueryOptions {
            type_filter: vec!["rs".to_string()],
            ..QueryOptions::default()
        };
        let matches = scanner.scan("TODO", false, false, &opts).unwrap();
        let matched_files: std::collections::BTreeSet<&str> = matches
            .iter()
            .map(|m| m.file_path.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(
            matched_files,
            ["code.rs"]
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
        );
        assert_eq!(matches.len(), 1);

        // Filter to only .md files
        let opts = QueryOptions {
            type_filter: vec!["md".to_string()],
            ..QueryOptions::default()
        };
        let matches = scanner.scan("TODO", false, false, &opts).unwrap();
        let matched_files: std::collections::BTreeSet<&str> = matches
            .iter()
            .map(|m| m.file_path.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(
            matched_files,
            ["notes.md"]
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
        );
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn scanner_scan_returns_empty_when_no_matches() {
        let dir = setup_temp_dir(&[
            ("a.txt", "hello world\nthis is content"),
            ("b.txt", "more stuff here\nnothing interesting"),
        ]);

        let scanner = Scanner::new(dir.path());
        let matches = scanner
            .scan("nonexistent", false, false, &QueryOptions::default())
            .unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn scanner_scan_respects_max_results() {
        let dir = tempdir().unwrap();
        let content = (1..=10)
            .map(|i| format!("needle match {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(dir.path().join("many.txt"), &content).unwrap();

        let scanner = Scanner::new(dir.path());
        let opts = QueryOptions {
            max_results: 3,
            ..QueryOptions::default()
        };
        let matches = scanner.scan("needle", false, false, &opts).unwrap();

        assert_eq!(matches.len(), 3);
        for m in &matches {
            assert!(m.file_path.ends_with("many.txt"));
            assert!(m.line_content.contains("needle"));
        }
    }

    #[test]
    fn scanner_scan_nonexistent_root_returns_error() {
        let root = std::path::PathBuf::from("/tmp/ix_nonexistent_root_test_xyz");
        // Ensure the path doesn't exist
        if root.exists() {
            let _ = std::fs::remove_dir_all(&root);
        }

        let scanner = Scanner::new(&root);
        let result = scanner.scan("anything", false, false, &QueryOptions::default());

        assert!(result.is_err());
        match result {
            Err(crate::error::Error::Io(e)) => {
                assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected Io(NotFound) error, got {other:?}"),
        }
    }

    // ── Rule 5: Integer Boundary Tests ─────────────────────────────────

    /// File with many lines: `line_numbers` must be sequential and never wrap to 0.
    #[test]
    fn test_line_number_no_wrap() {
        let dir = tempdir().unwrap();
        // Generate a file with enough lines to exercise the u32 counter
        // Use ~1000 lines (well within u32 range but exercises the counter)
        let mut content = String::new();
        for i in 1..=1000 {
            let _ = writeln!(content, "line {i} with needle here");
        }
        // Add a few extra non-matching lines at the end
        content.push_str("no match here\nno match here\n");

        fs::write(dir.path().join("many_lines.txt"), &content).unwrap();

        let scanner = Scanner::new(dir.path());
        let matches = scanner
            .scan("needle", false, false, &QueryOptions::default())
            .unwrap();

        // Every match should have line_number > 0 and ≤ 1000
        for m in &matches {
            assert!(
                m.line_number > 0,
                "line_number should never be 0 (found at line {})",
                m.line_number
            );
            assert!(
                m.line_number <= 1000,
                "line_number {} exceeds expected max 1000",
                m.line_number
            );
        }

        // Line numbers should be sequential (no gaps or wrap-around)
        let line_numbers: Vec<u32> = matches.iter().map(|m| m.line_number).collect();
        let mut unique_sorted: Vec<u32> = line_numbers.clone();
        unique_sorted.sort_unstable();
        unique_sorted.dedup();
        assert_eq!(
            line_numbers, unique_sorted,
            "line numbers should be unique and ordered"
        );
    }

    /// `max_results` boundary values: test 0, 1, large values.
    #[test]
    fn test_max_results_boundary() {
        let dir = tempdir().unwrap();
        let content = (1..=20)
            .map(|i| format!("needle {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(dir.path().join("matches.txt"), &content).unwrap();
        let scanner = Scanner::new(dir.path());

        // max_results = 0: unlimited (all 20 lines)
        let opts = QueryOptions {
            max_results: 0,
            ..QueryOptions::default()
        };
        let matches = scanner.scan("needle", false, false, &opts).unwrap();
        assert_eq!(matches.len(), 20, "max_results=0 should return all matches");

        // max_results = 1: exactly 1 result
        let opts = QueryOptions {
            max_results: 1,
            ..QueryOptions::default()
        };
        let matches = scanner.scan("needle", false, false, &opts).unwrap();
        assert_eq!(matches.len(), 1, "max_results=1 should return exactly 1");

        // max_results = large value (usize::MAX would be absurd, test with a known bound)
        let opts = QueryOptions {
            max_results: 10_000,
            ..QueryOptions::default()
        };
        let matches = scanner.scan("needle", false, false, &opts).unwrap();
        assert_eq!(
            matches.len(),
            20,
            "large max_results should return all matches"
        );
    }

    // ── Rule 6: Oversized Input Tests ─────────────────────────────────

    /// Scanner must handle files with very long lines (no newline)
    /// without panicking or OOM. Uses 1MB line for test speed.
    #[test]
    fn test_scanner_long_line_no_newline() {
        let dir = tempdir().unwrap();
        // Create a 100KB line (reasonable for unit test, exercises unbounded buffer)
        let long_line = "A".repeat(100_000);
        let content = format!("start of file\n{long_line}end of file\nnormal line\n");
        fs::write(dir.path().join("long.txt"), &content).unwrap();

        let scanner = Scanner::new(dir.path());
        // Search for a pattern that won't match in the long line
        let result = scanner.scan("normal", false, false, &QueryOptions::default());
        assert!(
            result.is_ok(),
            "scanner should handle long lines without error"
        );
        let matches = result.unwrap();
        // Should find "normal" in the last line
        assert!(!matches.is_empty(), "should find the normal line");
    }

    /// Scanner must handle files with lines near the size limit
    /// without panicking. The scanner skips files >10MB, so 1MB line is fine.
    #[test]
    fn test_scanner_moderate_long_line_matches() {
        let dir = tempdir().unwrap();
        // Line with a match right at the end of a long line
        let prefix = "x".repeat(50_000);
        let content = format!("{prefix}needle_found\nshort line\n");
        fs::write(dir.path().join("long_match.txt"), &content).unwrap();

        let scanner = Scanner::new(dir.path());
        let matches = scanner
            .scan("needle_found", false, false, &QueryOptions::default())
            .unwrap();
        assert_eq!(matches.len(), 1, "should find match in long line");
        assert!(
            matches[0].line_content.contains("needle_found"),
            "match line content should contain pattern"
        );
    }

    #[test]
    fn scanner_multiline_and_non_utf8_and_word_regex() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("multi.txt"), "alpha\nbeta\n").unwrap();

        let mut non_utf8 = vec![b'a'; 9000];
        non_utf8.push(b'\n');
        non_utf8.extend_from_slice(b"needle_\xFF_ok\n");
        fs::write(dir.path().join("latin1.txt"), &non_utf8).unwrap();

        let scanner = Scanner::new(dir.path());
        let multi = scanner
            .scan(
                r"alpha.*beta",
                true,
                false,
                &QueryOptions {
                    multiline: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(multi.len(), 1);

        let lossy = scanner
            .scan("needle_", false, false, &QueryOptions::default())
            .unwrap();
        assert_eq!(lossy.len(), 1);

        fs::write(dir.path().join("words.txt"), "foobar\nfoo\nbar\n").unwrap();
        let word_re = scanner
            .scan(
                "foo|bar",
                true,
                false,
                &QueryOptions {
                    word_boundary: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(word_re.len(), 2);
    }

    #[test]
    fn scanner_files_only_max_results_caps_distinct_files() {
        let dir = setup_temp_dir(&[
            ("a.txt", "needle 1\nneedle 2\nneedle 3\n"),
            ("b.txt", "needle 4\nneedle 5\n"),
            ("c.txt", "needle 6\n"),
        ]);
        let scanner = Scanner::new(dir.path());
        let matches = scanner
            .scan(
                "needle",
                false,
                false,
                &QueryOptions {
                    files_only: true,
                    max_results: 2,
                    ..Default::default()
                },
            )
            .unwrap();
        let distinct: std::collections::HashSet<_> =
            matches.iter().map(|m| m.file_path.clone()).collect();
        assert_eq!(distinct.len(), 2);
    }

    #[cfg(feature = "archive")]
    #[test]
    fn scanner_admits_archives_and_does_not_fall_through_on_corrupt_archive() {
        use std::io::Write;
        use zip::ZipWriter;
        use zip::write::SimpleFileOptions;

        let dir = tempdir().unwrap();
        let zip_path = dir.path().join("bundle.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = ZipWriter::new(f);
            zw.start_file("src/lib.rs", SimpleFileOptions::default())
                .unwrap();
            zw.write_all(b"fn needle_in_zip() {}\n").unwrap();
            zw.start_file("notes.txt", SimpleFileOptions::default())
                .unwrap();
            zw.write_all(b"needle_in_zip\n").unwrap();
            zw.finish().unwrap();
        }
        // Corrupt .zip that literally contains "needle_in_zip" in raw bytes.
        fs::write(
            dir.path().join("corrupt.zip"),
            b"not a valid zip but contains needle_in_zip",
        )
        .unwrap();

        let scanner = Scanner::new(dir.path());
        let matches = scanner
            .scan(
                "needle_in_zip",
                false,
                false,
                &QueryOptions {
                    archive: true,
                    binary: true,
                    type_filter: vec!["rs".to_string()],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].file_path.to_string_lossy().contains("lib.rs"));
    }
}
