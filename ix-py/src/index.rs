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
use std::sync::Arc;

/// An open ix index backed by a memory-mapped shard file.
///
/// Provides search, build, stats, and service-status methods.
/// Created via `Index(path)` where `path` is a directory containing
/// `.ix/shard.ix` or the index file itself.
#[pyclass(name = "Index")]
pub struct PyIndex {
    reader: Reader,
    index_path: PathBuf,
    posting_cache: Arc<ix::posting_cache::PostingCache>,
    neg_cache: Arc<ix::neg_cache::NegCache>,
    regex_pool: Arc<ix::regex_pool::RegexPool>,
    root: PathBuf,
    closed: std::sync::atomic::AtomicBool,
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
        if candidate.exists() {
            return Ok(candidate);
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

/// Build a Python dict with the 6 build-stats fields.
///
/// # Arguments
/// * `py` - Python interpreter token with lifetime `'py`.
/// * `root` - Root path displayed as a string.
/// * `fields` - Tuple of
///   (`files_scanned`, `files_skipped_binary`, `files_skipped_size`,
///   `bytes_scanned`, `unique_trigrams`).
///
/// # Returns
/// A new `PyDict` with string keys and mixed int/string values.
fn build_stats_dict<'py>(
    py: Python<'py>,
    root: &Path,
    fields: (u64, u64, u64, u64, u64),
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("path", root.display().to_string())?;
    dict.set_item("files_scanned", fields.0)?;
    dict.set_item("files_skipped_binary", fields.1)?;
    dict.set_item("files_skipped_size", fields.2)?;
    dict.set_item("bytes_scanned", fields.3)?;
    dict.set_item("unique_trigrams", fields.4)?;
    Ok(dict)
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
    /// Raises `IxIndexError` if no index is found or the file is corrupt.
    #[new]
    #[pyo3(signature = (path, *, cache_mb = 64))]
    pub fn new(path: &str, cache_mb: usize) -> PyResult<Self> {
        let path = std::path::Path::new(path);
        let index_path = find_index(path)?;
        let reader = Reader::open(&index_path).map_err(crate::error::to_pyerr)?;
        let root = index_path
            .parent()
            .and_then(|p| p.parent())
            .map_or_else(|| index_path.clone(), Path::to_path_buf);
        let cache_bytes = cache_mb * 1024 * 1024;
        let posting_cache = Arc::new(ix::posting_cache::PostingCache::new(cache_bytes));
        Ok(Self {
            reader,
            index_path,
            posting_cache,
            neg_cache: Arc::new(ix::neg_cache::NegCache::new(65_536)),
            regex_pool: Arc::new(ix::regex_pool::RegexPool::new(256)),
            root,
            closed: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Search the index with a literal or regex pattern.
    ///
    /// Internally calls `Planner::plan_with_options()` then
    /// `Executor::execute()` with the GIL released so that Rayon
    /// parallel threads are not blocked.
    ///
    /// # Arguments
    /// * `pattern` - Non-empty search string.
    /// * `regex` - Treat `pattern` as a regex.
    /// * `context_lines` - Context lines before and after each match.
    /// * `max_results` - Cap on total matches (0 = unlimited).
    /// * `type_filter` - File extensions to include (e.g. `["rs", "py"]`).
    /// * `multiline` - Dot-matches-newline flag.
    /// * `case_insensitive` - Case-insensitive matching.
    /// * `word_boundary` - Match only at word boundaries.
    ///
    /// # Returns
    /// `PySearchResult` with `matches` (list of `Match`) and `stats` (dict).
    ///
    /// # Errors
    /// Raises `IxIndexError` / `IxCorruptionError` / `IxRegexError`.
    #[pyo3(signature = (pattern, *, regex = false, context_lines = 0, max_results = 0, type_filter = None, multiline = false, case_insensitive = false, word_boundary = false, count_only = false, files_only = false))]
    #[allow(clippy::fn_params_excessive_bools)]
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::fn_params_excessive_bools)]
    pub fn search(
        &self,
        py: Python<'_>,
        pattern: &str,
        regex: bool,
        context_lines: usize,
        max_results: usize,
        type_filter: Option<Vec<String>>,
        multiline: bool,
        case_insensitive: bool,
        word_boundary: bool,
        count_only: bool,
        files_only: bool,
    ) -> PyResult<PySearchResult> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(pyo3::exceptions::PyRuntimeError::new_err("Index is closed"));
        }

        let plan = Planner::plan_with_options(
            pattern,
            ix::planner::QueryOptions {
                is_regex: regex,
                ignore_case: case_insensitive,
                multiline,
                word_boundary,
            },
        )
        .map_err(crate::error::to_pyerr)?;

        let options = ix::executor::QueryOptions {
            context_lines,
            max_results,
            type_filter: type_filter.unwrap_or_default(),
            count_only,
            files_only,
            ..Default::default()
        };

        let pc = Arc::clone(&self.posting_cache);
        let nc = Arc::clone(&self.neg_cache);
        let rp = Arc::clone(&self.regex_pool);

        let result = py.allow_threads(|| {
            let mut executor =
                ix::executor::Executor::new_with_caches(&self.reader, pc, nc, rp, None);
            executor.execute(&plan, &options)
        });

        match result {
            Ok((matches, stats)) => Ok(PySearchResult::from_executor_result((matches, stats))),
            Err(e) => Err(crate::error::to_pyerr(e)),
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
    /// Dictionary with 6 build-statistics fields.
    ///
    /// # Errors
    /// Raises `NotImplementedError` if built without the `notify` feature.
    /// Raises `IxIoError` / `IxConfigError` on build failures.
    #[cfg(feature = "notify")]
    #[pyo3(signature = (*, max_file_size_mb = 100, exclude_dirs = None))]
    pub fn rebuild(
        &mut self,
        py: Python<'_>,
        max_file_size_mb: u64,
        exclude_dirs: Option<Vec<String>>,
    ) -> PyResult<Py<PyDict>> {
        let root = self.root.clone();
        let max_bytes = max_file_size_mb * 1024 * 1024;
        let exclude = exclude_dirs;

        let fields = py
            .allow_threads(|| -> Result<(u64, u64, u64, u64, u64), ix::error::Error> {
                let mut builder = ix::builder::Builder::new(&root)?;
                builder.set_max_file_size(max_bytes);
                if let Some(dirs) = exclude {
                    let mut b = builder.with_exclude_patterns(dirs);
                    b.build()?;
                    let s = b.stats();
                    Ok((
                        s.files_scanned,
                        s.files_skipped_binary,
                        s.files_skipped_size,
                        s.bytes_scanned,
                        s.unique_trigrams,
                    ))
                } else {
                    builder.build()?;
                    let s = builder.stats();
                    Ok((
                        s.files_scanned,
                        s.files_skipped_binary,
                        s.files_skipped_size,
                        s.bytes_scanned,
                        s.unique_trigrams,
                    ))
                }
            })
            .map_err(crate::error::to_pyerr)?;

        self.posting_cache.invalidate_all();
        self.neg_cache.clear();
        self.regex_pool.clear();
        self.reader = Reader::open(&self.index_path).map_err(crate::error::to_pyerr)?;
        self.closed
            .store(false, std::sync::atomic::Ordering::Release);

        let dict = build_stats_dict(py, &root, fields)?;
        Ok(dict.unbind())
    }

    /// Build an index for `path` without requiring an existing index.
    ///
    /// This is a `@staticmethod` so `ix.build(path)` works even when no
    /// `.ix/shard.ix` exists yet.  Creates the builder, runs the full
    /// pipeline (walk, scan, serialize), and returns build statistics.
    ///
    /// # Arguments
    /// * `path` - Root directory containing source files.
    /// * `max_file_size_mb` - Skip files larger than this limit in MB.
    /// * `exclude_dirs` - Directory names to exclude from the walk.
    ///
    /// # Returns
    /// Dictionary with 6 build-statistics fields.
    ///
    /// # Errors
    /// Raises `NotImplementedError` if built without the `notify` feature.
    /// Raises `IxIoError` / `IxConfigError` on build failures.
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
        let max_bytes = max_file_size_mb * 1024 * 1024;
        let exclude = exclude_dirs;

        let fields = py
            .allow_threads(|| -> Result<(u64, u64, u64, u64, u64), ix::error::Error> {
                let mut builder = ix::builder::Builder::new(&root)?;
                builder.set_max_file_size(max_bytes);
                if let Some(dirs) = exclude {
                    let mut b = builder.with_exclude_patterns(dirs);
                    b.build()?;
                    let s = b.stats();
                    Ok((
                        s.files_scanned,
                        s.files_skipped_binary,
                        s.files_skipped_size,
                        s.bytes_scanned,
                        s.unique_trigrams,
                    ))
                } else {
                    builder.build()?;
                    let s = builder.stats();
                    Ok((
                        s.files_scanned,
                        s.files_skipped_binary,
                        s.files_skipped_size,
                        s.bytes_scanned,
                        s.unique_trigrams,
                    ))
                }
            })
            .map_err(crate::error::to_pyerr)?;

        let dict = build_stats_dict(py, &root, fields)?;
        Ok(dict.unbind())
    }

    /// Stub for build when notify feature is absent.
    #[cfg(not(feature = "notify"))]
    #[allow(unused_variables)]
    #[pyo3(signature = (path, *, max_file_size_mb = 100, exclude_dirs = None))]
    pub fn build(
        _py: Python<'_>,
        path: &str,
        max_file_size_mb: u64,
        exclude_dirs: Option<Vec<String>>,
    ) -> PyResult<Py<PyDict>> {
        Err(pyo3::exceptions::PyNotImplementedError::new_err(
            "ix-py built without 'notify' feature",
        ))
    }

    /// Return index-level statistics from the shard header.
    ///
    /// # Returns
    /// Dictionary with 12 fields: `shard_path`, `file_count`, `trigram_count`,
    /// `source_bytes_total`, `created_at`, `version`, `has_cdx`, `has_bloom`,
    /// `has_content_hashes`, `posting_lists_compressed`,
    /// `posting_lists_checksummed`.
    pub fn stats(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let h = &self.reader.header;
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
    /// Reads `beacon.json` from `.ix/` and checks the PID is alive with
    /// an `ixd` process name in `/proc/{pid}/comm`.
    ///
    /// # Returns
    /// Dictionary with 6 fields if a live daemon is found, `None` otherwise.
    ///
    /// # Errors
    /// Raises `NotImplementedError` if built without the `notify` feature.
    #[cfg(feature = "notify")]
    pub fn service_status(&self, py: Python<'_>) -> PyResult<Option<Py<PyDict>>> {
        let beacon_path = self.root.join(".ix").join("beacon.json");
        if !beacon_path.exists() {
            return Ok(None);
        }
        let beacon = ix::format::Beacon::read_from(self.root.join(".ix").as_path())
            .map_err(crate::error::to_pyerr)?;
        if !beacon.is_live() {
            return Ok(None);
        }
        let dict = PyDict::new(py);
        dict.set_item("pid", beacon.pid)?;
        dict.set_item("root", beacon.root.display().to_string())?;
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

    /// Stub for service_status when notify feature is absent.
    #[cfg(not(feature = "notify"))]
    #[allow(unused_variables)]
    pub fn service_status(&self, py: Python<'_>) -> PyResult<Option<Py<PyDict>>> {
        Err(pyo3::exceptions::PyNotImplementedError::new_err(
            "ix-py built without 'notify' feature",
        ))
    }

    /// Close the index and prevent further operations.
    ///
    /// Sets a flag that blocks subsequent `search()` and `stats()` calls.
    /// The underlying mmap remains alive until the Python GC collects the
    /// `Index` instance, but all methods will raise `RuntimeError`.
    #[allow(clippy::unused_self)]
    pub fn close(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
    }

    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
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
    #[getter]
    pub fn file_count(&self) -> u32 {
        self.reader.header.file_count
    }

    /// Number of unique trigrams in the index header.
    #[getter]
    pub fn trigram_count(&self) -> u32 {
        self.reader.header.trigram_count
    }

    /// Unix timestamp (microseconds) when the index was created.
    #[getter]
    pub fn created_at(&self) -> u64 {
        self.reader.header.created_at
    }

    /// The root path this index was opened from.
    #[getter]
    pub fn path(&self) -> String {
        self.root.display().to_string()
    }

    /// True if the shard file's inode changed (rebuilt under the live mmap).
    ///
    /// Checks whether the on-disk inode no longer matches the one captured
    /// when the reader was opened.
    #[getter]
    pub fn is_stale(&self) -> bool {
        self.reader.is_stale(&self.index_path)
    }

    /// String representation showing the root path and file count.
    fn __repr__(&self) -> String {
        format!(
            "Index(path={}, file_count={}, trigram_count={})",
            self.root.display(),
            self.reader.header.file_count,
            self.reader.header.trigram_count,
        )
    }
}
