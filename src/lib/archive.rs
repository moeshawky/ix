//! Archive searching support (.zip, .tar.gz).

#[cfg(feature = "archive")]
use crate::error::Result;
#[cfg(feature = "archive")]
use crate::executor::{Match, QueryOptions};
#[cfg(feature = "archive")]
use crate::format::is_binary;
#[cfg(feature = "archive")]
use regex::Regex;
#[cfg(feature = "archive")]
use std::fs::File;
#[cfg(feature = "archive")]
use std::io::{BufRead, BufReader, Read};
#[cfg(feature = "archive")]
use std::path::{Path, PathBuf};

/// Strip path-traversal components from an archive entry name.
///
/// Removes leading `/` and `\` prefixes, then drops any `..` component.
/// This prevents path traversal if downstream code ever uses `Match.file_path`
/// for file operations (currently display-only but load-bearing invariant).
#[cfg(feature = "archive")]
fn sanitize_archive_path(name: &str) -> String {
    let trimmed = name.trim_start_matches('/').trim_start_matches('\\');
    trimmed
        .split(['/', '\\'])
        .filter(|c| *c != "..")
        .collect::<Vec<_>>()
        .join("/")
}

/// Scan a .zip archive for matches.
///
/// # Errors
///
/// Returns an error if the archive cannot be read or entries are malformed.
#[cfg(feature = "archive")]
pub fn scan_zip(path: &Path, regex: &Regex, options: &QueryOptions) -> Result<Vec<Match>> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut matches = Vec::new();

    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        if !entry.is_file() {
            continue;
        }

        let entry_name = sanitize_archive_path(entry.name());
        let display_path = format!("{}:{}", path.display(), entry_name);
        let entry_matches =
            match_content_stream(entry, &PathBuf::from(display_path), regex, options)?;

        for m in entry_matches {
            matches.push(m);
            if options.max_results > 0 && matches.len() >= options.max_results {
                return Ok(matches);
            }
        }
    }

    Ok(matches)
}

/// Scan a .tar.gz archive for matches.
///
/// # Errors
///
/// Returns an error if the archive cannot be read or entries are malformed.
#[cfg(feature = "archive")]
pub fn scan_tar_gz(path: &Path, regex: &Regex, options: &QueryOptions) -> Result<Vec<Match>> {
    let file = File::open(path)?;
    let tar_gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(tar_gz);
    let mut matches = Vec::new();

    for entry in archive.entries()? {
        let entry = entry?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let path_in_tar = sanitize_archive_path(&entry.path()?.to_string_lossy());
        let display_path = format!("{}:{}", path.display(), path_in_tar);

        let entry_matches =
            match_content_stream(entry, &PathBuf::from(display_path), regex, options)?;

        for m in entry_matches {
            matches.push(m);
            if options.max_results > 0 && matches.len() >= options.max_results {
                return Ok(matches);
            }
        }
    }

    Ok(matches)
}

#[cfg(feature = "archive")]
fn match_content_stream<R: Read>(
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
        if is_binary(buffer) {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "archive"))]
#[allow(
    clippy::as_conversions,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::expect_used
)]
mod tests {
    use super::*;
    use regex::Regex;
    use std::io::Write;
    use tempfile::tempdir;

