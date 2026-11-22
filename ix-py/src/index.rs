//! `Index` class — the primary Python entry point for ix.
//!
//! Wraps an mmapped `ix::reader::Reader`, shared caches, and an optional
//! root path for discovery. Exposes `search`, `build`, `stats`,
//! `service_status`, and `close` methods matching the C-INDEX contract.

use crate::types::PySearchResult;
use ix::planner::Planner;
use ix::reader::Reader;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict};
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
#[cfg(feature = "notify")]
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Atomic snapshot pairing an open shard reader with its delta path and
/// generation fingerprint so search and metadata reads stay coherent across
/// out-of-band shard swaps.
struct ReaderSnapshot {
    reader: Arc<Reader>,
    delta_path: Option<PathBuf>,
    fingerprint: u64,
}

impl ReaderSnapshot {
    fn open(index_path: &Path) -> PyResult<Self> {
        let reader = Reader::open(index_path).map_err(|e| crate::error::to_pyerr(&e))?;
        let fingerprint = reader.shard_fingerprint();
        let delta_path = index_path.parent().map(|p| p.join("shard.ix.delta"));
        Ok(Self {
            reader: Arc::new(reader),
            delta_path,
            fingerprint,
        })
    }
}

/// An open ix index backed by a memory-mapped shard file.
///
/// Provides search, build, stats, and service-status methods.
/// Created via `Index(path)` where `path` is a directory containing
/// `.ix/shard.ix` or the index file itself.
#[pyclass(name = "Index")]
pub struct PyIndex {
    snapshot: RwLock<Arc<ReaderSnapshot>>,
    index_path: PathBuf,
    posting_cache: Arc<ix::posting_cache::PostingCache>,
    neg_cache: Arc<ix::neg_cache::NegCache>,
    regex_pool: Arc<ix::regex_pool::RegexPool>,
    resource_guard: Arc<llmosafe::ResourceGuard>,
    resource_guard_ceiling: usize,
    root: PathBuf,
    closed: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct ParsedSearchArgs {
    planner: ix::planner::QueryOptions,
    context_lines: usize,
    max_results: usize,
    type_filter: Vec<String>,
    count_only: bool,
    files_only: bool,
}

#[cfg(feature = "notify")]
struct BuildReport {
    files_scanned: u64,
    files_skipped_binary: u64,
    files_skipped_size: u64,
    bytes_scanned: u64,
    unique_trigrams: u64,
    created_at: u64,
    duration_ms: u64,
}

/// Discover the `.ix/shard.ix` file by walking upward from `path`.
///
/// # Arguments
/// * `path` - Starting directory or file path.
///
/// # Returns
/// The resolved `PathBuf` to `shard.ix`.
///
/// # Errors
/// Returns `PyErr` wrapping `IxIndexError` if no `.ix/shard.ix` is found.
fn find_index(path: &Path) -> PyResult<PathBuf> {
    let path = std::path::absolute(path)
        .map_err(|e| PyErr::new::<crate::error::IxIoError, _>(format!("invalid path: {e}")))?;

    if path.is_file() && path.file_name().is_some_and(|n| n == "shard.ix") {
        return Ok(path);
    }

    let mut current = if path.is_dir() {
        path.clone()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or(path.clone())
    };

    loop {
        let candidate = current.join(".ix").join("shard.ix");
        match candidate.try_exists() {
            Ok(true) => return Ok(candidate),
            Ok(false) => {}
            Err(e) => {
                return Err(PyErr::new::<crate::error::IxIoError, _>(format!(
                    "cannot access {}: {e}",
                    candidate.display()
                )));
            }
        }
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            return Err(PyErr::new::<crate::error::IxIndexError, _>(
                "no .ix/shard.ix found by walking up from the given path",
            ));
        }
    }
}

