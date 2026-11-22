//! Python native extension (`_ix`) for ix — sub-millisecond code search via sparse trigram indexing.
//!
//! Exposes `PyIndex` (the Python `Index` class), `PyMatch`, `PySearchResult`,
//! `PyPipeline`, and the `IxError` exception hierarchy. Module-level
//! convenience functions (`search`, `build`, `stats`, `service_status`) are
//! provided by the `ix` Python wrapper package (`python/ix/__init__.py`).
//!
//! # Error Mapping
//!
//! Each [`ix::error::Error`] variant maps to a specific Python exception
//! subclass of `IxError`, preserving the full type hierarchy for precise
//! exception handling from Python.
//!
//! # Feature Flags
//!
//! - **`notify`** (default) — Provides `build`, `rebuild`, and `service_status` methods.

// Lint configuration
#![warn(clippy::pedantic)]
#![warn(missing_docs)]
#![warn(clippy::unimplemented)]

mod error;
mod index;
mod safety;
mod types;

use pyo3::prelude::*;
use std::sync::Once;

static TRACING_INIT: Once = Once::new();

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
    TRACING_INIT.call_once(|| {
        let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(std::io::stderr)
            .try_init();
    });
    error::register_exceptions(m)?;
    m.add_class::<types::PyMatch>()?;
    m.add_class::<types::PySearchResult>()?;
    m.add_class::<index::PyIndex>()?;
    m.add_class::<safety::PyPipeline>()?;
    Ok(())
}
