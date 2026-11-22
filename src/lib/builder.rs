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
struct FileIndexInput<'a> {
    path: &'a Path,
    path_display: &'a str,
    data: &'a [u8],
    file_id: u32,
    mtime: u64,
    size: u64,
    content_hash: u64,
}

#[cfg(unix)]
use libc;
use llmosafe::ResourceGuard;
use memmap2::Mmap;
use std::collections::{BinaryHeap, HashMap, HashSet};
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
    /// Number of files skipped due to permission or I/O errors.
    pub files_skipped_error: u64,
    /// Total number of raw bytes scanned across all files.
    pub bytes_scanned: u64,
    /// Number of distinct trigrams discovered across all files.
    pub unique_trigrams: u64,
}

/// RAII guard that removes a temporary file on drop unless disarmed after commit.
struct TempFileGuard {
    path: PathBuf,
    armed: bool,
}

impl TempFileGuard {
    const fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    const fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if self.armed
            && let Err(e) = fs::remove_file(&self.path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                "Failed to cleanup guarded temp file {}: {e}",
                self.path.display()
            );
        }
    }
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

#[cfg(unix)]
trait StatvfsToU64 {
    fn to_u64(self) -> u64;
}

#[cfg(unix)]
impl StatvfsToU64 for u64 {
    fn to_u64(self) -> u64 {
        self
    }
}

#[cfg(unix)]
impl StatvfsToU64 for u32 {
    fn to_u64(self) -> u64 {
        u64::from(self)
    }
}

#[cfg(unix)]
impl StatvfsToU64 for i64 {
    fn to_u64(self) -> u64 {
        u64::try_from(self).unwrap_or(0)
    }
}

#[cfg(unix)]
impl StatvfsToU64 for i32 {
    fn to_u64(self) -> u64 {
        u64::try_from(self).unwrap_or(0)
    }
}

fn glob_matches(pattern: &str, text: &str) -> bool {
    let p = pattern.as_bytes();
    let t = text.as_bytes();
    let (mut pat_idx, mut text_idx) = (0usize, 0usize);
    let mut star_pat: Option<usize> = None;
    let mut backtrack_text = 0usize;

    while text_idx < t.len() {
        let pb = p.get(pat_idx).copied();
        let tb = t.get(text_idx).copied();
        if pb == Some(b'*') {
            star_pat = Some(pat_idx);
            backtrack_text = text_idx;
            pat_idx += 1;
        } else if pb == Some(b'?') || (pb.is_some() && pb == tb) {
            pat_idx += 1;
            text_idx += 1;
        } else if let Some(star_pos) = star_pat {
            pat_idx = star_pos + 1;
            backtrack_text += 1;
            text_idx = backtrack_text;
        } else {
            return false;
        }
    }
    while p.get(pat_idx).copied() == Some(b'*') {
        pat_idx += 1;
    }
    pat_idx == p.len()
}

fn matches_exclude_pattern(
    pattern: &str,
    component_name: &str,
    rel_prefix: &str,
    is_component_dir: bool,
) -> bool {
    let is_dir_only = pattern.ends_with('/') || pattern.ends_with('\\');
    let trimmed = pattern.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return false;
    }
    if is_dir_only && !is_component_dir {
        return false;
    }

    let has_glob = trimmed.contains('*') || trimmed.contains('?');
    if trimmed.contains('/') || trimmed.contains('\\') {
        let norm_pat = trimmed.replace('\\', "/");
        if has_glob {
            glob_matches(&norm_pat, rel_prefix)
        } else {
            rel_prefix == norm_pat
        }
    } else if has_glob {
        glob_matches(trimmed, component_name)
    } else {
        is_component_dir && component_name == trimmed
    }
}

