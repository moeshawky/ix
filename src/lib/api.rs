//! Convenience API for search operations.
//!
//! Thin wrappers around the planner + executor pipeline for simple
//! single-call searches.

use crate::error::Result;
use crate::executor::{Executor, Match, QueryOptions, QueryStats};
use crate::planner::Planner;
use crate::reader::Reader;

/// Execute a search against an open index with a single call.
///
/// Plans the query, creates an executor, and runs the search.
/// Returns all matches and query statistics.
///
/// # Errors
///
/// Returns an error if the index cannot be read, posting data is
/// corrupted, or file content cannot be accessed during verification.
pub fn execute(
    reader: &Reader,
    pattern: &str,
    is_regex: bool,
    options: &QueryOptions,
) -> Result<(Vec<Match>, QueryStats)> {
    let plan = Planner::plan_with_options(
        pattern,
        crate::planner::QueryOptions {
            is_regex,
            ..Default::default()
        },
    );
    let mut executor = Executor::new(reader);
    executor.execute(&plan, options)
}
