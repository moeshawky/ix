//! Public API surface for the search crate.

use executor::Executor;
use reader::Reader;

/// Execute a search request against an index reader.
///
/// # Source
/// `/src/lib/executor.rs:102` — verified 2026-04-19
///
/// # Example (Verified)
/// ```
/// # use std::sync::Arc;
/// # use moeix::{Reader, Executor};
/// # let reader = Reader::open("/tmp/index").unwrap();
/// # let mut executor = Executor::new(&reader);
/// let result = executor.execute(/* request parameters */);
/// ```
pub fn execute(reader: &Reader, request: &serde_json::Value) -> serde_json::Value {
    let mut executor = Executor::new(reader);
    executor.execute(request)
}