/// Core path admission policy shared across full builds and incremental updates.
///
/// Evaluates an entire path from root to leaf against the active exclusion rules.
///
/// # Filtering rules (in order)
///
/// 1. **`watch_roots`** (`&[PathBuf]`) — if non-empty, files whose path does not
///    start with any watch root are excluded (directories are always traversed
///    so that children under watch roots are reachable).
/// 2. **Directory exclusions** — `lost+found`, `.git`, `.ix`, `.codegraph`, and
///    `exclude_patterns` (`&[String]`, supporting directory names, trailing-slash
///    directory names like `build/`, and `*`/`?` globs like `*.pyc`).
/// 3. **Index-internal files** — `shard.ix`, `shard.ix.tmp`, `shard.ix.*`.
/// 4. **Binary extensions** — `.so`, `.o`, `.dylib`, `.a`, `.dll`, `.exe`,
///    `.pyc`, `.jpg`, `.png`, `.gif`, `.mp4`, `.mp3`, `.pdf`, `.zip`, `.7z`,
///    `.rar`, `.sqlite`, `.db`, `.bin`, `*.tar.gz`.
#[must_use]
pub(crate) fn is_path_admitted(
    path: &Path,
    is_dir: bool,
    root: Option<&Path>,
    exclude_patterns: &[String],
    watch_roots: &[PathBuf],
) -> bool {
    if !watch_roots.is_empty() {
        let mut in_scope = false;
        for wr in watch_roots {
            if path.starts_with(wr) || wr.starts_with(path) {
                in_scope = true;
                break;
            }
        }
        if !in_scope {
            return false;
        }
    }

    let rel_path = if let Some(r) = root {
        match path.strip_prefix(r) {
            Ok(p) => p,
            Err(_) => return false, // Path outside root is excluded
        }
    } else {
        path
    };
    let components: Vec<_> = rel_path.components().collect();
    let mut rel_prefix = String::new();

    for (i, component) in components.iter().enumerate() {
        if let std::path::Component::Normal(os_str) = component {
            let name = os_str.to_string_lossy();
            if !rel_prefix.is_empty() {
                rel_prefix.push('/');
            }
            rel_prefix.push_str(&name);
            let is_component_dir = i < components.len() - 1 || is_dir;

            if is_component_dir
                && (name == "lost+found" || name == ".git" || name == ".ix" || name == ".codegraph")
            {
                return false;
            }

            if exclude_patterns
                .iter()
                .any(|p| matches_exclude_pattern(p, &name, &rel_prefix, is_component_dir))
            {
                return false;
            }

            if !is_component_dir {
                if name == "shard.ix" || name == "shard.ix.tmp" || name.starts_with("shard.ix.") {
                    return false;
                }

                let ext = Path::new(name.as_ref())
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                match ext {
                    "so" | "o" | "dylib" | "a" | "dll" | "exe" | "pyc" | "jpg" | "png" | "gif"
                    | "mp4" | "mp3" | "pdf" | "zip" | "7z" | "rar" | "sqlite" | "db" | "bin" => {
                        return false;
                    }
                    _ => {}
                }
                if name.ends_with(".tar.gz") {
                    return false;
                }
            }
        }
    }

    true
}

/// Default directory-entry filter shared across the builder walker and the
/// watcher fallback walk.
///
/// Returns `true` if the entry should be **included** in the index/walk.
#[must_use]
pub(crate) fn default_filter_entry(
    entry: &ignore::DirEntry,
    root: &Path,
    exclude_patterns: &[String],
    watch_roots: &[PathBuf],
) -> bool {
    let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
    is_path_admitted(
        entry.path(),
        is_dir,
        Some(root),
        exclude_patterns,
        watch_roots,
    )
}

