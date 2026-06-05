//! Python-exported types — `Match` and `SearchResult`.
//!
//! Converts Rust structs from `ix::executor` into `#[pyclass]` types
//! that can be returned to Python callers with full field access.

use pyo3::prelude::*;
use std::collections::HashMap;

/// A single regex match found in a file.
///
/// Maps directly to [`ix::executor::Match`] with all 8 fields exposed
/// as Python properties.
#[pyclass(name = "Match")]
#[derive(Clone)]
pub struct PyMatch {
    /// Absolute path to the file containing the match.
    #[pyo3(get)]
    pub file_path: String,
    /// 1-based line number.
    #[pyo3(get)]
    pub line_number: u32,
    /// 1-based column within the line (byte offset).
    #[pyo3(get)]
    pub col: u32,
    /// The entire content of the matching line.
    #[pyo3(get)]
    pub line_content: String,
    /// Byte offset from the start of the file.
    #[pyo3(get)]
    pub byte_offset: u64,
    /// Lines preceding the match (context).
    #[pyo3(get)]
    pub context_before: Vec<String>,
    /// Lines following the match (context).
    #[pyo3(get)]
    pub context_after: Vec<String>,
    /// Whether the file was detected as binary.
    #[pyo3(get)]
    pub is_binary: bool,
}

#[pymethods]
impl PyMatch {
    /// String representation showing `file:line:col`.
    fn __repr__(&self) -> String {
        format!(
            "Match(file_path={}, line_number={}, col={})",
            self.file_path, self.line_number, self.col
        )
    }

    /// Equality comparison for testing.
    fn __eq__(&self, other: &PyMatch) -> bool {
        self.file_path == other.file_path
            && self.line_number == other.line_number
            && self.col == other.col
            && self.line_content == other.line_content
            && self.byte_offset == other.byte_offset
    }
}

impl From<ix::executor::Match> for PyMatch {
    fn from(m: ix::executor::Match) -> Self {
        Self {
            file_path: m.file_path.display().to_string(),
            line_number: m.line_number,
            col: m.col,
            line_content: m.line_content,
            byte_offset: m.byte_offset,
            context_before: m.context_before,
            context_after: m.context_after,
            is_binary: m.is_binary,
        }
    }
}

/// Container for search results — matches plus query statistics.
///
/// Fields:
/// - `matches`: List of [`PyMatch`] objects found.
/// - `stats`: Dictionary of 11 [`ix::executor::QueryStats`] fields.
#[pyclass(name = "SearchResult")]
pub struct PySearchResult {
    /// All matches found during the search.
    #[pyo3(get)]
    pub matches: Vec<PyMatch>,
    /// Query execution statistics as a flat dict.
    #[pyo3(get)]
    pub stats: HashMap<String, u64>,
}

impl PySearchResult {
    /// Construct from a tuple of Rust results.
    ///
    /// # Arguments
    /// * `(matches, stats)` - Tuple from `Executor::execute()`.
    ///
    /// # Returns
    /// A `PySearchResult` ready for return to Python.
    pub fn from_executor_result(
        result: (Vec<ix::executor::Match>, ix::executor::QueryStats),
    ) -> Self {
        let (matches, stats) = result;
        let mut stats_map = HashMap::new();
        stats_map.insert(
            "trigrams_queried".to_string(),
            u64::from(stats.trigrams_queried),
        );
        stats_map.insert(
            "posting_lists_decoded".to_string(),
            u64::from(stats.posting_lists_decoded),
        );
        stats_map.insert(
            "candidate_files".to_string(),
            u64::from(stats.candidate_files),
        );
        stats_map.insert(
            "files_verified".to_string(),
            u64::from(stats.files_verified),
        );
        stats_map.insert("files_failed_verify".to_string(), stats.files_failed_verify);
        stats_map.insert("bytes_verified".to_string(), stats.bytes_verified);
        stats_map.insert("total_matches".to_string(), u64::from(stats.total_matches));
        stats_map.insert("posting_cache_hits".to_string(), stats.posting_cache_hits);
        stats_map.insert(
            "posting_cache_misses".to_string(),
            stats.posting_cache_misses,
        );
        stats_map.insert("neg_cache_hits".to_string(), stats.neg_cache_hits);
        stats_map.insert("neg_cache_misses".to_string(), stats.neg_cache_misses);
        Self {
            matches: matches.into_iter().map(PyMatch::from).collect(),
            stats: stats_map,
        }
    }
}
