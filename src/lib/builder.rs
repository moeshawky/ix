//! Index builder — the complete pipeline from files to .ix shard.
//!
//! Phase 1: Discovery (walk directory, respect .gitignore)
//! Phase 2: Scan (mmap, check binary, extract trigrams, bloom filter)
//! Phase 3: Serialize (write sections, compute CRCs, atomic rename)

use crate::bloom::BloomFilter;
use crate::decompress::maybe_decompress;
use crate::error::{Error, Result};
use crate::format::{
    FileStatus, HEADER_SIZE, MAGIC, VERSION_MAJOR, VERSION_MINOR, flags, is_binary,
};
use crate::posting::{PostingEntry, PostingList};
use crate::trigram::{Extractor, Trigram};
use crate::varint;
use ignore::WalkBuilder;
#[cfg(unix)]
use libc;
use llmosafe::ResourceGuard;
use memmap2::Mmap;
use std::collections::{BinaryHeap, HashMap};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Index builder — coordinates the full pipeline from file discovery to shard
/// serialization.
pub struct Builder {
    root: PathBuf,
    ix_dir: PathBuf,
    file_count: u32,

    // O(1) memory streaming writers for temporary file table and blooms.
    // Wrapped in Option so they can be recreated per-build (clean-before-build pattern).
    files_writer: Option<BufWriter<File>>,
    blooms_writer: Option<BufWriter<File>>,
    strings_writer: Option<BufWriter<File>>,

    // Postings batching for external sort
    postings: HashMap<Trigram, Vec<PostingEntry>>,
    postings_count: usize,
    temp_runs: Vec<PathBuf>,

    extractor: Extractor,
    stats: BuildStats,
    decompress: bool,
    resource_guard: Option<ResourceGuard>,
    dead_ends: Vec<PathBuf>,
    max_file_size: u64,
    exclude_patterns: Vec<String>,
    watch_roots: Vec<PathBuf>,
    committed: bool,
    debug_build: bool,
}

/// Accumulated metrics collected during a build run.
#[derive(Default, Debug)]
pub struct BuildStats {
    /// Number of files processed (text only, after filtering).
    pub files_scanned: u64,
    /// Number of files skipped because they were detected as binary.
    pub files_skipped_binary: u64,
    /// Number of files skipped because they exceeded the size limit.
    pub files_skipped_size: u64,
    /// Total number of raw bytes scanned across all files.
    pub bytes_scanned: u64,
    /// Number of distinct trigrams discovered across all files.
    pub unique_trigrams: u64,
}

struct RunIterator {
    file: BufReader<File>,
}

impl RunIterator {
    fn new(path: &Path) -> Result<Self> {
        let f = File::open(path)?;
        Ok(Self {
            file: BufReader::new(f),
        })
    }

    fn next_trigram(&mut self) -> Result<Option<(Trigram, Vec<PostingEntry>)>> {
        let mut tri_buf = [0u8; 4];
        if let Err(e) = self.file.read_exact(&mut tri_buf) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e.into());
        }
        let tri = u32::from_le_bytes(tri_buf);

        let mut len_buf = [0u8; 4];
        self.file.read_exact(&mut len_buf)?;
        let entries_len = usize::try_from(u32::from_le_bytes(len_buf)).unwrap_or(0);

        let mut entries = Vec::with_capacity(entries_len);
        for _ in 0..entries_len {
            self.file.read_exact(&mut len_buf)?;
            let file_id = u32::from_le_bytes(len_buf);

            self.file.read_exact(&mut len_buf)?;
            let offsets_len = usize::try_from(u32::from_le_bytes(len_buf)).unwrap_or(0);

            let mut offsets = Vec::with_capacity(offsets_len);
            for _ in 0..offsets_len {
                self.file.read_exact(&mut len_buf)?;
                offsets.push(u32::from_le_bytes(len_buf));
            }
            entries.push(PostingEntry { file_id, offsets });
        }

        Ok(Some((tri, entries)))
    }
}

#[derive(Eq, PartialEq)]
struct MergeItem {
    tri: Trigram,
    run_idx: usize,
}

impl PartialOrd for MergeItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MergeItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.tri.cmp(&self.tri) // Min-heap
    }
}

/// Default directory-entry filter shared across the builder walker, the
/// watcher fallback walk, and the scanner.
///
/// Returns `true` if the entry should be **included** in the index/walk.
///
/// # Filtering rules (in order)
///
/// 1. **`watch_roots`** (`&[PathBuf]`) — if non-empty, files whose path does not
///    start with any watch root are excluded (directories are always traversed
///    so that children under watch roots are reachable).
/// 2. **Directory exclusions** — `lost+found`, `.git`, `.ix`, and any name in
///    `exclude_patterns` (`&[String]`).
/// 3. **Index-internal files** — `shard.ix`, `shard.ix.tmp`, `shard.ix.*`.
/// 4. **Binary extensions** — `.so`, `.o`, `.dylib`, `.a`, `.dll`, `.exe`,
///    `.pyc`, `.jpg`, `.png`, `.gif`, `.mp4`, `.mp3`, `.pdf`, `.zip`, `.7z`,
///    `.rar`, `.sqlite`, `.db`, `.bin`, `*.tar.gz`.
#[must_use]
pub(crate) fn default_filter_entry(
    entry: &ignore::DirEntry,
    exclude_patterns: &[String],
    watch_roots: &[PathBuf],
) -> bool {
    let path = entry.path();
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    if !watch_roots.is_empty() {
        let is_file = entry.file_type().is_some_and(|t| t.is_file());
        if is_file && !watch_roots.iter().any(|wr| path.starts_with(wr)) {
            return false;
        }
    }

    if entry.file_type().is_some_and(|t| t.is_dir())
        && (name == "lost+found"
            || name == ".git"
            || name == ".ix"
            || exclude_patterns.iter().any(|p| p == name))
    {
        return false;
    }

    if entry.file_type().is_some_and(|t| t.is_file())
        && (name == "shard.ix" || name == "shard.ix.tmp" || name.starts_with("shard.ix."))
    {
        return false;
    }

    if entry.file_type().is_some_and(|t| t.is_file()) {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match ext {
            "so" | "o" | "dylib" | "a" | "dll" | "exe" | "pyc" | "jpg" | "png" | "gif" | "mp4"
            | "mp3" | "pdf" | "zip" | "7z" | "rar" | "sqlite" | "db" | "bin" => {
                return false;
            }
            _ => {}
        }
        if name.ends_with(".tar.gz") {
            return false;
        }
    }
    true
}