    // Helper: write bytes to a named file in a temp directory and return the path.
    fn write_temp(dir: &std::path::Path, name: &str, data: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, data).expect("write temp file");
        path
    }

    // ----------------------------------------------------------------
    // zip helpers
    // ----------------------------------------------------------------

    /// Create a .zip file containing one text entry.
    fn make_zip(path: &std::path::Path, entry_name: &str, content: &[u8]) {
        use zip::ZipWriter;
        use zip::write::SimpleFileOptions;

        let file = std::fs::File::create(path).expect("create zip");
        let mut zip = ZipWriter::new(file);
        zip.start_file(entry_name, SimpleFileOptions::default())
            .expect("start_file");
        zip.write_all(content).expect("write_all");
        zip.finish().expect("finish zip");
    }

    /// Create an empty .zip (no entries).
    fn make_empty_zip(path: &std::path::Path) {
        let file = std::fs::File::create(path).expect("create zip");
        let mut zip = zip::ZipWriter::new(file);
        // Add a directory entry so the archive has at least one entry,
        // then `scan_zip` will skip it (entry.is_file() is false) —
        // resulting in zero matches.
        zip.add_directory("empty_dir", zip::write::SimpleFileOptions::default())
            .expect("add_directory");
        zip.finish().expect("finish empty zip");
    }

    // ----------------------------------------------------------------
    // tar.gz helpers
    // ----------------------------------------------------------------

    /// Create a .tar.gz file containing one text entry.
    fn make_tar_gz(path: &std::path::Path, entry_name: &str, content: &[u8]) {
        use flate2::{Compression, write::GzEncoder};
        use tar::{Builder, Header};

        let file = std::fs::File::create(path).expect("create tar.gz");
        let enc = GzEncoder::new(file, Compression::default());
        let mut tar = Builder::new(enc);

        let mut header = Header::new_gnu();
        header.set_size(content.len().try_into().expect("size fits u64"));
        header.set_path(entry_name).expect("set_path");
        header.set_cksum();
        tar.append(&header, content).expect("append");
        let enc = tar.into_inner().expect("into_inner");
        enc.finish().expect("finish gz");
    }

    /// Create an empty .tar.gz (no entries at all).
    fn make_empty_tar_gz(path: &std::path::Path) {
        use flate2::{Compression, write::GzEncoder};
        use tar::Builder;

        let file = std::fs::File::create(path).expect("create tar.gz");
        let enc = GzEncoder::new(file, Compression::default());
        let tar = Builder::new(enc);
        // Finish without adding any entries — produces a valid empty archive.
        let enc = tar.into_inner().expect("into_inner");
        enc.finish().expect("finish gz");
    }

    // ----------------------------------------------------------------
    // scan_zip tests
    // ----------------------------------------------------------------

    #[test]
    fn scan_zip_single_match() {
        let dir = tempdir().unwrap();
        let zip_path = write_temp(dir.path(), "test.zip", &[]);
        make_zip(&zip_path, "hello.txt", b"this has needle inside\n");

        let re = Regex::new("needle").unwrap();
        let opts = QueryOptions::default();
        let results = scan_zip(&zip_path, &re, &opts).unwrap();

        assert_eq!(results.len(), 1);
        let m = &results[0];
        assert!(m.file_path.to_string_lossy().contains("hello.txt"));
        assert_eq!(m.line_number, 1);
        assert!(m.line_content.contains("needle"));
        assert!(!m.is_binary);
    }

    #[test]
    fn scan_zip_no_match() {
        let dir = tempdir().unwrap();
        let zip_path = write_temp(dir.path(), "test.zip", &[]);
        make_zip(&zip_path, "data.txt", b"nothing to see here\n");

        let re = Regex::new("absent").unwrap();
        let opts = QueryOptions::default();
        let results = scan_zip(&zip_path, &re, &opts).unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn scan_zip_multiple_matches() {
        let dir = tempdir().unwrap();
        let zip_path = write_temp(dir.path(), "test.zip", &[]);
        make_zip(
            &zip_path,
            "lines.txt",
            b"line one needle here\nline two also needle\n",
        );

        let re = Regex::new("needle").unwrap();
        let opts = QueryOptions::default();
        let results = scan_zip(&zip_path, &re, &opts).unwrap();

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn scan_zip_regex() {
        let dir = tempdir().unwrap();
        let zip_path = write_temp(dir.path(), "test.zip", &[]);
        make_zip(&zip_path, "code.py", b"def foo():\n    return 42\n");

        let re = Regex::new(r"def\s+\w+").unwrap();
        let opts = QueryOptions::default();
        let results = scan_zip(&zip_path, &re, &opts).unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].line_content.contains("def foo"));
    }

    #[test]
    fn scan_zip_max_results() {
        let dir = tempdir().unwrap();
        let zip_path = write_temp(dir.path(), "test.zip", &[]);
        make_zip(
            &zip_path,
            "many.txt",
            b"a\nb needle\nc needle\nd needle\ne\n",
        );

        let re = Regex::new("needle").unwrap();
        let opts = QueryOptions {
            max_results: 2,
            ..Default::default()
        };
        let results = scan_zip(&zip_path, &re, &opts).unwrap();

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn scan_zip_empty_archive() {
        let dir = tempdir().unwrap();
        let zip_path = write_temp(dir.path(), "empty.zip", &[]);
        make_empty_zip(&zip_path);

        let re = Regex::new("anything").unwrap();
        let opts = QueryOptions::default();
        let results = scan_zip(&zip_path, &re, &opts).unwrap();

        assert!(results.is_empty());
    }

    // ----------------------------------------------------------------
    // scan_tar_gz tests
    // ----------------------------------------------------------------

    #[test]
    fn scan_tar_gz_single_match() {
        let dir = tempdir().unwrap();
        let tgz_path = write_temp(dir.path(), "test.tar.gz", &[]);
        make_tar_gz(&tgz_path, "notes.txt", b"found: needle\n");

        let re = Regex::new("needle").unwrap();
        let opts = QueryOptions::default();
        let results = scan_tar_gz(&tgz_path, &re, &opts).unwrap();

        assert_eq!(results.len(), 1);
        let m = &results[0];
        assert!(m.file_path.to_string_lossy().contains("notes.txt"));
        assert!(m.line_content.contains("needle"));
    }

    #[test]
    fn scan_tar_gz_no_match() {
        let dir = tempdir().unwrap();
        let tgz_path = write_temp(dir.path(), "test.tar.gz", &[]);
        make_tar_gz(&tgz_path, "log.txt", b"all clear\n");

        let re = Regex::new("missing").unwrap();
        let opts = QueryOptions::default();
        let results = scan_tar_gz(&tgz_path, &re, &opts).unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn scan_tar_gz_multiple_entries() {
        let dir = tempdir().unwrap();
        let tgz_path = write_temp(dir.path(), "multi.tar.gz", &[]);

        // Build a tar.gz with two text entries.
        {
            use flate2::{Compression, write::GzEncoder};
            use tar::{Builder, Header};

            let file = std::fs::File::create(&tgz_path).expect("create");
            let enc = GzEncoder::new(file, Compression::default());
            let mut tar = Builder::new(enc);

            for (name, body) in [
                ("a.txt", &b"apple needle\n"[..]),
                ("b.txt", &b"banana needle\n"[..]),
            ] {
                let mut header = Header::new_gnu();
                header.set_size(u64::try_from(body.len()).expect("size"));
                header.set_path(name).expect("set_path");
                header.set_cksum();
                tar.append(&header, body).expect("append");
            }
            let enc = tar.into_inner().expect("into_inner");
            enc.finish().expect("finish");
        }

        let re = Regex::new("needle").unwrap();
        let opts = QueryOptions::default();
        let results = scan_tar_gz(&tgz_path, &re, &opts).unwrap();

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn scan_tar_gz_regex() {
        let dir = tempdir().unwrap();
        let tgz_path = write_temp(dir.path(), "test.tar.gz", &[]);
        make_tar_gz(
            &tgz_path,
            "f.rs",
            b"fn main() {\n    println!(\"hi\");\n}\n",
        );

        let re = Regex::new(r"fn\s+\w+").unwrap();
        let opts = QueryOptions::default();
        let results = scan_tar_gz(&tgz_path, &re, &opts).unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].line_content.contains("fn main"));
    }

    #[test]
    fn scan_tar_gz_empty() {
        let dir = tempdir().unwrap();
        let tgz_path = write_temp(dir.path(), "empty.tar.gz", &[]);
        make_empty_tar_gz(&tgz_path);

        let re = Regex::new("anything").unwrap();
        let opts = QueryOptions::default();
        let results = scan_tar_gz(&tgz_path, &re, &opts).unwrap();

        assert!(results.is_empty());
    }

    // ----------------------------------------------------------------
    // context_lines tests (exercises the context plumbing)
    // ----------------------------------------------------------------

    #[test]
    fn scan_zip_with_context() {
        let dir = tempdir().unwrap();
        let zip_path = write_temp(dir.path(), "test.zip", &[]);
        make_zip(&zip_path, "ctx.txt", b"before\nneedle here\nafter\n");

        let re = Regex::new("needle").unwrap();
        let opts = QueryOptions {
            context_lines: 1,
            ..Default::default()
        };
        let results = scan_zip(&zip_path, &re, &opts).unwrap();

        assert_eq!(results.len(), 1);
        let m = &results[0];
        assert_eq!(m.context_before.len(), 1);
        assert_eq!(m.context_after.len(), 1);
        assert_eq!(m.context_before[0], "before");
        assert_eq!(m.context_after[0], "after");
    }

    #[test]
    fn scan_tar_gz_with_context() {
        let dir = tempdir().unwrap();
        let tgz_path = write_temp(dir.path(), "test.tar.gz", &[]);
        make_tar_gz(&tgz_path, "ctx.txt", b"line 1\nline 2 target\nline 3\n");

        let re = Regex::new("target").unwrap();
        let opts = QueryOptions {
            context_lines: 2,
            ..Default::default()
        };
        let results = scan_tar_gz(&tgz_path, &re, &opts).unwrap();

        assert_eq!(results.len(), 1);
        let m = &results[0];
        assert_eq!(m.context_before.len(), 1); // only 1 line before target
        assert_eq!(m.context_after.len(), 1); // only 1 line after target
    }
}
