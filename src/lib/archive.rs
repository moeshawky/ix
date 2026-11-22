//! Archive searching support (.zip, .tar.gz).

#[cfg(feature = "archive")]
use crate::error::Result;
#[cfg(feature = "archive")]
use crate::executor::{Match, QueryOptions};
#[cfg(feature = "archive")]
use crate::streaming::{StreamLimits, StreamStats, stream_file_bounded};
#[cfg(feature = "archive")]
use regex::Regex;
#[cfg(feature = "archive")]
use std::fs::File;
#[cfg(feature = "archive")]
use std::io::Read;
#[cfg(feature = "archive")]
use std::path::{Path, PathBuf};
#[cfg(feature = "archive")]
use std::sync::Arc;
#[cfg(feature = "archive")]
use std::sync::atomic::{AtomicU64, Ordering};

/// Maximum decompressed bytes per individual archive entry (`100 MiB`).
#[cfg(feature = "archive")]
const MAX_ENTRY_BYTES: usize = 100 * 1024 * 1024;
/// Maximum line length within an archive entry (`1 MiB`).
#[cfg(feature = "archive")]
const MAX_LINE_BYTES: usize = 1024 * 1024;
/// Maximum total decompressed bytes across an entire archive (`512 MiB`).
#[cfg(feature = "archive")]
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum number of entries inspected per archive.
#[cfg(feature = "archive")]
const MAX_ARCHIVE_ENTRIES: usize = 100_000;

/// Reader wrapper that enforces a shared decompressed byte ceiling across an archive.
#[cfg(feature = "archive")]
struct BoundedArchiveReader<R> {
    inner: R,
    bytes_read: Arc<AtomicU64>,
    limit: u64,
}

#[cfg(feature = "archive")]
impl<R> BoundedArchiveReader<R> {
    const fn new(inner: R, bytes_read: Arc<AtomicU64>, limit: u64) -> Self {
        Self {
            inner,
            bytes_read,
            limit,
        }
    }
}

#[cfg(feature = "archive")]
impl<R: Read> Read for BoundedArchiveReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        let prev = self
            .bytes_read
            .fetch_add(u64::try_from(n).unwrap_or(u64::MAX), Ordering::Relaxed);
        let total = prev.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        if total > self.limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "archive decompressed data exceeds {} byte limit",
                    self.limit
                ),
            ));
        }
        Ok(n)
    }
}

/// Strip path-traversal components, leading separators, NUL bytes, and drive
/// prefixes from an archive entry name.
///
/// Prevents path traversal if downstream code ever uses `Match.file_path`
/// for file operations (currently display-only but load-bearing invariant).
#[cfg(feature = "archive")]
fn sanitize_archive_path(name: &str) -> String {
    let cleaned: String = name.chars().filter(|&c| c != '\0').collect();
    let without_drive = match cleaned.as_bytes() {
        [first, b':', ..] if first.is_ascii_alphabetic() => cleaned.get(2..).unwrap_or(""),
        _ => cleaned.as_str(),
    };
    let trimmed = without_drive.trim_start_matches(['/', '\\']);
    trimmed
        .split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != "." && *c != "..")
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(feature = "archive")]
fn entry_matches_type_filter(entry_name: &str, type_filter: &[String]) -> bool {
    if type_filter.is_empty() {
        return true;
    }
    let ext = Path::new(entry_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    type_filter.iter().any(|allowed| allowed == ext)
}

/// Scan a `.zip` archive for matches.
///
/// Enforces per-entry and archive-wide decompression caps and skips malformed
/// or oversized individual entries with a warning instead of aborting prior
/// matches.
///
/// # Errors
///
/// Returns an error if the archive file cannot be opened or its central
/// directory cannot be parsed.
#[cfg(feature = "archive")]
pub fn scan_zip(path: &Path, regex: &Regex, options: &QueryOptions) -> Result<Vec<Match>> {
    scan_zip_with_stats(path, regex, options).map(|(matches, _)| matches)
}
/// Scan a `.zip` archive for matches, returning both matches and the number of
/// skipped/malformed/capped entries.
///
/// # Errors
///
/// Returns an error if the archive file cannot be opened or its central
/// directory cannot be parsed.
#[cfg(feature = "archive")]
pub fn scan_zip_with_stats(
    path: &Path,
    regex: &Regex,
    options: &QueryOptions,
) -> Result<(Vec<Match>, u64)> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut matches = Vec::new();
    let mut skipped_entries = 0u64;
    let archive_bytes = Arc::new(AtomicU64::new(0));
    let total_entries = archive.len();
    if total_entries > MAX_ARCHIVE_ENTRIES {
        skipped_entries = skipped_entries.saturating_add(
            u64::try_from(total_entries.saturating_sub(MAX_ARCHIVE_ENTRIES)).unwrap_or(u64::MAX),
        );
        tracing::warn!(
            "archive {} has {} entries; capping inspection at {}",
            path.display(),
            total_entries,
            MAX_ARCHIVE_ENTRIES
        );
    }

    for i in 0..total_entries.min(MAX_ARCHIVE_ENTRIES) {
        if archive_bytes.load(Ordering::Relaxed) >= MAX_ARCHIVE_BYTES {
            let remaining = total_entries.min(MAX_ARCHIVE_ENTRIES).saturating_sub(i);
            skipped_entries =
                skipped_entries.saturating_add(u64::try_from(remaining).unwrap_or(u64::MAX));
            tracing::warn!(
                "archive {} exceeded {} byte decompressed limit; stopping",
                path.display(),
                MAX_ARCHIVE_BYTES
            );
            break;
        }

        let entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(e) => {
                skipped_entries = skipped_entries.saturating_add(1);
                tracing::warn!("archive: skipping entry #{i} in {}: {e}", path.display());
                continue;
            }
        };
        if !entry.is_file() {
            continue;
        }

        let entry_name = sanitize_archive_path(entry.name());
        if !entry_matches_type_filter(&entry_name, &options.type_filter) {
            continue;
        }
        let display_path = format!("{}:{}", path.display(), entry_name);
        let bounded =
            BoundedArchiveReader::new(entry, Arc::clone(&archive_bytes), MAX_ARCHIVE_BYTES);
        let entry_matches =
            match match_content_stream(bounded, &PathBuf::from(display_path), regex, options) {
                Ok(m) => m,
                Err(e) => {
                    skipped_entries = skipped_entries.saturating_add(1);
                    tracing::warn!(
                        "archive: skipping entry {} in {}: {e}",
                        entry_name,
                        path.display()
                    );
                    continue;
                }
            };

        for m in entry_matches {
            matches.push(m);
            if options.max_results > 0 && matches.len() >= options.max_results {
                return Ok((matches, skipped_entries));
            }
        }
    }

    Ok((matches, skipped_entries))
}

