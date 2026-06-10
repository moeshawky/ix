//! Fallback scanner (no index, competitive with ripgrep).
//!
//! Used when .ix index is missing or explicitly disabled.

use crate::decompress::maybe_decompress;
use crate::error::Result;
use crate::executor::{Match, QueryOptions};
use crate::format::is_binary;
use ignore::WalkBuilder;
use memmap2::Mmap;
use rayon::prelude::*;
use regex::Regex;
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

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
    #[allow(clippy::too_many_lines)]
    pub fn scan(
        &self,
        pattern: &str,
        is_regex: bool,
        ignore_case: bool,
        options: &QueryOptions,
    ) -> Result<Vec<Match>> {
        // Early-exit: surface a clear error when the root directory does not
        // exist, rather than silently returning an empty result set from
        // walk-iterator errors.
        if !self.root.exists() {
            return Err(crate::error::Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("scanner root does not exist: {}", self.root.display()),
            )));
        }

        let raw = if is_regex {
            pattern.to_string()
        } else {
            regex::escape(pattern)
        };

        // Apply word boundary wrapping for literal patterns (same as planner.rs)
        let with_word_boundaries = if options.word_boundary && !is_regex {
            format!("\\b{raw}\\b")
        } else {
            raw
        };

        // Build regex pattern with flags
        let mut regex_pat = String::new();
        if ignore_case {
            regex_pat.push_str("(?i)");
        }
        if options.multiline {
            regex_pat.push_str("(?s)");
        }
        regex_pat.push_str(&with_word_boundaries);

        let regex = Regex::new(&regex_pat)?;

        let walker = WalkBuilder::new(&self.root)
            .hidden(false)
            .git_ignore(true)
            .require_git(false)
            .add_custom_ignore_filename(".ixignore")
            .filter_entry(move |entry| {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                // Built-in directory defaults
                if entry.file_type().is_some_and(|t| t.is_dir())
                    && (name == "lost+found"
                        || name == ".git"
                        || name == "node_modules"
                        || name == "target"
                        || name == "__pycache__"
                        || name == ".tox"
                        || name == ".venv"
                        || name == "venv"
                        || name == ".ix")
                {
                    return false;
                }

                // Built-in file noise defaults
                if entry.file_type().is_some_and(|t| t.is_file()) {
                    if let Ok(metadata) = entry.metadata()
                        && metadata.len() > 10 * 1024 * 1024
                    {
                        return false;
                    }
                    if name == "Cargo.lock"
                        || name == "package-lock.json"
                        || name == "pnpm-lock.yaml"
                        || name == "shard.ix"
                        || name == "shard.ix.tmp"
                    {
                        return false;
                    }
                }

                // Built-in file extension defaults
                if entry.file_type().is_some_and(|t| t.is_file()) {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    match ext {
                        // Binary extensions
                        "so" | "o" | "dylib" | "a" | "dll" | "exe" | "pyc" |
                        // Media
                        "jpg" | "png" | "gif" | "mp4" | "mp3" | "pdf" |
                        // Archives
                        "zip" | "7z" | "rar" |
                        // Data
                        "sqlite" | "db" | "bin" => return false,
                        _ => {}
                    }
                    if name.ends_with(".tar.gz") {
                        return false;
                    }
                }
                true
            })
            .build();

        let paths: Vec<PathBuf> = walker
            .filter_map(|result| match result {
                Ok(entry) => Some(entry),
                Err(e) => {
                    eprintln!("ix: warning: scanner skipping path: {e}");
                    None
                }
            })
            .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
            .map(|entry| entry.path().to_owned())
            .collect();

        let matches_found = AtomicU32::new(0);
        let mut matches: Vec<Match> = paths
            .into_par_iter()
            .filter_map(|path| {
                if options.max_results > 0
                    && matches_found.load(Ordering::Relaxed)
                        >= u32::try_from(options.max_results).unwrap_or(0)
                {
                    return None;
                }

                // Filter by extension
                if !options.type_filter.is_empty() {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if !options.type_filter.iter().any(|e: &String| e == ext) {
                        return None;
                    }
                }

                // Archive support
                if options.archive {
                    #[cfg(feature = "archive")]
                    {
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                        let is_tar_gz = path.to_str().is_some_and(|s| s.ends_with(".tar.gz"));
                        if ext == "zip"
                            && let Ok(archive_matches) =
                                crate::archive::scan_zip(&path, &regex, options)
                        {
                            matches_found.fetch_add(
                                u32::try_from(archive_matches.len()).unwrap_or(0),
                                Ordering::Relaxed,
                            );
                            return Some(archive_matches);
                        }
                        if is_tar_gz
                            && let Ok(archive_matches) =
                                crate::archive::scan_tar_gz(&path, &regex, options)
                        {
                            matches_found.fetch_add(
                                u32::try_from(archive_matches.len()).unwrap_or(0),
                                Ordering::Relaxed,
                            );
                            return Some(archive_matches);
                        }
                    }
                }

                let file_matches = match Self::scan_file(&path, &regex, options) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!("scanner: cannot read {}: {e}", path.display());
                        return None;
                    }
                };
                matches_found.fetch_add(
                    u32::try_from(file_matches.len()).unwrap_or(0),
                    Ordering::Relaxed,
                );
                Some(file_matches)
            })
            .flatten()
            .collect();

        if options.max_results > 0 && matches.len() > options.max_results {
            matches.truncate(options.max_results);
        }

        Ok(matches)
    }

    #[allow(clippy::too_many_lines)]
    fn scan_stream<R: Read>(
        reader: R,
        path: &Path,
        regex: &Regex,
        options: &QueryOptions,
    ) -> Result<Vec<Match>> {
        let mut buf_reader = BufReader::new(reader);
        let mut matches = Vec::new();
        let mut line_number = 0u32;
        let mut byte_offset = 0u64;

        // Binary check on first 8KB
        {
            let buffer = buf_reader.fill_buf()?;
            if buffer.is_empty() {
                return Ok(vec![]);
            }
            let is_bin = is_binary(buffer);
            if is_bin && !options.binary {
                return Ok(vec![]);
            }
        }

        let mut line = String::new();
        let mut context_before = std::collections::VecDeque::new();
        let mut pending_matches: Vec<Match> = Vec::new();

        while buf_reader.read_line(&mut line)? > 0 {
            line_number += 1;
            let line_len = u64::try_from(line.len()).unwrap_or(0);
            let trimmed_line_str = line.trim_end();

            // Fill context_after for pending matches
            for m in &mut pending_matches {
                if m.context_after.len() < options.context_lines {
                    m.context_after.push(trimmed_line_str.to_string());
                }
            }

            // Move completed matches to final list
            let (completed, still_pending): (Vec<_>, Vec<_>) = pending_matches
                .into_iter()
                .partition(|m| m.context_after.len() >= options.context_lines);
            matches.extend(completed);
            pending_matches = still_pending;

            if let Some(m) = regex.find(&line) {
                let context_before_vec: Vec<String> = context_before.iter().cloned().collect();

                let new_match = Match {
                    file_path: path.to_owned(),
                    line_number,
                    col: u32::try_from(m.start() + 1).unwrap_or(0),
                    line_content: if options.count_only {
                        String::new()
                    } else {
                        trimmed_line_str.to_string()
                    },
                    byte_offset: byte_offset + u64::try_from(m.start()).unwrap_or(0),
                    context_before: context_before_vec,
                    context_after: vec![],
                    is_binary: false,
                };

                if options.context_lines > 0 {
                    pending_matches.push(new_match);
                } else {
                    matches.push(new_match);
                }

                if options.max_results > 0
                    && (matches.len() + pending_matches.len()) >= options.max_results
                    && (pending_matches.is_empty() || matches.len() >= options.max_results)
                {
                    break;
                }
            }

            if options.context_lines > 0 {
                if context_before.len() == options.context_lines {
                    if let Some(mut old_line) = context_before.pop_front() {
                        old_line.clear();
                        old_line.push_str(trimmed_line_str);
                        context_before.push_back(old_line);
                    }
                } else {
                    context_before.push_back(trimmed_line_str.to_string());
                }
            }

            byte_offset += line_len;
            line.clear();
        }

        matches.extend(pending_matches);
        Ok(matches)
    }

    fn scan_file(path: &Path, regex: &Regex, options: &QueryOptions) -> Result<Vec<Match>> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        if metadata.len() > 100 * 1024 * 1024 && !options.decompress {
            // Keep 100MB limit for raw files to avoid huge mmaps in parallel
            return Ok(vec![]);
        }

        // SAFETY: The file is opened read-only and the mmap is used only
        // within this function via a Cursor, which treats the mapped memory
        // as an immutable byte slice. No concurrent modification to the
        // underlying file is expected during scanning.
        let mmap = unsafe { Mmap::map(&file)? };

        if options.decompress
            && let Some(reader) = maybe_decompress(path, &mmap)?
        {
            return Self::scan_stream(reader, path, regex, options);
        }

        // Default to streaming via Cursor for uncompressed files to ensure constant memory (R-02)
        Self::scan_stream(Cursor::new(&mmap[..]), path, regex, options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::QueryOptions;
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
        let mut opts = QueryOptions::default();
        opts.type_filter = vec!["rs".to_string()];
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
        let mut opts = QueryOptions::default();
        opts.type_filter = vec!["md".to_string()];
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
        let mut opts = QueryOptions::default();
        opts.max_results = 3;
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
}