#[allow(clippy::as_conversions)] // binary format: usize→u32/u16 for index encoding
#[allow(clippy::indexing_slicing)] // binary format: fixed-size buffer ops
impl Builder {
    /// Creates a new builder for the given root directory.
    ///
    /// # Errors
    ///
    /// Returns an error if the `.ix` directory cannot be created or temp files
    /// cannot be opened.
    pub fn new(root: &Path) -> Result<Self> {
        let ix_dir = root.join(".ix");
        fs::create_dir_all(&ix_dir)?;

        Ok(Self {
            root: root.to_owned(),
            ix_dir,
            file_count: 0,
            files_writer: None,
            blooms_writer: None,
            strings_writer: None,
            postings: HashMap::new(),
            postings_count: 0,
            temp_runs: Vec::new(),
            extractor: Extractor::new(),
            stats: BuildStats::default(),
            decompress: false,
            resource_guard: None,
            dead_ends: Vec::new(),
            max_file_size: 100 * 1024 * 1024,
            // Builder starts with no exclude_patterns — callers set them via
            // with_exclude_patterns(). The daemon (daemon.rs:195) bridges
            // Config's defaults here; standalone CLI builds get an empty set.
            // See also: src/lib/config.rs:39, src/lib/watcher.rs:40
            exclude_patterns: Vec::new(),
            watch_roots: Vec::new(),
            committed: false,
            debug_build: false,
        })
    }

    /// Attach a [`ResourceGuard`] for memory-pressure-driven flush.
    #[must_use]
    pub const fn with_resource_guard(mut self, guard: ResourceGuard) -> Self {
        self.resource_guard = Some(guard);
        self
    }

    /// Enable or disable transparent decompression of archives during
    /// scanning.
    pub const fn set_decompress(&mut self, decompress: bool) {
        self.decompress = decompress;
    }

    /// Set the maximum file size to index (0 = unlimited). Default: 100 MB.
    pub const fn set_max_file_size(&mut self, max_bytes: u64) {
        self.max_file_size = max_bytes;
    }

    /// Set directory name patterns to exclude during the file walk.
    ///
    /// Names are matched against the last component of each entry's
    /// path. Matching directories and their contents are skipped.
    #[must_use]
    pub fn with_exclude_patterns(mut self, patterns: Vec<String>) -> Self {
        self.exclude_patterns = patterns;
        self
    }