/// Build a Python dict with the unified build-statistics fields.
#[cfg(feature = "notify")]
fn build_stats_dict<'py>(
    py: Python<'py>,
    root: &Path,
    report: &BuildReport,
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("path", root.display().to_string())?;
    dict.set_item("files_scanned", report.files_scanned)?;
    dict.set_item("files_indexed", report.files_scanned)?;
    dict.set_item(
        "files_skipped",
        report
            .files_skipped_binary
            .saturating_add(report.files_skipped_size),
    )?;
    dict.set_item("files_skipped_binary", report.files_skipped_binary)?;
    dict.set_item("files_skipped_size", report.files_skipped_size)?;
    dict.set_item("bytes_scanned", report.bytes_scanned)?;
    dict.set_item("unique_trigrams", report.unique_trigrams)?;
    dict.set_item("trigram_count", report.unique_trigrams)?;
    dict.set_item("created_at", report.created_at)?;
    dict.set_item("duration_ms", report.duration_ms)?;
    Ok(dict)
}

#[cfg(feature = "notify")]
fn run_builder(
    root: &Path,
    max_bytes: u64,
    exclude_dirs: Option<Vec<String>>,
) -> Result<BuildReport, ix::error::Error> {
    let ix_dir = root.join(".ix");
    if let Ok(beacon) = ix::format::Beacon::read_from(&ix_dir)
        && beacon.is_live()
    {
        tracing::warn!(
            "ix-py: active ixd daemon (PID {}) is watching {}; performing out-of-band index build",
            beacon.pid,
            root.display()
        );
    }
    let started = Instant::now();
    let mut builder = ix::builder::Builder::new(root)?;
    builder.set_max_file_size(max_bytes);
    if let Some(dirs) = exclude_dirs {
        builder = builder.with_exclude_patterns(dirs);
    }
    builder.build()?;
    let s = builder.stats();
    let created_at = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros(),
    )
    .unwrap_or(u64::MAX);
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(BuildReport {
        files_scanned: s.files_scanned,
        files_skipped_binary: s.files_skipped_binary,
        files_skipped_size: s.files_skipped_size,
        bytes_scanned: s.bytes_scanned,
        unique_trigrams: s.unique_trigrams,
        created_at,
        duration_ms,
    })
}

fn parse_search_kwargs(kwds: Option<&Bound<'_, PyDict>>) -> PyResult<ParsedSearchArgs> {
    let mut parsed = ParsedSearchArgs::default();
    let Some(dict) = kwds else {
        return Ok(parsed);
    };
    for (k, v) in dict.iter() {
        let key: String = k.extract()?;
        match key.as_str() {
            "regex" => parsed.planner.is_regex = v.extract()?,
            "context_lines" => parsed.context_lines = v.extract()?,
            "max_results" => parsed.max_results = v.extract()?,
            "type_filter" => {
                let tf: Option<Vec<String>> = v.extract()?;
                parsed.type_filter = tf.map(|t| ix::file_types::expand(&t)).unwrap_or_default();
            }
            "multiline" => parsed.planner.multiline = v.extract()?,
            "case_insensitive" => parsed.planner.ignore_case = v.extract()?,
            "word_boundary" => parsed.planner.word_boundary = v.extract()?,
            "count_only" => parsed.count_only = v.extract()?,
            "files_only" => parsed.files_only = v.extract()?,
            _ => {
                return Err(pyo3::exceptions::PyTypeError::new_err(format!(
                    "search() got an unexpected keyword argument '{key}'"
                )));
            }
        }
    }
    if parsed.planner.multiline && !parsed.planner.is_regex {
        return Err(PyErr::new::<crate::error::IxConfigError, _>(
            "multiline=True requires regex=True",
        ));
    }
    Ok(parsed)
}

impl PyIndex {
    fn ensure_open(&self) -> PyResult<()> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(pyo3::exceptions::PyRuntimeError::new_err("Index is closed"));
        }
        Ok(())
    }

    fn snapshot_arc(&self) -> Arc<ReaderSnapshot> {
        let guard = self.snapshot.read().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(&guard)
    }

    fn fresh_snapshot(&self) -> PyResult<Arc<ReaderSnapshot>> {
        {
            let guard = self.snapshot.read().unwrap_or_else(PoisonError::into_inner);
            if !guard.reader.is_stale(&self.index_path) {
                return Ok(Arc::clone(&guard));
            }
        }
        let mut guard = self
            .snapshot
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if guard.reader.is_stale(&self.index_path) {
            let new_snap = ReaderSnapshot::open(&self.index_path)?;
            self.posting_cache.ensure_shard(new_snap.fingerprint);
            self.neg_cache.clear();
            self.regex_pool.clear();
            *guard = Arc::new(new_snap);
        }
        Ok(Arc::clone(&guard))
    }
}

