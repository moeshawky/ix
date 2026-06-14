//! Error bridge — maps `ix::error::Error` variants to Python exception classes.
//!
//! The exception hierarchy mirrors the error taxonomy from C-ERROR:
//!
//! ```text
//! IxError (base, extends Exception)
//! ├── IxIndexError
//! ├── IxCorruptionError
//! ├── IxIoError
//! ├── IxRegexError
//! ├── IxConfigError
//! ├── IxWatcherError      (notify feature only)
//! └── IxArchiveError      (archive feature only)
//! ```
//!
//! Exception types are defined via [`pyo3::create_exception`] which manages
//! the `CPython` type object with correct base-class inheritance.

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;

create_exception!(
    "ix._ix",
    IxError,
    PyException,
    "Root exception class for all ix errors."
);

create_exception!(
    "ix._ix",
    IxIndexError,
    IxError,
    "Raised when the index file cannot be opened or parsed."
);

create_exception!(
    "ix._ix",
    IxCorruptionError,
    IxError,
    "Raised when index data is internally corrupted."
);

create_exception!(
    "ix._ix",
    IxIoError,
    IxError,
    "Raised for I/O and path errors."
);

create_exception!(
    "ix._ix",
    IxRegexError,
    IxError,
    "Raised for invalid regex patterns or matching failures."
);

create_exception!(
    "ix._ix",
    IxConfigError,
    IxError,
    "Raised for configuration-related errors."
);

create_exception!(
    "ix._ix",
    IxWatcherError,
    IxError,
    "Raised for file-watcher errors (notify feature only)."
);

create_exception!(
    "ix._ix",
    IxArchiveError,
    IxError,
    "Raised for archive errors (archive feature only)."
);

create_exception!(
    "ix._ix",
    LLMOSafeError,
    IxError,
    "Raised when an llmosafe C-ABI getter call fails."
);

/// Register all exception classes on the given Python module.
///
/// # Arguments
/// * `m` - Python module to attach exception classes to.
///
/// # Errors
/// Returns `PyErr` if any class cannot be added.
pub fn register_exceptions(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("IxError", m.py().get_type::<IxError>())?;
    m.add("IxIndexError", m.py().get_type::<IxIndexError>())?;
    m.add("IxCorruptionError", m.py().get_type::<IxCorruptionError>())?;
    m.add("IxIoError", m.py().get_type::<IxIoError>())?;
    m.add("IxRegexError", m.py().get_type::<IxRegexError>())?;
    m.add("IxConfigError", m.py().get_type::<IxConfigError>())?;
    m.add("IxWatcherError", m.py().get_type::<IxWatcherError>())?;
    m.add("IxArchiveError", m.py().get_type::<IxArchiveError>())?;
    m.add("LLMOSafeError", m.py().get_type::<LLMOSafeError>())?;
    Ok(())
}

/// Convert an [`ix::error::Error`] into a Python error.
///
/// Maps each variant to the appropriate `Ix*Error` subclass.
///
/// # Arguments
/// * `err` - Error from the ix crate to convert (taken by value for destructuring).
///
/// # Returns
/// A `PyErr` with the appropriate Python exception class and message.
#[allow(clippy::needless_pass_by_value)]
pub fn to_pyerr(err: ix::error::Error) -> PyErr {
    let message = err.to_string();
    match err {
        ix::error::Error::Io(_) => PyErr::new::<IxIoError, _>(message),
        ix::error::Error::IndexTooSmall
        | ix::error::Error::BadMagic
        | ix::error::Error::UnsupportedVersion { .. }
        | ix::error::Error::HeaderCorrupted { .. } => PyErr::new::<IxIndexError, _>(message),
        ix::error::Error::CdxBlockCorrupted(msg) => {
            PyErr::new::<IxCorruptionError, _>(format!("CDX block data is corrupted: {msg}"))
        }
        ix::error::Error::PostingCorrupted
        | ix::error::Error::TruncatedVarint(_)
        | ix::error::Error::OverflowVarint
        | ix::error::Error::PostingOutOfBounds
        | ix::error::Error::FileIdOutOfBounds(_)
        | ix::error::Error::StringPoolOutOfBounds
        | ix::error::Error::SectionOutOfBounds { .. } => {
            PyErr::new::<IxCorruptionError, _>(message)
        }
        ix::error::Error::InvalidPath => PyErr::new::<IxIoError, _>(message),
        ix::error::Error::Regex(_) => PyErr::new::<IxRegexError, _>(message),
        ix::error::Error::Config(_) => PyErr::new::<IxConfigError, _>(message),
        #[cfg(feature = "notify")]
        ix::error::Error::Watcher(_) => PyErr::new::<IxWatcherError, _>(message),
        #[cfg(feature = "archive")]
        ix::error::Error::Zip(_) => PyErr::new::<IxArchiveError, _>(message),
    }
}