    /// Restrict file discovery to the given subdirectory paths.
    ///
    /// When non-empty, only files under one of the specified watch roots
    /// are indexed. Paths are relative to the builder's root directory.
    /// An empty set (default) indexes all files under the root.
    #[must_use]
    pub fn with_watch_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.watch_roots = roots;
        self
    }

    /// Initialize temporary files and writers for building.
    ///
    /// Called at start of each `build()` to ensure fresh file handles.
    /// This is the core of the "clean-before-build" pattern: temp files
    /// are BUILD ARTIFACTS, not struct state — created fresh each build,
    /// cleaned at NEXT build start, never deleted mid-lifecycle.
    ///
    /// # Errors
    ///
    /// Returns an error if temp files cannot be created.
    fn init_writers(&mut self) -> Result<()> {
        let files_tmp = self.ix_dir.join("shard.ix.tmp.files");
        let blooms_tmp = self.ix_dir.join("shard.ix.tmp.blooms");
        let strings_tmp = self.ix_dir.join("shard.ix.tmp.strings");

        self.files_writer = Some(BufWriter::new(File::create(&files_tmp)?));
        self.blooms_writer = Some(BufWriter::new(File::create(&blooms_tmp)?));
        let mut strings_writer = BufWriter::new(File::create(&strings_tmp)?);

        // Initialize strings table header (10 bytes total):
        // version(u32) + size(u16) + count(u16) + padding(2)
        strings_writer.write_all(&1u32.to_le_bytes())?; // version
        strings_writer.write_all(&0u16.to_le_bytes())?; // size placeholder
        strings_writer.write_all(&0u16.to_le_bytes())?; // count
        strings_writer.write_all(&[0u8; 2])?; // padding

        self.strings_writer = Some(strings_writer);
        Ok(())
    }

    /// Clean up old temporary files from a previous build.
    ///
    /// Called at start of `build()` to release disk space before
    /// allocating new temp files. On Linux, deleting a file while a
    /// process holds an open FD keeps the inode alive; dropping the
    /// old writers first ensures inodes are released.
    fn cleanup_old_temp_files(&mut self) {
        // Drop writers first to release inode references on Linux
        self.files_writer = None;
        self.blooms_writer = None;
        self.strings_writer = None;

        let patterns = [
            "shard.ix.tmp.files",
            "shard.ix.tmp.blooms",
            "shard.ix.tmp.strings",
            "shard.ix.tmp",
            "shard.ix.bak",
        ];

        for pattern in &patterns {
            let path = self.ix_dir.join(pattern);
            if path.exists() {
                if let Err(e) = fs::remove_file(&path) {
                    tracing::warn!("failed to clean temp file {}: {e}", path.display());
                }
            }
        }

        if let Ok(entries) = fs::read_dir(&self.ix_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("shard.ix.run.") || name.starts_with("shard.ix.merged.") {
                    if let Err(e) = fs::remove_file(entry.path()) {
                        tracing::warn!("failed to clean temp run {}: {e}", entry.path().display());
                    }
                }
            }
        }
    }
    /// Helper to get mutable writer reference with proper error handling.
    ///
    /// Returns an error if the writer is not initialized (violates clean-before-build contract).
    fn get_writer<'a, T>(writer: &'a mut Option<T>, context: &'static str) -> Result<&'a mut T> {
        writer.as_mut().ok_or_else(|| {
        Error::Io(std::io::Error::other(format!(
            "Builder invariant violated: {context} not initialized (clean-before-build contract)"
        )))
        })
    }

    fn flush_run(&mut self) -> Result<()> {
        if self.postings.is_empty() {
            return Ok(());
        }
        let old_postings = std::mem::take(&mut self.postings);
        let mut sorted: Vec<_> = old_postings.into_iter().collect();
        sorted.sort_unstable_by_key(|(t, _)| *t);

        let run_path = self
            .ix_dir
            .join(format!("shard.ix.run.{}", self.temp_runs.len()));
        let mut f = BufWriter::new(File::create(&run_path)?);

        for (tri, entries) in sorted {
            f.write_all(&tri.to_le_bytes())?;
            f.write_all(
                &u32::try_from(entries.len())
                    .unwrap_or_else(|_| {
                        tracing::warn!("trigram entry count overflow: {}", entries.len());
                        0
                    })
                    .to_le_bytes(),
            )?;
            for entry in entries {
                f.write_all(&entry.file_id.to_le_bytes())?;
                f.write_all(
                    &u32::try_from(entry.offsets.len())
                        .unwrap_or_else(|_| {
                            tracing::warn!("trigram entry count overflow: {}", entry.offsets.len());
                            0
                        })
                        .to_le_bytes(),
                )?;
                for off in entry.offsets {
                    f.write_all(&off.to_le_bytes())?;
                }
            }
        }
        f.flush()?;

        self.temp_runs.push(run_path);
        self.postings_count = 0;
        Ok(())
    }

    /// Build or rebuild the index from scratch.
    ///
    /// # Errors
    ///
    /// Returns an error if the build process fails (I/O, disk space, etc.).
    pub fn build(&mut self) -> Result<PathBuf> {
        // === Clean-before-build: release old temp files + create fresh writers ===
        // This fixes the stale FD bug where serialize() deletes temp files but
        // Builder keeps BufWriter handles pointing to deleted inodes.
        // On Linux, dropping the writers releases the inode references first.
        self.cleanup_old_temp_files();
        self.init_writers()?;

        // Reset state for fresh build — clear any residual data from previous builds
        self.postings.clear();
        self.postings_count = 0;
        self.temp_runs.clear();
        self.file_count = 0;
        self.stats = BuildStats::default();
        self.dead_ends.clear();
        self.committed = false;

        // Upfront disk space check: estimate required bytes before any I/O.

        // Peak overhead is ~3× existing shard (old + tmp + bak + runs).
        // For first builds, use a 200 MB floor.
        if let Ok(free) = Self::free_bytes_at(&self.ix_dir) {
            let existing_shard_size =
                fs::metadata(self.ix_dir.join("shard.ix")).map_or(0, |m| m.len());
            let required = if existing_shard_size > 0 {
                existing_shard_size.saturating_mul(3)
            } else {
                200 * 1024 * 1024 // 200 MB floor for first builds
            };
            if free < required {
                return Err(Error::Io(std::io::Error::other(format!(
                    "insufficient disk space: {} MB free, need ≥{} MB (path: {})",
                    free / 1024 / 1024,
                    required / 1024 / 1024,
                    self.ix_dir.display(),
                ))));
            }
        }

        // Cleanup old intermediate shard files before building
        if self.ix_dir.exists()
            && let Ok(entries) = std::fs::read_dir(&self.ix_dir)
        {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if (name_str.starts_with("shard.ix.run.")
                    || name_str.starts_with("shard.ix.merged."))
                    && let Err(e) = std::fs::remove_file(entry.path())
                {
                    tracing::warn!("Failed to cleanup shard file {}: {}", name_str, e);
                }
            }
        }

        let start = Instant::now();
        let root = self.root.clone();
        self.debug_build = std::env::var("IX_DEBUG_BUILD").is_ok();

        // LLMOSafe Formal Law: Sensitive filesystem traversal (Root)
        if root.to_string_lossy() == "/" {
            tracing::warn!(
                "LLMOSafe Advisory: Indexing root filesystem. Ensure adequate resource guards are in place."
            );
        }

        let walker = WalkBuilder::new(&root)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            // require_git(true): apply .gitignore only WITHIN a git repo, never ancestor
            // (e.g. ~/.gitignore). With require_git(false) the ignore crate climbs parent
            // directories and pulls in ~/.gitignore rules — a global `lib/` rule then
            // silently excludes src/lib from the index (file-scope search returns 0
            // while rg/find match). Matches ripgrep's default. See audit D4.
            .require_git(true)
            .add_custom_ignore_filename(".ixignore")
            .filter_entry({
                let exclude_patterns = self.exclude_patterns.clone();
                let watch_roots = self.watch_roots.clone();
                move |entry| default_filter_entry(entry, &exclude_patterns, &watch_roots)
            })
            .build();

        let mut files_processed = 0u64;
        for entry_res in walker {
            let entry = match entry_res {
                Ok(e) => e,
                Err(e) => {
                    // Handle KernelError::BacktrackSignaled (-7) during the walk
                    let backtrack_path = match &e {
                        ignore::Error::Io(io_err) if io_err.raw_os_error() == Some(-7) => {
                            Some(None)
                        }
                        ignore::Error::WithPath { path, err } => {
                            if let ignore::Error::Io(io_err) = err.as_ref() {
                                if io_err.raw_os_error() == Some(-7) {
                                    Some(Some(path.clone()))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };

                    if let Some(path_opt) = backtrack_path {
                        tracing::warn!(
                            "Immune Memory Triggered: Skipping path due to backtrack signal."
                        );
                        if let Some(path) = path_opt {
                            self.dead_ends.push(path);
                        }
                    }
                    continue;
                }
            };

            if entry.file_type().is_some_and(|t| t.is_file()) {
                self.process_file(entry.path())?;
                files_processed += 1;

                // Resource Guard Check: check every 250 files to prevent OOM
                // allow: manual_is_multiple_of — MSRV 1.85 compat; is_multiple_of not available
                #[allow(clippy::manual_is_multiple_of)]
                if files_processed % 250 == 0 {
                    if let Some(guard) = &self.resource_guard {
                        if let Err(_err) = guard.check() {
                            eprintln!(
                                "ixd: memory ceiling reached... flushing intermediate chunk ({files_processed} files processed)"
                            );
                            self.flush_run()?;
                        }
                    } else {
                        // Fallback to proportional RSS limit if no formal guard provided.
                        // Use 80% of system RAM — higher than ResourceGuard's 60%, so the
                        // smart governor always fires first when present.
                        let sys_mem = ResourceGuard::system_memory_bytes();
                        let rss_threshold: u64 = if sys_mem > 0 {
                            (sys_mem.saturating_mul(4) / 5) as u64
                        } else {
                            512 * 1024 * 1024
                        };
                        if let Ok(rss) = Self::current_rss_bytes()
                            && rss > rss_threshold
                        {
                            eprintln!(
                                "ixd: RSS ceiling reached ({} MB) after {} files — flushing intermediate chunk",
                                rss / 1024 / 1024,
                                files_processed
                            );
                            self.flush_run()?;
                        }
                    }
                }
            }
        }

        let output_path = self.serialize()?;
        if let Err(e) = std::fs::remove_file(self.ix_dir.join("shard.ix.delta")) {
            tracing::warn!("failed to clean delta file: {e}");
        }
        if let Err(e) = std::fs::remove_file(self.ix_dir.join("shard.ix.delta.tmp")) {
            tracing::warn!("failed to clean delta tmp file: {e}");
        }
        tracing::info!("Build completed in {:?}: {:?}", start.elapsed(), self.stats);
        Ok(output_path)
    }

    /// Update the index for changed files using append-only delta indexing.
    ///
    /// # Errors
    ///
    /// Returns an error if I/O fails or a file cannot be processed.
    pub fn update(&mut self, changed_files: &[PathBuf]) -> Result<PathBuf> {
        use crate::format::{DELTA_MAGIC, DELTA_TOMBSTONE};
        use crate::reader::{DeltaReader, Reader};
        use std::fs::OpenOptions;

        let index_file = self.ix_dir.join("shard.ix");
        let delta_file = self.ix_dir.join("shard.ix.delta");
        let delta_tmp = self.ix_dir.join("shard.ix.delta.tmp");

        let Ok(main_reader) = Reader::open(&index_file) else {
            return self.build();
        };

        let delta_reader = DeltaReader::open(&delta_file)?;
        let mut next_file_id = main_reader.header.file_count + delta_reader.total_file_entries;

        let is_new = !delta_file.exists();
        if !is_new {
            std::fs::copy(&delta_file, &delta_tmp)?;
        }
        let mut delta_out = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&delta_tmp)?;
        if is_new {
            delta_out.write_all(&DELTA_MAGIC)?;
        }

        let mut path_to_id = delta_reader.path_to_id;
        for file_id in 0..main_reader.header.file_count {
            if let Ok(info) = main_reader.get_file(file_id) {
                path_to_id.entry(info.path).or_insert(file_id);
            }
        }

        for path in changed_files {
            if !self.watch_roots.is_empty()
                && !self.watch_roots.iter().any(|wr| path.starts_with(wr))
            {
                continue;
            }

            if let Some(&old_id) = path_to_id.get(path) {
                delta_out.write_all(&[DELTA_TOMBSTONE])?;
                delta_out.write_all(&old_id.to_le_bytes())?;
            }

            if path.exists() && self.process_file_delta(path, next_file_id, &mut delta_out)? {
                next_file_id += 1;
            }
        }

        delta_out.flush()?;
        std::fs::rename(&delta_tmp, &delta_file)?;
        Ok(index_file)
    }

    /// Number of files indexed so far (snapshot for progress reporting).
    #[must_use]
    pub const fn files_len(&self) -> usize {
        self.file_count as usize
    }

    /// Number of unique trigrams accumulated so far.
    #[must_use]
    pub const fn trigrams_len(&self) -> usize {
        self.stats.unique_trigrams as usize
    }

    /// Returns a reference to the accumulated [`BuildStats`] for this build run.
    #[must_use]
    pub const fn stats(&self) -> &BuildStats {
        &self.stats
    }

    /// Returns current process RSS in bytes by reading /proc/self/status.
    fn current_rss_bytes() -> std::io::Result<u64> {
        let status = std::fs::read_to_string("/proc/self/status")?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kb: u64 = rest
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                return Ok(kb * 1024);
            }
        }
        Ok(0)
    }

    /// Returns free bytes available on the filesystem containing `path`.
    #[cfg(unix)]
    fn free_bytes_at(path: &Path) -> std::io::Result<u64> {
        use std::os::unix::ffi::OsStrExt;
        let path_c = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        // SAFETY: libc::statvfs only requires a pointer to an initialized
        // statvfs struct. std::mem::zeroed() produces a valid zeroed struct
        // on Linux. path_c is a null-terminated CString with valid UTF-8
        // content.
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: stat is a valid, initialized statvfs struct. path_c is a
        // null-terminated CString.
        let ret = unsafe { libc::statvfs(path_c.as_ptr(), &raw mut stat) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // statvfs fields are fsblkcnt_t (platform-width); explicit u64
        // conversion required for multiplication safety on 32-bit targets.
        #[allow(
            clippy::unnecessary_cast,
            clippy::cast_lossless,
            clippy::useless_conversion
        )]
        Ok(stat.f_bavail as u64 * stat.f_frsize as u64)
    }

    /// Windows: skip the upfront disk check (no statvfs).
    #[cfg(not(unix))]
    #[allow(clippy::unnecessary_wraps)]
    fn free_bytes_at(_path: &Path) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }

    fn cleanup_temp_files(&self) {
        let paths = [
            self.ix_dir.join("shard.ix.tmp.files"),
            self.ix_dir.join("shard.ix.tmp.blooms"),
            self.ix_dir.join("shard.ix.tmp.strings"),
            self.ix_dir.join("shard.ix.tmp"),
            self.ix_dir.join("shard.ix.bak"),
        ];
        for p in &paths {
            if let Err(e) = fs::remove_file(p)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("Failed to cleanup temp file {}: {}", p.display(), e);
            }
        }
        for run_path in &self.temp_runs {
            if let Err(e) = fs::remove_file(run_path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("Failed to cleanup temp run {}: {}", run_path.display(), e);
            }
        }
    }

    fn process_file(&mut self, path: &Path) -> Result<bool> {
        let path_str = path.display().to_string();
        // TOCTOU guard: file may have been deleted between walk and open
        let metadata = match fs::metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        let size = metadata.len();
        let mtime = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(0));

        if self.max_file_size > 0 && size > self.max_file_size {
            tracing::debug!("SKIP: file too large ({size} bytes): {path_str}");
            self.stats.files_skipped_size += 1;
            return Ok(false);
        }

        let file = match File::open(path) {
            Ok(f) => f,
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    || e.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        };
        // SAFETY: The file is held open for the duration of this method,
        // and no concurrent modification is expected — the file is either
        // being indexed from disk (read-only) or watched by the daemon
        // which serializes rebuilds through the builder.
        let mmap = unsafe { Mmap::map(&file)? };

        let raw_data = if self.decompress {
            if let Some(mut reader) = maybe_decompress(path, &mmap)? {
                let mut buf = Vec::new();
                let take_limit = if self.max_file_size > 0 {
                    self.max_file_size
                } else {
                    100 * 1024 * 1024
                };
                reader.by_ref().take(take_limit).read_to_end(&mut buf)?;
                std::borrow::Cow::Owned(buf)
            } else {
                std::borrow::Cow::Borrowed(&mmap[..])
            }
        } else {
            std::borrow::Cow::Borrowed(&mmap[..])
        };

        let data = &raw_data[..];
        if is_binary(data) {
            tracing::debug!("SKIP: binary detection ({size} bytes): {path_str}");
            self.stats.files_skipped_binary += 1;
            return Ok(false);
        }

        let content_hash = xxhash_rust::xxh64::xxh64(data, 0);
        let pairs = self.extractor.extract_with_offsets(data);

        if pairs.is_empty() && !data.is_empty() {
            tracing::debug!("SKIP: no trigrams extracted ({size} bytes): {path_str}");
        }

        let file_id = self.file_count;
        self.file_count += 1;

        let path_str = path.to_string_lossy();
        let path_bytes = path_str.as_bytes();
        let path_off = u32::try_from(
            Self::get_writer(&mut self.strings_writer, "strings_writer")?.stream_position()?,
        )
        .map_err(|e| Error::Io(std::io::Error::other(e)))?;
        let path_len = u16::try_from(path_bytes.len()).map_err(|_| {
            Error::Config(format!("path exceeds maximum length: {}", path_bytes.len()))
        })?;

        self.strings_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("strings_writer not initialized")))?
            .write_all(&0u16.to_le_bytes())?;
        self.strings_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("strings_writer not initialized")))?
            .write_all(&path_len.to_le_bytes())?;
        self.strings_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("strings_writer not initialized")))?
            .write_all(path_bytes)?;

        let mut bloom = BloomFilter::new(256, 5);
        let mut trigram_count = 0u32;

        let mut i = 0;
        while i < pairs.len() {
            let tri = pairs[i].0;
            let mut j = i + 1;
            while j < pairs.len() && pairs[j].0 == tri {
                j += 1;
            }

            bloom.insert(tri);
            trigram_count += 1;

            let mut start = i;
            while start < j {
                let end = (start + 10_000).min(j);
                let offsets: Vec<u32> = pairs[start..end].iter().map(|p| p.1).collect();
                let take = end - start;
                self.postings
                    .entry(tri)
                    .or_default()
                    .push(PostingEntry { file_id, offsets });
                self.postings_count += take + 8;
                start = end;
            }
            i = j;
        }

        bloom.serialize(
            self.blooms_writer
                .as_mut()
                .ok_or_else(|| Error::Io(std::io::Error::other("blooms_writer not initialized")))?,
        )?;

        let bloom_offset = u64::from(file_id)
            .checked_mul(260)
            .ok_or_else(|| Error::Config("file_id * 260 overflow".into()))?;
        let bloom_offset_u32 = u32::try_from(bloom_offset)
            .map_err(|_| Error::Config(format!("bloom_offset {bloom_offset} exceeds u32::MAX")))?;
        self.files_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("files_writer not initialized")))?
            .write_all(&file_id.to_le_bytes())?;
        self.files_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("files_writer not initialized")))?
            .write_all(&path_off.to_le_bytes())?;
        self.files_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("files_writer not initialized")))?
            .write_all(&path_len.to_le_bytes())?;
        self.files_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("files_writer not initialized")))?
            .write_all(&[FileStatus::Fresh as u8])?;
        self.files_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("files_writer not initialized")))?
            .write_all(&[0u8])?;
        self.files_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("files_writer not initialized")))?
            .write_all(&mtime.to_le_bytes())?;
        self.files_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("files_writer not initialized")))?
            .write_all(&size.to_le_bytes())?;
        self.files_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("files_writer not initialized")))?
            .write_all(&content_hash.to_le_bytes())?;
        self.files_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("files_writer not initialized")))?
            .write_all(&trigram_count.to_le_bytes())?;
        self.files_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("files_writer not initialized")))?
            .write_all(&bloom_offset_u32.to_le_bytes())?;
        self.files_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("files_writer not initialized")))?
            .write_all(&[0u8; 4])?;

        self.stats.files_scanned += 1;
        self.stats.bytes_scanned += size;

        if self.debug_build {
            eprintln!(
                "IX-INDEXED: file_id={file_id} unique_trigrams={trigram_count} size={size}: {path_str}"
            );
        }

        // Flush every 500k entries (~8MB peak RAM) to prevent unbounded HashMap growth.
        // This was the RAM DDOS root cause in v0.1.1 — threshold was 5M (far too high).
        if self.postings_count >= 500_000 {
            if self.debug_build {
                eprintln!(
                    "IX-FLUSH: postings_count={} after file_id={file_id}",
                    self.postings_count
                );
            }
            self.flush_run()?;
        }

        Ok(true)
    }

    #[allow(clippy::too_many_lines)]
    fn process_file_delta<W: std::io::Write>(
        &mut self,
        path: &Path,
        file_id: u32,
        delta: &mut W,
    ) -> Result<bool> {
        use crate::format::{DELTA_FILE_ENTRY, DELTA_TRIGRAM_ENTRY};

        let metadata = match fs::metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        let size = metadata.len();
        let mtime = metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);

        if self.max_file_size > 0 && size > self.max_file_size {
            return Ok(false);
        }
        let file = fs::File::open(path)?;
        // SAFETY: The file is held open via `file` for the duration of this
        // method. The file system will not truncate the file while we hold
        // the open handle. The mmap region covers the entire file as mapped.
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        let data = &mmap[..];
        if crate::format::is_binary(data) {
            return Ok(false);
        }

        let content_hash = xxhash_rust::xxh64::xxh64(data, 0);
        let pairs = self.extractor.extract_with_offsets(data);

        let mut bloom = BloomFilter::new(256, 5);
        let mut trigram_entries: Vec<(crate::trigram::Trigram, Vec<u32>)> = Vec::new();

        let mut i = 0;
        while i < pairs.len() {
            let tri = pairs[i].0;
            let mut j = i + 1;
            while j < pairs.len() && pairs[j].0 == tri {
                j += 1;
            }
            let offsets: Vec<u32> = pairs[i..j].iter().map(|p| p.1).collect();
            bloom.insert(tri);
            trigram_entries.push((tri, offsets));
            i = j;
        }

        let path_bytes = path.to_string_lossy().as_bytes().to_vec();
        let path_len_u16 = u16::try_from(path_bytes.len()).map_err(|_| {
            Error::Config(format!("path exceeds maximum length: {}", path_bytes.len()))
        })?;
        delta.write_all(&[DELTA_FILE_ENTRY])?;
        delta.write_all(&file_id.to_le_bytes())?;
        delta.write_all(&path_len_u16.to_le_bytes())?;
        delta.write_all(&path_bytes)?;
        delta.write_all(&mtime.to_le_bytes())?;
        delta.write_all(&size.to_le_bytes())?;
        delta.write_all(&content_hash.to_le_bytes())?;
        let mut bloom_buf = Vec::new();
        bloom.serialize(&mut bloom_buf)?;
        delta.write_all(&bloom_buf)?;

        for (tri, offsets) in trigram_entries {
            delta.write_all(&[DELTA_TRIGRAM_ENTRY])?;
            delta.write_all(&tri.to_le_bytes())?;
            delta.write_all(&1u32.to_le_bytes())?;
            delta.write_all(&file_id.to_le_bytes())?;
            delta.write_all(&(offsets.len() as u32).to_le_bytes())?;
            for off in offsets {
                delta.write_all(&off.to_le_bytes())?;
            }
        }

        Ok(true)
    }

    fn serialize(&mut self) -> Result<PathBuf> {
        self.flush_run()?;

        self.files_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("files_writer not initialized")))?
            .flush()?;
        self.blooms_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("blooms_writer not initialized")))?
            .flush()?;
        self.strings_writer
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("strings_writer not initialized")))?
            .flush()?;

        // Hierarchical Merge to stay under ulimit
        while self.temp_runs.len() > 128 {
            let mut next_generation = Vec::new();
            for chunk in self.temp_runs.chunks(128) {
                let out_path = self.ix_dir.join(format!(
                    "shard.ix.merged.{}.{}",
                    next_generation.len(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|e| Error::Config(format!("time went backwards: {e}")))?
                        .as_micros()
                ));
                Self::merge_to_run(chunk, &out_path)?;
                next_generation.push(out_path);
                for p in chunk {
                    if let Err(e) = fs::remove_file(p) {
                        tracing::warn!("Failed to cleanup temp run {}: {}", p.display(), e);
                    }
                }
            }
            self.temp_runs = next_generation;
        }

        let tmp_path = self.ix_dir.join("shard.ix.tmp");
        let final_path = self.ix_dir.join("shard.ix");

        let mut f = BufWriter::new(File::create(&tmp_path)?);
        f.write_all(&[0u8; HEADER_SIZE])?;

        let file_table_offset = Self::align_to_8(&mut f)?;
        let mut files_reader = File::open(self.ix_dir.join("shard.ix.tmp.files"))?;
        std::io::copy(&mut files_reader, &mut f)?;
        let file_table_size = f.stream_position()? - file_table_offset;

        Self::align_to_8(&mut f)?;
        let posting_data_offset = f.stream_position()?;

        let mut cdx_entries: Vec<(Trigram, u64, u32, u32)> = Vec::new();
        let mut global_trigram_count = 0u32;

        let mut runs = Vec::new();
        for path in &self.temp_runs {
            runs.push(RunIterator::new(path)?);
        }

        let mut heap = BinaryHeap::new();
        let mut current_items = vec![None; runs.len()];

        for (i, run) in runs.iter_mut().enumerate() {
            if let Some(item) = run.next_trigram()? {
                heap.push(MergeItem {
                    tri: item.0,
                    run_idx: i,
                });
                current_items[i] = Some(item);
            }
        }

        let mut current_tri: Option<Trigram> = None;
        let mut merged_entries: Vec<PostingEntry> = Vec::new();

        while let Some(MergeItem { tri, run_idx }) = heap.pop() {
            if Some(tri) != current_tri {
                if let Some(t) = current_tri {
                    let cdx_entry = Self::write_merged_posting(
                        &mut f,
                        t,
                        posting_data_offset,
                        &mut merged_entries,
                    )?;
                    cdx_entries.push(cdx_entry);
                    global_trigram_count += 1;
                    merged_entries.clear();
                }
                current_tri = Some(tri);
            }

            let item = current_items
                .get_mut(run_idx)
                .ok_or_else(|| Error::Config("run_idx out of bounds".into()))?
                .take()
                .ok_or_else(|| Error::Config("current item is None".into()))?;
            merged_entries.extend(item.1);

            if let Some(next_item) = runs
                .get_mut(run_idx)
                .ok_or_else(|| Error::Config("run_idx out of bounds".into()))?
                .next_trigram()?
            {
                heap.push(MergeItem {
                    tri: next_item.0,
                    run_idx,
                });
                *current_items
                    .get_mut(run_idx)
                    .ok_or_else(|| Error::Config("run_idx out of bounds".into()))? =
                    Some(next_item);
            }
        }

        if let Some(t) = current_tri {
            let cdx_entry =
                Self::write_merged_posting(&mut f, t, posting_data_offset, &mut merged_entries)?;
            cdx_entries.push(cdx_entry);
            global_trigram_count += 1;
        }

        self.stats.unique_trigrams = u64::from(global_trigram_count);
        let posting_data_size = f.stream_position()? - posting_data_offset;

        Self::align_to_8(&mut f)?;
        let trigram_table_offset = f.stream_position()?;

        // CDX encode: delta + varint + ZSTD per block
        let cdx_block_index = Self::write_cdx_blocks(&mut f, &cdx_entries)?;
        let trigram_table_size = f.stream_position()? - trigram_table_offset;

        Self::align_to_8(&mut f)?;
        let cdx_block_index_offset = f.stream_position()?;
        for entry in &cdx_block_index {
            f.write_all(&entry.0.to_le_bytes())?;
            f.write_all(&entry.1.to_le_bytes())?;
        }
        // Sentinel entry
        f.write_all(&u32::MAX.to_le_bytes())?;
        f.write_all(&(trigram_table_offset + trigram_table_size).to_le_bytes())?;
        let cdx_block_index_size = f.stream_position()? - cdx_block_index_offset;

        Self::align_to_8(&mut f)?;
        let bloom_offset = f.stream_position()?;
        let mut blooms_reader = File::open(self.ix_dir.join("shard.ix.tmp.blooms"))?;
        std::io::copy(&mut blooms_reader, &mut f)?;
        let bloom_size = f.stream_position()? - bloom_offset;

        Self::align_to_8(&mut f)?;
        let string_pool_offset = f.stream_position()?;
        let mut strings_reader = File::open(self.ix_dir.join("shard.ix.tmp.strings"))?;
        std::io::copy(&mut strings_reader, &mut f)?;
        let string_pool_size = f.stream_position()? - string_pool_offset;

        let name_index_offset = f.stream_position()?;
        let name_index_size = 0u64;

        let created_at = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| Error::Config(format!("time went backwards: {e}")))?
                .as_micros(),
        )
        .unwrap_or(0);
        let mut header_bytes = [0u8; HEADER_SIZE];
        header_bytes[0..4].copy_from_slice(&MAGIC);
        header_bytes[0x04..0x06].copy_from_slice(&VERSION_MAJOR.to_le_bytes());
        header_bytes[0x06..0x08].copy_from_slice(&VERSION_MINOR.to_le_bytes());
        header_bytes[0x08..0x10].copy_from_slice(
            &(flags::HAS_BLOOM_FILTERS
                | flags::HAS_CONTENT_HASHES
                | flags::POSTING_LISTS_COMPRESSED
                | flags::POSTING_LISTS_CHECKSUMMED
                | flags::HAS_CDX_INDEX)
                .to_le_bytes(),
        );
        header_bytes[0x10..0x18].copy_from_slice(&created_at.to_le_bytes());
        header_bytes[0x18..0x20].copy_from_slice(&self.stats.bytes_scanned.to_le_bytes());
        header_bytes[0x20..0x24].copy_from_slice(&self.file_count.to_le_bytes());
        header_bytes[0x24..0x28].copy_from_slice(&(global_trigram_count).to_le_bytes());
        header_bytes[0x28..0x30].copy_from_slice(&file_table_offset.to_le_bytes());
        header_bytes[0x30..0x38].copy_from_slice(&file_table_size.to_le_bytes());
        header_bytes[0x38..0x40].copy_from_slice(&trigram_table_offset.to_le_bytes());
        header_bytes[0x40..0x48].copy_from_slice(&trigram_table_size.to_le_bytes());
        header_bytes[0x48..0x50].copy_from_slice(&posting_data_offset.to_le_bytes());
        header_bytes[0x50..0x58].copy_from_slice(&posting_data_size.to_le_bytes());
        header_bytes[0x58..0x60].copy_from_slice(&bloom_offset.to_le_bytes());
        header_bytes[0x60..0x68].copy_from_slice(&bloom_size.to_le_bytes());
        header_bytes[0x68..0x70].copy_from_slice(&string_pool_offset.to_le_bytes());
        header_bytes[0x70..0x78].copy_from_slice(&string_pool_size.to_le_bytes());
        header_bytes[0x78..0x80].copy_from_slice(&name_index_offset.to_le_bytes());
        header_bytes[0x80..0x88].copy_from_slice(&name_index_size.to_le_bytes());
        header_bytes[0x88..0x90].copy_from_slice(&cdx_block_index_offset.to_le_bytes());
        header_bytes[0x90..0x98].copy_from_slice(&cdx_block_index_size.to_le_bytes());

        let crc = crc32c::crc32c(&header_bytes[0..0xF8]);
        header_bytes[0xF8..0xFC].copy_from_slice(&crc.to_le_bytes());

        f.seek(SeekFrom::Start(0))?;
        f.write_all(&header_bytes)?;
        f.flush()?;
        drop(f);

        // Atomic swap: backup old index, rename new to final
        let backup_path = self.ix_dir.join("shard.ix.bak");
        let old_index_exists = final_path.exists();
        if old_index_exists {
            // Remove old backup if exists
            if let Err(e) = fs::remove_file(&backup_path) {
                tracing::warn!("failed to clean old backup file: {e}");
            }
            // Backup current index
            fs::rename(&final_path, &backup_path).map_err(|e| {
                Error::Io(std::io::Error::other(format!(
                    "failed to backup existing index: {e}"
                )))
            })?;
        }
        fs::rename(&tmp_path, &final_path)?;
        self.committed = true;
        // Remove backup after successful rename
        if old_index_exists {
            if let Err(e) = fs::remove_file(&backup_path) {
                tracing::warn!("failed to clean backup file: {e}");
            }
        }

        if let Err(e) = fs::remove_file(self.ix_dir.join("shard.ix.tmp.files")) {
            tracing::warn!("failed to clean temp files: {e}");
        }
        if let Err(e) = fs::remove_file(self.ix_dir.join("shard.ix.tmp.blooms")) {
            tracing::warn!("failed to clean temp blooms: {e}");
        }
        if let Err(e) = fs::remove_file(self.ix_dir.join("shard.ix.tmp.strings")) {
            tracing::warn!("failed to clean temp strings: {e}");
        }
        for path in &self.temp_runs {
            if let Err(e) = fs::remove_file(path) {
                tracing::warn!("Failed to cleanup temp run {}: {}", path.display(), e);
            }
        }
        self.temp_runs.clear();

        Ok(final_path)
    }

    fn merge_to_run(run_paths: &[PathBuf], out_path: &Path) -> Result<()> {
        let mut runs = Vec::new();
        for path in run_paths {
            runs.push(RunIterator::new(path)?);
        }
        let mut heap = BinaryHeap::new();
        let mut current_items = vec![None; runs.len()];
        for (i, run) in runs.iter_mut().enumerate() {
            if let Some(item) = run.next_trigram()? {
                heap.push(MergeItem {
                    tri: item.0,
                    run_idx: i,
                });
                current_items[i] = Some(item);
            }
        }
        let mut out = BufWriter::new(File::create(out_path)?);
        let mut current_tri: Option<Trigram> = None;
        let mut merged_entries: Vec<PostingEntry> = Vec::new();
        while let Some(MergeItem { tri, run_idx }) = heap.pop() {
            if Some(tri) != current_tri {
                if let Some(t) = current_tri {
                    Self::write_run_entry(&mut out, t, &mut merged_entries)?;
                    merged_entries.clear();
                }
                current_tri = Some(tri);
            }
            let item = current_items[run_idx].take().ok_or_else(|| {
                Error::Config(format!(
                    "merge invariant broken: no item at run index {run_idx}"
                ))
            })?;
            merged_entries.extend(item.1);
            if let Some(next_item) = runs[run_idx].next_trigram()? {
                heap.push(MergeItem {
                    tri: next_item.0,
                    run_idx,
                });
                current_items[run_idx] = Some(next_item);
            }
        }
        if let Some(t) = current_tri {
            Self::write_run_entry(&mut out, t, &mut merged_entries)?;
        }
        out.flush()?;
        Ok(())
    }

    fn write_run_entry<W: Write>(
        w: &mut W,
        tri: Trigram,
        entries: &mut [PostingEntry],
    ) -> Result<()> {
        entries.sort_by_key(|e| e.file_id);
        w.write_all(&tri.to_le_bytes())?;
        w.write_all(&(entries.len() as u32).to_le_bytes())?;
        for entry in entries {
            w.write_all(&entry.file_id.to_le_bytes())?;
            w.write_all(&(entry.offsets.len() as u32).to_le_bytes())?;
            for off in &entry.offsets {
                w.write_all(&off.to_le_bytes())?;
            }
        }
        Ok(())
    }

    fn write_merged_posting<W: Write + Seek>(
        f: &mut W,
        tri: Trigram,
        base_off: u64,
        entries: &mut [PostingEntry],
    ) -> Result<(Trigram, u64, u32, u32)> {
        entries.sort_by_key(|e| e.file_id);
        let count = entries.len() as u32;
        let list = PostingList {
            entries: entries.to_vec(),
        };
        let encoded = list.encode()?;
        let offset = f.stream_position()? - base_off;
        f.write_all(&encoded)?;
        let abs_off = base_off + offset;
        Ok((tri, abs_off, encoded.len() as u32, count))
    }

    fn write_cdx_blocks<W: Write + Seek>(
        f: &mut W,
        cdx_entries: &[(Trigram, u64, u32, u32)],
    ) -> Result<Vec<(u32, u64)>> {
        let mut block_index = Vec::new();
        for chunk in cdx_entries.chunks(crate::format::CDX_BLOCK_SIZE) {
            let first_key = chunk[0].0;
            let block_offset = f.stream_position()?;
            block_index.push((first_key, block_offset));

            let mut buf = Vec::new();
            varint::encode(u64::try_from(chunk.len()).unwrap_or(0), &mut buf);
            let mut last_key = 0u32;
            for &(tri, posting_offset, posting_length, doc_frequency) in chunk {
                varint::encode(u64::from(tri - last_key), &mut buf);
                last_key = tri;
                varint::encode(posting_offset, &mut buf);
                varint::encode(u64::from(posting_length), &mut buf);
                varint::encode(u64::from(doc_frequency), &mut buf);
            }

            let compressed = zstd::encode_all(&buf[..], PostingList::ZSTD_COMPRESSION_LEVEL)
                .map_err(|e| Error::Config(format!("cdx zstd encode: {e}")))?;
            f.write_all(&compressed)?;
        }
        Ok(block_index)
    }

    fn align_to_8<W: Write + Seek>(mut w: W) -> std::io::Result<u64> {
        let pos = w.stream_position()?;
        let padding = (8 - (pos % 8)) % 8;
        if padding > 0 {
            w.write_all(&vec![0u8; padding as usize])?;
        }
        w.stream_position()
    }
}

impl Drop for Builder {
    fn drop(&mut self) {
        if !self.committed {
            self.cleanup_temp_files();
        }
    }
}