type CdxEntryTuple = (Trigram, u64, u32, u32);
type MergedRunOutput = (Vec<CdxEntryTuple>, u32);

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
    /// are indexed. Relative paths are resolved against the builder's root
    /// directory (with canonicalization fallback matching `Watcher`).
    /// An empty set (default) indexes all files under the root.
    #[must_use]
    pub fn with_watch_roots(mut self, roots: Vec<PathBuf>) -> Self {
        let canonical_root = self.root.canonicalize().ok();
        let mut normalized = Vec::with_capacity(roots.len());
        for wr in roots {
            let joined = if wr.is_relative() {
                self.root.join(&wr)
            } else {
                wr
            };
            if let Ok(canon) = joined.canonicalize() {
                if canon != joined {
                    normalized.push(joined);
                }
                normalized.push(canon);
            } else {
                if let Some(ref cr) = canonical_root
                    && let Ok(rel) = joined.strip_prefix(&self.root)
                {
                    let cr_joined = cr.join(rel);
                    if cr_joined != joined {
                        normalized.push(cr_joined);
                    }
                }
                normalized.push(joined);
            }
        }
        self.watch_roots = normalized;
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

    fn check_disk_space(&self) -> Result<()> {
        // Peak overhead is ~3x existing shard (old + tmp + bak + runs).
        // For first builds, use a 200 MB floor.
        if let Some(free) = Self::free_bytes_at(&self.ix_dir) {
            let existing_shard_size =
                fs::metadata(self.ix_dir.join("shard.ix")).map_or(0, |m| m.len());
            let required = if existing_shard_size > 0 {
                existing_shard_size.saturating_mul(3)
            } else {
                200 * 1024 * 1024
            };
            if free < required {
                return Err(Error::Io(std::io::Error::other(format!(
                    "insufficient disk space: {} MB free, need >= {} MB (path: {})",
                    free / 1024 / 1024,
                    required / 1024 / 1024,
                    self.ix_dir.display(),
                ))));
            }
        }
        Ok(())
    }

    fn maybe_flush_on_pressure(&mut self, files_processed: u64) -> Result<()> {
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
            let sys_mem = crate::cache_policy::system_memory_bytes();
            let rss_threshold: u64 = if sys_mem > 0 {
                u64::try_from(sys_mem.saturating_mul(4) / 5).unwrap_or(u64::MAX)
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

        self.check_disk_space()?;

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
                let root_clone = root.clone();
                move |entry| {
                    default_filter_entry(entry, &root_clone, &exclude_patterns, &watch_roots)
                }
            })
            .build();

        let mut files_processed = 0u64;
        let mut files_since_guard_check = 0u64;
        for entry_res in walker {
            let entry = match entry_res {
                Ok(e) => e,
                Err(e) => {
                    self.record_backtrack_dead_end(&e);
                    continue;
                }
            };

            if entry.file_type().is_some_and(|t| t.is_file()) {
                self.process_file(entry.path())?;
                files_processed += 1;
                files_since_guard_check += 1;

                if files_since_guard_check >= 250 {
                    files_since_guard_check = 0;
                    self.maybe_flush_on_pressure(files_processed)?;
                }
            }
        }

        let output_path = self.serialize()?;
        tracing::info!("Build completed in {:?}: {:?}", start.elapsed(), self.stats);
        Ok(output_path)
    }

    fn record_backtrack_dead_end(&mut self, e: &ignore::Error) {
        let backtrack_path = match e {
            ignore::Error::Io(io_err) if io_err.raw_os_error() == Some(-7) => Some(None),
            ignore::Error::WithPath { path, err } => {
                if let ignore::Error::Io(io_err) = err.as_ref()
                    && io_err.raw_os_error() == Some(-7)
                {
                    Some(Some(path.clone()))
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some(path_opt) = backtrack_path {
            tracing::warn!("Immune Memory Triggered: Skipping path due to backtrack signal.");
            if let Some(path) = path_opt {
                self.dead_ends.push(path);
            }
        }
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

        let main_reader = match Reader::open(&index_file) {
            Ok(r) => r,
            Err(Error::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return self.build();
            }
            Err(Error::MissingIndex(_)) => return self.build(),
            Err(e) => return Err(e),
        };

        let delta_reader = DeltaReader::open(&delta_file)?;
        let mut next_file_id = main_reader
            .header
            .file_count
            .checked_add(delta_reader.total_file_entries)
            .ok_or_else(|| Error::Config("delta file_id overflow".into()))?;

        let mut delta_tmp_guard = TempFileGuard::new(delta_tmp.clone());
        let is_new = !delta_file.exists();
        let mut delta_out = if is_new {
            let mut f = File::create(&delta_tmp)?;
            f.write_all(&DELTA_MAGIC)?;
            f
        } else {
            std::fs::copy(&delta_file, &delta_tmp)?;
            OpenOptions::new().append(true).open(&delta_tmp)?
        };

        let tombstones = &delta_reader.tombstones;
        let mut path_to_id: HashMap<PathBuf, u32> = delta_reader
            .path_to_id
            .into_iter()
            .filter(|(_, fid)| !tombstones.contains(fid))
            .collect();
        for file_id in 0..main_reader.header.file_count {
            if tombstones.contains(&file_id) {
                continue;
            }
            let info = main_reader.get_file(file_id)?;
            path_to_id.entry(info.path).or_insert(file_id);
        }

        let mut seen_paths = HashSet::with_capacity(changed_files.len());
        for path in changed_files {
            if !seen_paths.insert(path.as_path()) {
                continue;
            }

            let meta_opt = match fs::symlink_metadata(path) {
                Ok(m) => Some(m),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    self.stats.files_skipped_error += 1;
                    tracing::warn!("SKIP: permission denied for {}", path.display());
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let is_dir = meta_opt.as_ref().is_some_and(|m| m.file_type().is_dir());
            if !crate::builder::is_path_admitted(
                path,
                is_dir,
                Some(&self.root),
                &self.exclude_patterns,
                &self.watch_roots,
            ) {
                continue;
            }

            if let Some(old_id) = path_to_id.remove(path) {
                delta_out.write_all(&[DELTA_TOMBSTONE])?;
                delta_out.write_all(&old_id.to_le_bytes())?;
            } else if meta_opt.is_none() {
                // Deleted or renamed directory: tombstone every live descendant path.
                let descendants: Vec<PathBuf> = path_to_id
                    .keys()
                    .filter(|p| p.starts_with(path))
                    .cloned()
                    .collect();
                for desc in descendants {
                    if let Some(old_id) = path_to_id.remove(&desc) {
                        delta_out.write_all(&[DELTA_TOMBSTONE])?;
                        delta_out.write_all(&old_id.to_le_bytes())?;
                    }
                }
            }

            if let Some(meta) = meta_opt
                && meta.file_type().is_file()
                && self.process_file_delta(path, next_file_id, &mut delta_out)?
            {
                path_to_id.insert(path.clone(), next_file_id);
                next_file_id = next_file_id
                    .checked_add(1)
                    .ok_or_else(|| Error::Config("delta file_id overflow".into()))?;
            }
        }

        delta_out.flush()?;
        std::fs::rename(&delta_tmp, &delta_file)?;
        delta_tmp_guard.disarm();
        self.file_count = u32::try_from(path_to_id.len()).unwrap_or(u32::MAX);
        Ok(index_file)
    }

    /// Number of files indexed so far (snapshot for progress reporting).
    #[must_use]
    pub fn files_len(&self) -> usize {
        usize::try_from(self.file_count).unwrap_or(usize::MAX)
    }

    /// Number of unique trigrams accumulated so far.
    #[must_use]
    pub fn trigrams_len(&self) -> usize {
        usize::try_from(self.stats.unique_trigrams).unwrap_or(usize::MAX)
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
    fn free_bytes_at(path: &Path) -> Option<u64> {
        use std::os::unix::ffi::OsStrExt;
        let path_c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: libc::statvfs only requires a pointer to an initialized
        // statvfs struct. std::mem::zeroed() produces a valid zeroed struct
        // on Linux. path_c is a null-terminated CString with valid UTF-8
        // content.
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: stat is a valid, initialized statvfs struct. path_c is a
        // null-terminated CString.
        let ret = unsafe { libc::statvfs(path_c.as_ptr(), &raw mut stat) };
        if ret != 0 {
            return None;
        }
        Some(
            stat.f_bavail
                .to_u64()
                .saturating_mul(stat.f_frsize.to_u64()),
        )
    }

    /// Windows: skip the upfront disk check (no `statvfs`).
    #[cfg(not(unix))]
    const fn free_bytes_at(_path: &Path) -> Option<u64> {
        None
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
        let metadata = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        if !metadata.file_type().is_file() {
            return Ok(false);
        }
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
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                self.stats.files_skipped_error += 1;
                tracing::warn!("SKIP: permission denied for {path_str}");
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
                std::borrow::Cow::Borrowed(mmap.as_ref())
            }
        } else {
            std::borrow::Cow::Borrowed(mmap.as_ref())
        };

        let data = raw_data.as_ref();
        if is_binary(data) {
            tracing::debug!("SKIP: binary detection ({size} bytes): {path_str}");
            self.stats.files_skipped_binary += 1;
            return Ok(false);
        }

        let content_hash = xxhash_rust::xxh64::xxh64(data, 0);
        let file_id = self.file_count;
        self.file_count += 1;

        let trigram_count = self.index_file_trigrams_and_write_records(&FileIndexInput {
            path,
            path_display: &path_str,
            data,
            file_id,
            mtime,
            size,
            content_hash,
        })?;

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

    fn index_file_trigrams_and_write_records(&mut self, input: &FileIndexInput<'_>) -> Result<u32> {
        let path = input.path;
        let path_display = input.path_display;
        let data = input.data;
        let file_id = input.file_id;
        let mtime = input.mtime;
        let size = input.size;
        let content_hash = input.content_hash;
        let pairs = self.extractor.extract_with_offsets(data);

        if pairs.is_empty() && !data.is_empty() {
            tracing::debug!("SKIP: no trigrams extracted ({size} bytes): {path_display}");
        }

        let path_str = path.to_str().ok_or_else(|| {
            tracing::warn!(
                "SKIP: non-UTF-8 path cannot be indexed cleanly: {}",
                path.display()
            );
            Error::InvalidPath
        })?;
        let path_bytes = path_str.as_bytes();
        let path_len = u16::try_from(path_bytes.len()).map_err(|_| {
            Error::Config(format!("path exceeds maximum length: {}", path_bytes.len()))
        })?;

        let strings_writer = Self::get_writer(&mut self.strings_writer, "strings_writer")?;
        let path_off = u32::try_from(strings_writer.stream_position()?)
            .map_err(|e| Error::Io(std::io::Error::other(e)))?;
        strings_writer.write_all(&0u16.to_le_bytes())?;
        strings_writer.write_all(&path_len.to_le_bytes())?;
        strings_writer.write_all(path_bytes)?;

        let mut bloom = BloomFilter::new(256, 5);
        let mut trigram_count = 0u32;

        for group in pairs.chunk_by(|a, b| a.0 == b.0) {
            let Some(&(tri, _)) = group.first() else {
                continue;
            };
            bloom.insert(tri);
            trigram_count += 1;

            for chunk in group.chunks(10_000) {
                let offsets: Vec<u32> = chunk.iter().map(|p| p.1).collect();
                let take = chunk.len();
                self.postings
                    .entry(tri)
                    .or_default()
                    .push(PostingEntry { file_id, offsets });
                self.postings_count += take + 8;
            }
        }

        bloom.serialize(Self::get_writer(&mut self.blooms_writer, "blooms_writer")?)?;

        let bloom_offset = u64::from(file_id)
            .checked_mul(260)
            .ok_or_else(|| Error::Config("file_id * 260 overflow".into()))?;
        let bloom_offset_u32 = u32::try_from(bloom_offset)
            .map_err(|_| Error::Config(format!("bloom_offset {bloom_offset} exceeds u32::MAX")))?;

        let files_writer = Self::get_writer(&mut self.files_writer, "files_writer")?;
        files_writer.write_all(&file_id.to_le_bytes())?;
        files_writer.write_all(&path_off.to_le_bytes())?;
        files_writer.write_all(&path_len.to_le_bytes())?;
        files_writer.write_all(&[FileStatus::Fresh as u8])?;
        files_writer.write_all(&[0u8])?;
        files_writer.write_all(&mtime.to_le_bytes())?;
        files_writer.write_all(&size.to_le_bytes())?;
        files_writer.write_all(&content_hash.to_le_bytes())?;
        files_writer.write_all(&trigram_count.to_le_bytes())?;
        files_writer.write_all(&bloom_offset_u32.to_le_bytes())?;
        files_writer.write_all(&[0u8; 4])?;

        Ok(trigram_count)
    }

    fn process_file_delta<W: std::io::Write>(
        &mut self,
        path: &Path,
        file_id: u32,
        delta: &mut W,
    ) -> Result<bool> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                self.stats.files_skipped_error += 1;
                tracing::warn!("SKIP: permission denied for {}", path.display());
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        };
        if !metadata.file_type().is_file() {
            return Ok(false);
        }

        let size = metadata.len();
        let mtime = metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(0));

        if self.max_file_size > 0 && size > self.max_file_size {
            return Ok(false);
        }

        let file = match fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                self.stats.files_skipped_error += 1;
                tracing::warn!("SKIP: permission denied for {}", path.display());
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        };
        if !file.metadata()?.file_type().is_file() {
            return Ok(false);
        }

        // SAFETY: The file is held open via `file` for the duration of this
        // method. The file system will not truncate the file while we hold
        // the open handle. The mmap region covers the entire file as mapped.
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        let data = mmap.as_ref();
        if crate::format::is_binary(data) {
            return Ok(false);
        }

        let content_hash = xxhash_rust::xxh64::xxh64(data, 0);
        let pairs = self.extractor.extract_with_offsets(data);
        Self::write_delta_entries(path, file_id, mtime, size, content_hash, pairs, delta)?;
        Ok(true)
    }

    fn write_delta_entries<W: std::io::Write>(
        path: &Path,
        file_id: u32,
        mtime: u64,
        size: u64,
        content_hash: u64,
        pairs: &[(Trigram, u32)],
        delta: &mut W,
    ) -> Result<()> {
        use crate::format::{DELTA_FILE_ENTRY, DELTA_TRIGRAM_ENTRY};

        let mut bloom = BloomFilter::new(256, 5);
        let mut trigram_entries: Vec<(Trigram, Vec<u32>)> = Vec::new();
        for group in pairs.chunk_by(|a, b| a.0 == b.0) {
            let Some(&(tri, _)) = group.first() else {
                continue;
            };
            let offsets: Vec<u32> = group.iter().map(|p| p.1).collect();
            bloom.insert(tri);
            trigram_entries.push((tri, offsets));
        }

        let path_str = path.to_str().ok_or_else(|| {
            tracing::warn!(
                "SKIP: non-UTF-8 path cannot be indexed cleanly: {}",
                path.display()
            );
            Error::InvalidPath
        })?;
        let path_bytes = path_str.as_bytes();
        let path_len_u16 = u16::try_from(path_bytes.len()).map_err(|_| {
            Error::Config(format!("path exceeds maximum length: {}", path_bytes.len()))
        })?;
        delta.write_all(&[DELTA_FILE_ENTRY])?;
        delta.write_all(&file_id.to_le_bytes())?;
        delta.write_all(&path_len_u16.to_le_bytes())?;
        delta.write_all(path_bytes)?;
        delta.write_all(&mtime.to_le_bytes())?;
        delta.write_all(&size.to_le_bytes())?;
        delta.write_all(&content_hash.to_le_bytes())?;
        let mut bloom_buf = Vec::new();
        bloom.serialize(&mut bloom_buf)?;
        delta.write_all(&bloom_buf)?;

        for (tri, offsets) in trigram_entries {
            let off_count = u32::try_from(offsets.len())
                .map_err(|_| Error::Config("delta offset count overflow".into()))?;
            delta.write_all(&[DELTA_TRIGRAM_ENTRY])?;
            delta.write_all(&tri.to_le_bytes())?;
            delta.write_all(&1u32.to_le_bytes())?;
            delta.write_all(&file_id.to_le_bytes())?;
            delta.write_all(&off_count.to_le_bytes())?;
            for off in offsets {
                delta.write_all(&off.to_le_bytes())?;
            }
        }
        Ok(())
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
                let mut merged_guard = TempFileGuard::new(out_path.clone());
                Self::merge_to_run(chunk, &out_path)?;
                merged_guard.disarm();
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
        self.write_shard_file(&tmp_path)?;
        self.swap_shard_atomically(&tmp_path, &final_path)?;

        for name in [
            "shard.ix.tmp.files",
            "shard.ix.tmp.blooms",
            "shard.ix.tmp.strings",
        ] {
            if let Err(e) = fs::remove_file(self.ix_dir.join(name)) {
                tracing::warn!("failed to clean temp file {name}: {e}");
            }
        }
        for path in &self.temp_runs {
            if let Err(e) = fs::remove_file(path) {
                tracing::warn!("Failed to cleanup temp run {}: {}", path.display(), e);
            }
        }
        self.temp_runs.clear();

        Ok(final_path)
    }

    fn merge_runs_into_shard<W: Write + Seek>(
        temp_runs: &[PathBuf],
        f: &mut W,
        posting_data_offset: u64,
    ) -> Result<MergedRunOutput> {
        let mut cdx_entries: Vec<CdxEntryTuple> = Vec::new();
        let mut global_trigram_count = 0u32;

        let mut runs = Vec::new();
        for path in temp_runs {
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
                if let Some(slot) = current_items.get_mut(i) {
                    *slot = Some(item);
                }
            }
        }

        let mut current_tri: Option<Trigram> = None;
        let mut merged_entries: Vec<PostingEntry> = Vec::new();

        while let Some(MergeItem { tri, run_idx }) = heap.pop() {
            if Some(tri) != current_tri {
                if let Some(t) = current_tri {
                    let cdx_entry =
                        Self::write_merged_posting(f, t, posting_data_offset, &mut merged_entries)?;
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
                Self::write_merged_posting(f, t, posting_data_offset, &mut merged_entries)?;
            cdx_entries.push(cdx_entry);
            global_trigram_count += 1;
        }

        Ok((cdx_entries, global_trigram_count))
    }

    fn write_shard_file(&mut self, tmp_path: &Path) -> Result<()> {
        let mut f = BufWriter::new(File::create(tmp_path)?);
        f.write_all(&[0u8; HEADER_SIZE])?;

        let file_table_offset = Self::align_to_8(&mut f)?;
        let mut files_reader = File::open(self.ix_dir.join("shard.ix.tmp.files"))?;
        std::io::copy(&mut files_reader, &mut f)?;
        let file_table_size = f.stream_position()? - file_table_offset;

        Self::align_to_8(&mut f)?;
        let posting_data_offset = f.stream_position()?;

        let (cdx_entries, global_trigram_count) =
            Self::merge_runs_into_shard(&self.temp_runs, &mut f, posting_data_offset)?;
        self.stats.unique_trigrams = u64::from(global_trigram_count);
        let posting_data_size = f.stream_position()? - posting_data_offset;

        Self::align_to_8(&mut f)?;
        let trigram_table_offset = f.stream_position()?;
        let cdx_block_index = Self::write_cdx_blocks(&mut f, &cdx_entries)?;
        let trigram_table_size = f.stream_position()? - trigram_table_offset;

        Self::align_to_8(&mut f)?;
        let cdx_block_index_offset = f.stream_position()?;
        for entry in &cdx_block_index {
            f.write_all(&entry.0.to_le_bytes())?;
            f.write_all(&entry.1.to_le_bytes())?;
        }
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
        let created_at = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| Error::Config(format!("time went backwards: {e}")))?
                .as_micros(),
        )
        .unwrap_or(0);

        let header_flags = flags::HAS_BLOOM_FILTERS
            | flags::HAS_CONTENT_HASHES
            | flags::POSTING_LISTS_COMPRESSED
            | flags::POSTING_LISTS_CHECKSUMMED
            | flags::HAS_CDX_INDEX;
        let mut header_bytes = [0u8; HEADER_SIZE];
        let write_slice = |buf: &mut [u8], off: usize, src: &[u8]| {
            if let Some(dst) = buf.get_mut(off..off + src.len()) {
                dst.copy_from_slice(src);
            }
        };
        write_slice(&mut header_bytes, 0x00, &MAGIC);
        write_slice(&mut header_bytes, 0x04, &VERSION_MAJOR.to_le_bytes());
        write_slice(&mut header_bytes, 0x06, &VERSION_MINOR.to_le_bytes());
        write_slice(&mut header_bytes, 0x08, &header_flags.to_le_bytes());
        write_slice(&mut header_bytes, 0x10, &created_at.to_le_bytes());
        write_slice(
            &mut header_bytes,
            0x18,
            &self.stats.bytes_scanned.to_le_bytes(),
        );
        write_slice(&mut header_bytes, 0x20, &self.file_count.to_le_bytes());
        write_slice(&mut header_bytes, 0x24, &global_trigram_count.to_le_bytes());
        write_slice(&mut header_bytes, 0x28, &file_table_offset.to_le_bytes());
        write_slice(&mut header_bytes, 0x30, &file_table_size.to_le_bytes());
        write_slice(&mut header_bytes, 0x38, &trigram_table_offset.to_le_bytes());
        write_slice(&mut header_bytes, 0x40, &trigram_table_size.to_le_bytes());
        write_slice(&mut header_bytes, 0x48, &posting_data_offset.to_le_bytes());
        write_slice(&mut header_bytes, 0x50, &posting_data_size.to_le_bytes());
        write_slice(&mut header_bytes, 0x58, &bloom_offset.to_le_bytes());
        write_slice(&mut header_bytes, 0x60, &bloom_size.to_le_bytes());
        write_slice(&mut header_bytes, 0x68, &string_pool_offset.to_le_bytes());
        write_slice(&mut header_bytes, 0x70, &string_pool_size.to_le_bytes());
        write_slice(&mut header_bytes, 0x78, &name_index_offset.to_le_bytes());
        write_slice(&mut header_bytes, 0x80, &0u64.to_le_bytes());
        write_slice(
            &mut header_bytes,
            0x88,
            &cdx_block_index_offset.to_le_bytes(),
        );
        write_slice(&mut header_bytes, 0x90, &cdx_block_index_size.to_le_bytes());

        let crc = crc32c::crc32c(header_bytes.get(0..0xF8).unwrap_or(&[]));
        write_slice(&mut header_bytes, 0xF8, &crc.to_le_bytes());

        f.seek(SeekFrom::Start(0))?;
        f.write_all(&header_bytes)?;
        f.flush()?;
        Ok(())
    }

    fn swap_shard_atomically(&mut self, tmp_path: &Path, final_path: &Path) -> Result<()> {
        // Backup old shard BEFORE atomic rename without ever unlinking or moving
        // `shard.ix` out of place first (BLD-2).
        let backup_path = self.ix_dir.join("shard.ix.bak");
        let old_index_exists = final_path.exists();
        if old_index_exists {
            if let Err(e) = fs::remove_file(&backup_path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("failed to clean old backup file: {e}");
            }
            fs::hard_link(final_path, &backup_path)
                .or_else(|_| fs::copy(final_path, &backup_path).map(|_| ()))
                .map_err(|e| {
                    Error::Io(std::io::Error::other(format!(
                        "failed to backup existing index: {e}"
                    )))
                })?;
        }

        fs::rename(tmp_path, final_path)?;
        self.committed = true;

        // Clean delta files right with the swap so new shard.ix never pairs
        // with an old-generation delta.
        for delta_name in ["shard.ix.delta", "shard.ix.delta.tmp"] {
            if let Err(e) = fs::remove_file(self.ix_dir.join(delta_name))
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("failed to clean {delta_name}: {e}");
            }
        }

        if old_index_exists
            && let Err(e) = fs::remove_file(&backup_path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("failed to clean backup file: {e}");
        }
        Ok(())
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
                if let Some(slot) = current_items.get_mut(i) {
                    *slot = Some(item);
                }
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
            let item = current_items
                .get_mut(run_idx)
                .ok_or_else(|| Error::Config(format!("run_idx {run_idx} out of bounds")))?
                .take()
                .ok_or_else(|| {
                    Error::Config(format!(
                        "merge invariant broken: no item at run index {run_idx}"
                    ))
                })?;
            merged_entries.extend(item.1);
            if let Some(next_item) = runs
                .get_mut(run_idx)
                .ok_or_else(|| Error::Config(format!("run_idx {run_idx} out of bounds")))?
                .next_trigram()?
            {
                heap.push(MergeItem {
                    tri: next_item.0,
                    run_idx,
                });
                *current_items
                    .get_mut(run_idx)
                    .ok_or_else(|| Error::Config(format!("run_idx {run_idx} out of bounds")))? =
                    Some(next_item);
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
        let entry_count = u32::try_from(entries.len())
            .map_err(|_| Error::Config("run entry count overflow".into()))?;
        w.write_all(&tri.to_le_bytes())?;
        w.write_all(&entry_count.to_le_bytes())?;
        for entry in entries {
            let off_count = u32::try_from(entry.offsets.len())
                .map_err(|_| Error::Config("run offset count overflow".into()))?;
            w.write_all(&entry.file_id.to_le_bytes())?;
            w.write_all(&off_count.to_le_bytes())?;
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
        let count = u32::try_from(entries.len())
            .map_err(|_| Error::Config("merged posting count overflow".into()))?;
        let list = PostingList {
            entries: entries.to_vec(),
        };
        let encoded = list.encode()?;
        let encoded_len = u32::try_from(encoded.len())
            .map_err(|_| Error::Config("encoded posting length overflow".into()))?;
        let offset = f.stream_position()? - base_off;
        f.write_all(&encoded)?;
        let abs_off = base_off + offset;
        Ok((tri, abs_off, encoded_len, count))
    }

    fn write_cdx_blocks<W: Write + Seek>(
        f: &mut W,
        cdx_entries: &[(Trigram, u64, u32, u32)],
    ) -> Result<Vec<(u32, u64)>> {
        let mut block_index = Vec::new();
        for chunk in cdx_entries.chunks(crate::format::CDX_BLOCK_SIZE) {
            let Some(&(first_key, _, _, _)) = chunk.first() else {
                continue;
            };
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

            let compressed = zstd::encode_all(buf.as_slice(), PostingList::ZSTD_COMPRESSION_LEVEL)
                .map_err(|e| Error::Config(format!("cdx zstd encode: {e}")))?;
            f.write_all(&compressed)?;
        }
        Ok(block_index)
    }

    fn align_to_8<W: Write + Seek>(mut w: W) -> std::io::Result<u64> {
        let pos = w.stream_position()?;
        let padding = usize::try_from((8 - (pos % 8)) % 8).unwrap_or(0);
        if padding > 0 {
            let zeros = [0u8; 8];
            w.write_all(zeros.get(..padding).unwrap_or(&[]))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::DeltaReader;

    #[test]
    fn test_update_deduplicates_and_updates_path_to_id() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let file_a = root.join("a.txt");
        fs::write(&file_a, "initial needle\n").unwrap();

        let mut builder = Builder::new(root).unwrap();
        builder.build().unwrap();

        fs::write(&file_a, "updated needle\n").unwrap();
        builder.update(&[file_a.clone(), file_a]).unwrap();

        let delta = DeltaReader::open(&root.join(".ix").join("shard.ix.delta")).unwrap();
        assert_eq!(
            delta.total_file_entries, 1,
            "duplicate paths in single update batch must emit only 1 delta file entry"
        );
        assert!(delta.tombstones.contains(&0));
        assert!(!delta.tombstones.contains(&1));
    }

    #[test]
    fn test_with_watch_roots_normalizes_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("other")).unwrap();
        fs::write(root.join("src/lib.rs"), "needle_in_src\n").unwrap();
        fs::write(root.join("other/skip.rs"), "needle_in_other\n").unwrap();

        let mut builder = Builder::new(root)
            .unwrap()
            .with_watch_roots(vec![PathBuf::from("src")]);
        let shard = builder.build().unwrap();
        let reader = crate::reader::Reader::open(&shard).unwrap();
        assert_eq!(reader.header.file_count, 1);
        assert!(reader.get_file(0).unwrap().path.ends_with("lib.rs"));
    }

    #[test]
    fn test_is_path_admitted_trailing_slash_and_glob_patterns() {
        let root = Path::new("/repo");
        let excludes = vec!["build/".to_string(), "*.pyc".to_string()];

        assert!(!is_path_admitted(
            Path::new("/repo/build/app.js"),
            false,
            Some(root),
            &excludes,
            &[]
        ));
        assert!(!is_path_admitted(
            Path::new("/repo/src/mod.pyc"),
            false,
            Some(root),
            &excludes,
            &[]
        ));
        assert!(is_path_admitted(
            Path::new("/repo/src/mod.py"),
            false,
            Some(root),
            &excludes,
            &[]
        ));
    }

    #[cfg(unix)]
    #[test]
    fn test_update_skips_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let target = root.join("real.txt");
        let link = root.join("sym.txt");
        fs::write(&target, "real content\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let mut builder = Builder::new(root).unwrap();
        builder.build().unwrap();

        builder.update(std::slice::from_ref(&link)).unwrap();
        let delta = DeltaReader::open(&root.join(".ix").join("shard.ix.delta")).unwrap();
        assert_eq!(delta.total_file_entries, 0);
    }
}