/// Scan a `.tar.gz` archive for matches.
///
/// Wraps the gzip stream in an archive-wide byte limit, caps inspected entries,
/// and warns and skips malformed or oversized individual entries.
///
/// # Errors
///
/// Returns an error if the archive file cannot be opened or the tar stream
/// headers cannot be read.
#[cfg(feature = "archive")]
pub fn scan_tar_gz(path: &Path, regex: &Regex, options: &QueryOptions) -> Result<Vec<Match>> {
    scan_tar_gz_with_stats(path, regex, options).map(|(matches, _)| matches)
}

/// Scan a `.tar.gz` archive for matches, returning both matches and the number
/// of skipped/malformed/capped entries.
///
/// # Errors
///
/// Returns an error if the archive file cannot be opened or the tar stream
/// headers cannot be read.
#[cfg(feature = "archive")]
pub fn scan_tar_gz_with_stats(
    path: &Path,
    regex: &Regex,
    options: &QueryOptions,
) -> Result<(Vec<Match>, u64)> {
    let file = File::open(path)?;
    let archive_bytes = Arc::new(AtomicU64::new(0));
    let tar_gz = BoundedArchiveReader::new(
        flate2::read::GzDecoder::new(file),
        Arc::clone(&archive_bytes),
        MAX_ARCHIVE_BYTES,
    );
    let mut archive = tar::Archive::new(tar_gz);
    let mut matches = Vec::new();
    let mut skipped_entries = 0u64;
    let mut entries_seen = 0usize;

    for entry_res in archive.entries()? {
        entries_seen = entries_seen.saturating_add(1);
        if entries_seen > MAX_ARCHIVE_ENTRIES {
            skipped_entries = skipped_entries.saturating_add(1);
            tracing::warn!(
                "archive {} exceeded {} entry limit; stopping",
                path.display(),
                MAX_ARCHIVE_ENTRIES
            );
            break;
        }
        let entry = match entry_res {
            Ok(e) => e,
            Err(e) => {
                skipped_entries = skipped_entries.saturating_add(1);
                tracing::warn!(
                    "archive: error reading tar entry in {}: {e}",
                    path.display()
                );
                if archive_bytes.load(Ordering::Relaxed) >= MAX_ARCHIVE_BYTES {
                    break;
                }
                continue;
            }
        };
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let raw_entry_path = match entry.path() {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => {
                skipped_entries = skipped_entries.saturating_add(1);
                tracing::warn!("archive: invalid tar path in {}: {e}", path.display());
                continue;
            }
        };
        let path_in_tar = sanitize_archive_path(&raw_entry_path);
        if !entry_matches_type_filter(&path_in_tar, &options.type_filter) {
            continue;
        }
        let display_path = format!("{}:{}", path.display(), path_in_tar);

        let entry_matches =
            match match_content_stream(entry, &PathBuf::from(display_path), regex, options) {
                Ok(m) => m,
                Err(e) => {
                    skipped_entries = skipped_entries.saturating_add(1);
                    tracing::warn!(
                        "archive: skipping entry {} in {}: {e}",
                        path_in_tar,
                        path.display()
                    );
                    if archive_bytes.load(Ordering::Relaxed) >= MAX_ARCHIVE_BYTES {
                        break;
                    }
                    continue;
                }
            };

        for m in entry_matches {
            matches.push(m);
            if options.max_results > 0 && matches.len() >= options.max_results {
                return Ok((matches, skipped_entries));
            }
        }
    }

    Ok((matches, skipped_entries))
}