#[pymethods]
impl PyIndex {
    /// Open an index from a root directory or shard file path.
    ///
    /// Walks upward to find `.ix/shard.ix`, opens it via mmap, and
    /// initialises shared LRU caches.
    ///
    /// # Arguments
    /// * `path` - Directory containing `.ix/` or the shard file itself.
    /// * `cache_mb` - Ceiling for the posting-list LRU cache in MB.
    ///
    /// # Errors
    /// Raises `IxIndexError` if no index is found or the file is corrupt, or
    /// `ValueError` if `cache_mb` overflows `usize`.
    #[new]
    #[pyo3(signature = (path, *, cache_mb = 64))]
    pub fn new(path: &str, cache_mb: usize) -> PyResult<Self> {
        let path = std::path::Path::new(path);
        let index_path = find_index(path)?;
        let snapshot = ReaderSnapshot::open(&index_path)?;
        let root = index_path
            .parent()
            .and_then(|p| p.parent())
            .map_or_else(|| index_path.clone(), Path::to_path_buf);
        let cache_bytes = cache_mb.checked_mul(1024 * 1024).ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err("cache_mb overflows byte capacity")
        })?;
        let posting_cache = Arc::new(ix::posting_cache::PostingCache::new(cache_bytes));
        posting_cache.ensure_shard(snapshot.fingerprint);
        let resource_guard = Arc::new(ix::cache_policy::resource_guard_auto(0.6));
        let resource_guard_ceiling = ix::cache_policy::ceiling_bytes_for_fraction(0.6);
        Ok(Self {
            snapshot: RwLock::new(Arc::new(snapshot)),
            index_path,
            posting_cache,
            neg_cache: Arc::new(ix::neg_cache::NegCache::new(65_536)),
            regex_pool: Arc::new(ix::regex_pool::RegexPool::new(256)),
            resource_guard,
            resource_guard_ceiling,
            root,
            closed: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Search the index with a literal or regex pattern.
    ///
    /// Internally calls `Planner::plan_with_pool()` then
    /// `Executor::execute()` with the GIL released so that Rayon
    /// parallel threads are not blocked. Automatically reloads stale mmaps
    /// and includes `.ix/shard.ix.delta` updates when present.
    ///
    /// # Arguments
    /// * `pattern` - Non-empty search string.
    /// * `kwds` - Optional keyword flags: `regex`, `context_lines`,
    ///   `max_results`, `type_filter`, `multiline`, `case_insensitive`,
    ///   `word_boundary`, `count_only`, `files_only`.
    ///
    /// # Returns
    /// `PySearchResult` with `matches` (list of `Match`) and `stats` (dict).
    ///
    /// # Errors
    /// Raises `RuntimeError` if closed, `TypeError` for unknown keyword
    /// arguments, or `IxIndexError` / `IxCorruptionError` / `IxRegexError`.
    #[pyo3(signature = (pattern, **kwds))]
    pub fn search(
        &self,
        py: Python<'_>,
        pattern: &str,
        kwds: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<PySearchResult> {
        self.ensure_open()?;
        let parsed = parse_search_kwargs(kwds)?;

        let plan = Planner::plan_with_pool(pattern, parsed.planner, &self.regex_pool)
            .map_err(|e| crate::error::to_pyerr(&e))?;

        let options = ix::executor::QueryOptions {
            context_lines: parsed.context_lines,
            max_results: parsed.max_results,
            type_filter: parsed.type_filter,
            count_only: parsed.count_only,
            files_only: parsed.files_only,
            multiline: parsed.planner.multiline,
            word_boundary: parsed.planner.word_boundary,
            ..Default::default()
        };

        let snap = self.fresh_snapshot()?;
        let delta_path = snap.delta_path.clone();

        let pc = Arc::clone(&self.posting_cache);
        let nc = Arc::clone(&self.neg_cache);
        let rp = Arc::clone(&self.regex_pool);
        let guard = Arc::clone(&self.resource_guard);
        let ceiling = self.resource_guard_ceiling;

        let result = py.allow_threads(|| {
            let mut executor =
                ix::executor::Executor::new_with_caches(&snap.reader, pc, nc, rp, delta_path)
                    .with_resource_guard_and_ceiling(guard, ceiling);
            executor.execute(&plan, &options)
        });

        match result {
            Ok((matches, stats)) => Ok(PySearchResult::from_executor_result((matches, stats))),
            Err(e) => Err(crate::error::to_pyerr(&e)),
        }
    }

    /// Rebuild the index for this root directory.
    ///
    /// Requires the `notify` feature. Runs the full builder pipeline
    /// (walk, scan, serialize) with the GIL released, then reopens
    /// the reader so subsequent searches see the fresh data.
    ///
    /// # Arguments
    /// * `max_file_size_mb` - Skip files larger than this limit in MB.
    /// * `exclude_dirs` - Directory names to exclude from the walk.
    ///
    /// # Returns
    /// Dictionary with unified build-statistics fields.
    ///
    /// # Errors
    /// Raises `RuntimeError` if closed, `ValueError` on byte-size overflow,
    /// `NotImplementedError` if built without the `notify` feature, or
    /// `IxIoError` / `IxConfigError` on build failures.
    #[cfg(feature = "notify")]
    #[pyo3(signature = (*, max_file_size_mb = 100, exclude_dirs = None))]
    pub fn rebuild(
        &mut self,
        py: Python<'_>,
        max_file_size_mb: u64,
        exclude_dirs: Option<Vec<String>>,
    ) -> PyResult<Py<PyDict>> {
        self.ensure_open()?;
        let root = self.root.clone();
        let max_bytes = max_file_size_mb.checked_mul(1024 * 1024).ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err("max_file_size_mb overflows u64 bytes")
        })?;

        let report = py
            .allow_threads(|| run_builder(&root, max_bytes, exclude_dirs))
            .map_err(|e| crate::error::to_pyerr(&e))?;

        let new_snap = ReaderSnapshot::open(&self.index_path)?;
        self.posting_cache.ensure_shard(new_snap.fingerprint);
        self.posting_cache.invalidate_all();
        self.neg_cache.clear();
        self.regex_pool.clear();
        let mut guard = self
            .snapshot
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        *guard = Arc::new(new_snap);

        let dict = build_stats_dict(py, &root, &report)?;
        Ok(dict.unbind())
    }

    /// Stub for rebuild when notify feature is absent.
    ///
    /// # Errors
    /// Raises `RuntimeError` if closed, or `NotImplementedError` because the
    /// `notify` feature is disabled.
    #[cfg(not(feature = "notify"))]
    #[pyo3(signature = (*, max_file_size_mb = 100, exclude_dirs = None))]
    pub fn rebuild(
        &mut self,
        py: Python<'_>,
        max_file_size_mb: u64,
        exclude_dirs: Option<Vec<String>>,
    ) -> PyResult<Py<PyDict>> {
        self.ensure_open()?;
        let _ = (py, max_file_size_mb, exclude_dirs);
        Err(pyo3::exceptions::PyNotImplementedError::new_err(
            "ix-py built without 'notify' feature",
        ))
    }

    /// Build an index for `path` without requiring an existing index.
    ///
    /// This is a `@staticmethod` so `ix.build(path)` works even when no
    /// `.ix/shard.ix` exists yet. Creates the builder, runs the full
    /// pipeline (walk, scan, serialize), and returns build statistics.
    ///
    /// # Arguments
    /// * `path` - Root directory containing source files.
    /// * `max_file_size_mb` - Skip files larger than this limit in MB.
    /// * `exclude_dirs` - Directory names to exclude from the walk.
    ///
    /// # Returns
    /// Dictionary with unified build-statistics fields.
    ///
    /// # Errors
    /// Raises `ValueError` on byte-size overflow, `NotImplementedError` if
    /// built without the `notify` feature, or `IxIoError` / `IxConfigError`
    /// on build failures.
    #[cfg(feature = "notify")]
    #[staticmethod]
    #[pyo3(signature = (path, *, max_file_size_mb = 100, exclude_dirs = None))]
    pub fn build(
        py: Python<'_>,
        path: &str,
        max_file_size_mb: u64,
        exclude_dirs: Option<Vec<String>>,
    ) -> PyResult<Py<PyDict>> {
        let root = std::path::absolute(path)
            .map_err(|e| PyErr::new::<crate::error::IxIoError, _>(format!("invalid path: {e}")))?;
        let max_bytes = max_file_size_mb.checked_mul(1024 * 1024).ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err("max_file_size_mb overflows u64 bytes")
        })?;

        let report = py
            .allow_threads(|| run_builder(&root, max_bytes, exclude_dirs))
            .map_err(|e| crate::error::to_pyerr(&e))?;

        let dict = build_stats_dict(py, &root, &report)?;
        Ok(dict.unbind())
    }

    /// Stub for build when notify feature is absent.
    ///
    /// # Errors
    /// Always raises `NotImplementedError` because the `notify` feature is disabled.
    #[cfg(not(feature = "notify"))]
    #[staticmethod]
    #[pyo3(signature = (path, *, max_file_size_mb = 100, exclude_dirs = None))]
    pub fn build(
        py: Python<'_>,
        path: &str,
        max_file_size_mb: u64,
        exclude_dirs: Option<Vec<String>>,
    ) -> PyResult<Py<PyDict>> {
        let _ = (py, path, max_file_size_mb, exclude_dirs);
        Err(pyo3::exceptions::PyNotImplementedError::new_err(
            "ix-py built without 'notify' feature",
        ))
    }

    /// Return index-level statistics from the shard header.
    ///
    /// # Returns
    /// Dictionary with 11 fields: `shard_path`, `file_count`, `trigram_count`,
    /// `source_bytes_total`, `created_at`, `version`, `has_cdx`, `has_bloom`,
    /// `has_content_hashes`, `posting_lists_compressed`,
    /// `posting_lists_checksummed`.
    ///
    /// # Errors
    /// Raises `RuntimeError` if the index is closed.
    pub fn stats(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        self.ensure_open()?;
        let snap = self.fresh_snapshot()?;
        let h = &snap.reader.header;
        let dict = PyDict::new(py);
        let version = format!("{}.{}", h.version_major, h.version_minor);
        dict.set_item("shard_path", self.index_path.display().to_string())?;
        dict.set_item("file_count", h.file_count)?;
        dict.set_item("trigram_count", h.trigram_count)?;
        dict.set_item("source_bytes_total", h.source_bytes_total)?;
        dict.set_item("created_at", h.created_at)?;
        dict.set_item("version", version)?;
        dict.set_item("has_cdx", h.has_cdx())?;
        dict.set_item("has_bloom", h.has_bloom())?;
        let fl = h.flags;
        dict.set_item(
            "has_content_hashes",
            (fl & ix::format::flags::HAS_CONTENT_HASHES) != 0,
        )?;
        dict.set_item(
            "posting_lists_compressed",
            (fl & ix::format::flags::POSTING_LISTS_COMPRESSED) != 0,
        )?;
        dict.set_item(
            "posting_lists_checksummed",
            (fl & ix::format::flags::POSTING_LISTS_CHECKSUMMED) != 0,
        )?;
        Ok(dict.unbind())
    }

    /// Check whether a daemon (`ixd`) is currently watching this root.
    ///
    /// Reads `beacon.json` from `.ix/` and checks whether the recorded PID is
    /// live (and verifies process identity on Linux).
    ///
    /// # Returns
    /// Dictionary with unified status fields if a live daemon is found, `None` otherwise.
    ///
    /// # Errors
    /// Raises `RuntimeError` if closed, `NotImplementedError` if built without
    /// the `notify` feature, or `IxIoError` if `beacon.json` cannot be read.
    #[cfg(feature = "notify")]
    pub fn service_status(&self, py: Python<'_>) -> PyResult<Option<Py<PyDict>>> {
        self.ensure_open()?;
        let beacon_path = self.root.join(".ix").join("beacon.json");
        if !beacon_path.exists() {
            return Ok(None);
        }
        let beacon = ix::format::Beacon::read_from(self.root.join(".ix").as_path())
            .map_err(|e| crate::error::to_pyerr(&e))?;
        if !beacon.is_live() {
            return Ok(None);
        }
        let shard_rebuilt = self.snapshot_arc().reader.is_stale(&self.index_path);
        let snap = self.fresh_snapshot()?;
        let last_rebuild_at = snap.reader.header.created_at / 1_000_000;
        let workspace_dirty = Reader::get_last_modified(&self.root)
            .is_ok_and(|last_mod| last_mod > snap.reader.header.created_at);
        let root_str = beacon.root.display().to_string();
        let dict = PyDict::new(py);
        dict.set_item("daemon_running", true)?;
        dict.set_item("watching_path", &root_str)?;
        dict.set_item("index_path", self.index_path.display().to_string())?;
        dict.set_item("is_stale", shard_rebuilt)?;
        dict.set_item("shard_rebuilt", shard_rebuilt)?;
        dict.set_item("workspace_dirty", workspace_dirty)?;
        dict.set_item("last_rebuild_at", last_rebuild_at)?;
        dict.set_item("pid", beacon.pid)?;
        dict.set_item("root", &root_str)?;
        dict.set_item("start_time", beacon.start_time)?;
        dict.set_item("status", &beacon.status)?;
        dict.set_item("last_event_at", beacon.last_event_at)?;
        dict.set_item(
            "socket_path",
            beacon
                .socket_path
                .map_or_else(String::new, |p| p.display().to_string()),
        )?;
        Ok(Some(dict.unbind()))
    }

    /// Stub for `service_status` when notify feature is absent.
    ///
    /// # Errors
    /// Raises `RuntimeError` if closed, or `NotImplementedError` because the
    /// `notify` feature is disabled.
    #[cfg(not(feature = "notify"))]
    pub fn service_status(&self, py: Python<'_>) -> PyResult<Option<Py<PyDict>>> {
        self.ensure_open()?;
        let _ = py;
        Err(pyo3::exceptions::PyNotImplementedError::new_err(
            "ix-py built without 'notify' feature",
        ))
    }

    /// Close the index, release cached memory, and prevent further operations.
    ///
    /// Clears the posting-list cache, negative cache, and regex pool
    /// immediately and sets a flag that blocks subsequent `search()`,
    /// `stats()`, `rebuild()`, `service_status()`, and property access calls.
    pub fn close(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
        self.posting_cache.invalidate_all();
        self.neg_cache.clear();
        self.regex_pool.clear();
    }

    const fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __exit__(
        &self,
        _exc_type: Option<Py<PyAny>>,
        _exc_value: Option<Py<PyAny>>,
        _traceback: Option<Py<PyAny>>,
    ) {
        self.close();
    }

    /// Number of files in the index header.
    ///
    /// # Errors
    /// Raises `RuntimeError` if the index is closed.
    #[getter]
    pub fn file_count(&self) -> PyResult<u32> {
        self.ensure_open()?;
        Ok(self.fresh_snapshot()?.reader.header.file_count)
    }

    /// Number of unique trigrams in the index header.
    ///
    /// # Errors
    /// Raises `RuntimeError` if the index is closed.
    #[getter]
    pub fn trigram_count(&self) -> PyResult<u32> {
        self.ensure_open()?;
        Ok(self.fresh_snapshot()?.reader.header.trigram_count)
    }

    /// Unix timestamp (microseconds) when the index was created.
    ///
    /// # Errors
    /// Raises `RuntimeError` if the index is closed.
    #[getter]
    pub fn created_at(&self) -> PyResult<u64> {
        self.ensure_open()?;
        Ok(self.fresh_snapshot()?.reader.header.created_at)
    }

    /// The root path this index was opened from.
    ///
    /// # Errors
    /// Raises `RuntimeError` if the index is closed.
    #[getter]
    pub fn path(&self) -> PyResult<String> {
        self.ensure_open()?;
        Ok(self.root.display().to_string())
    }

    /// True if the shard file's inode changed (rebuilt under the live mmap).
    ///
    /// Checks whether the on-disk inode no longer matches the one captured
    /// when the reader was opened.
    ///
    /// # Errors
    /// Raises `RuntimeError` if the index is closed.
    #[getter]
    pub fn is_stale(&self) -> PyResult<bool> {
        self.ensure_open()?;
        Ok(self.snapshot_arc().reader.is_stale(&self.index_path))
    }

    /// String representation showing the root path and file count.
    fn __repr__(&self) -> String {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return format!("Index(path={}, closed=True)", self.root.display());
        }
        let snap = self
            .fresh_snapshot()
            .unwrap_or_else(|_| self.snapshot_arc());
        format!(
            "Index(path={}, file_count={}, trigram_count={})",
            self.root.display(),
            snap.reader.header.file_count,
            snap.reader.header.trigram_count,
        )
    }
}
