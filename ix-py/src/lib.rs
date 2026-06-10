//! Python bindings for ix — sub-millisecond code search via sparse trigram indexing.
//!
//! Exposes `PyIndex` (the Python `Index` class) for opening and querying ix shard files, along with
//! error types that map directly to Python exception classes. Module-level
//! convenience functions provide single-call search, build, stats, and
//! service status without managing an `Index` handle.
//!
//! # Error Mapping
//!
//! Each [`ix::error::Error`] variant maps to a specific Python exception
//! subclass of `IxError`, preserving the full type hierarchy for precise
//! exception handling from Python.
//!
//! # Feature Flags
//!
//! - **`notify`** (default) — Enables the `build` and `service_status` methods.

// Lint configuration
#![warn(clippy::pedantic)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::module_name_repetitions)]
#![warn(missing_docs)]
#![warn(clippy::unimplemented)]

mod error;
mod index;
mod safety;
mod types;

use pyo3::prelude::*;

/// Register all Python classes, exceptions, and module-level functions.
///
/// Called once at `import _ix` to populate the module namespace.
///
/// # Errors
///
/// Returns an error if any class registration fails (should never happen
/// in practice unless the module is misconfigured).
#[pymodule]
#[pyo3(name = "_ix")]
pub fn _ix(m: &Bound<'_, PyModule>) -> PyResult<()> {
    error::register_exceptions(m)?;
    m.add_class::<types::PyMatch>()?;
    m.add_class::<types::PySearchResult>()?;
    m.add_class::<index::PyIndex>()?;
    m.add_class::<safety::PyPipeline>()?;
    Ok(())
}