#[cfg(feature = "archive")]
fn match_content_stream<R: Read>(
    reader: R,
    path: &Path,
    regex: &Regex,
    options: &QueryOptions,
) -> Result<Vec<Match>> {
    let limits = StreamLimits {
        max_line_bytes: Some(MAX_LINE_BYTES),
        max_total_bytes: Some(MAX_ENTRY_BYTES),
        line_limit_label: Some("archive entry line exceeds size limit"),
        total_limit_label: Some("archive entry exceeds size limit"),
    };
    let effective_opts;
    let opts = if options.files_only {
        effective_opts = QueryOptions {
            max_results: 1,
            ..options.clone()
        };
        &effective_opts
    } else {
        options
    };
    let mut stats = StreamStats::default();
    stream_file_bounded(reader, path, regex, opts, limits, &mut stats)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "archive"))]
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

    #[test]
    fn sanitize_archive_path_strips_traversal_separators_nul_and_drives() {
        assert_eq!(sanitize_archive_path(r"\../etc/passwd"), "etc/passwd");
        assert_eq!(sanitize_archive_path("/./a/../b\\c"), "a/b/c");
        assert_eq!(
            sanitize_archive_path(r"C:\Windows\System32\cmd.exe"),
            "Windows/System32/cmd.exe"
        );
        assert_eq!(sanitize_archive_path("foo\0bar/./baz"), "foobar/baz");
    }

    #[test]
    fn scan_zip_handles_non_utf8_multiline_and_type_filter() {
        use zip::ZipWriter;
        use zip::write::SimpleFileOptions;

        let dir = tempdir().unwrap();
        let zip_path = dir.path().join("mixed.zip");
        {
            let file = std::fs::File::create(&zip_path).expect("create zip");
            let mut zip = ZipWriter::new(file);
            zip.start_file("skip.txt", SimpleFileOptions::default())
                .expect("start skip");
            zip.write_all(b"alpha\nomega\n").expect("write skip");

            zip.start_file("code.rs", SimpleFileOptions::default())
                .expect("start rs");
            zip.write_all(b"// \xFF\nfn alpha() {\n    omega();\n}\n")
                .expect("write rs");
            zip.finish().expect("finish");
        }

        let re = Regex::new(r"(?s)alpha.*omega").unwrap();
        let opts = QueryOptions {
            multiline: true,
            type_filter: vec!["rs".to_string()],
            ..Default::default()
        };
        let results = scan_zip(&zip_path, &re, &opts).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].file_path.to_string_lossy().ends_with("code.rs"));
    }

    #[test]
    fn scan_tar_gz_skips_oversized_line_entry_and_keeps_valid_entries() {
        use flate2::{Compression, write::GzEncoder};
        use tar::{Builder, Header};

        let dir = tempdir().unwrap();
        let tgz_path = dir.path().join("partial.tar.gz");
        {
            let file = std::fs::File::create(&tgz_path).expect("create");
            let enc = GzEncoder::new(file, Compression::default());
            let mut tar = Builder::new(enc);

            let huge_line = vec![b'x'; MAX_LINE_BYTES + 128];
            let mut h1 = Header::new_gnu();
            h1.set_size(u64::try_from(huge_line.len()).expect("len"));
            h1.set_path("huge_line.txt").expect("path");
            h1.set_cksum();
            tar.append(&h1, huge_line.as_slice()).expect("append huge");

            let good = b"valid needle here\n";
            let mut h2 = Header::new_gnu();
            h2.set_size(u64::try_from(good.len()).expect("len"));
            h2.set_path("good.txt").expect("path");
            h2.set_cksum();
            tar.append(&h2, &good[..]).expect("append good");

            let enc = tar.into_inner().expect("into_inner");
            enc.finish().expect("finish");
        }

        let re = Regex::new("needle").unwrap();
        let results = scan_tar_gz(&tgz_path, &re, &QueryOptions::default()).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].file_path.to_string_lossy().contains("good.txt"));
    }
}
