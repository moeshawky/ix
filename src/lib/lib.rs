//! ix — sub-millisecond code search via sparse trigram indexing.
//!
//! `ix` pre-computes a byte-level trigram index to narrow search candidates
//! to a fraction of the total file set, then verifies matches with a
//! memory-constant streaming architecture. This eliminates the linear-scan
//! bottleneck of traditional tools on large codebases.
//!
//! # Installation
//!
//! ```bash
//! cargo install moeix
//! ```
//!
//! # Quick Start
//!
//! ```bash
//! ix --build /path/to/repo
//! ix "fn validate"
//! ix --regex "fn\\s+\\w+_handler" --context 3
//! ```
//!
//! # Library Usage
//!
//! ```rust,ignore
//! use ix::reader::Reader;
//! use ix::executor::{Executor, QueryOptions};
//! use ix::planner::Planner;
//!
//! let reader = Reader::open(".ix/shard.ix")?;
//! let plan = Planner::plan("struct Config", false);
//! let mut executor = Executor::new(&reader);
//! let (matches, stats) = executor.execute(&plan, &QueryOptions::default())?;
//! ```
//!
//! # Module Build Order
//!
//! `format` → `varint` → `trigram` → `bloom` → `posting` →
//! `string_pool` → `builder` → `reader` → `planner` → `executor`
//!
//! Cache layer: `posting_cache` · `neg_cache` · `regex_pool` (used by `executor`)
//!
//! Policy: `cache_policy` (standalone, adapts to `ResourceGuard` pressure)
//!
//! Support: `scanner` (fallback, no index) · `streaming` (line/mmap alternatives) ·
//! `api` (convenience wrapper) · `config` (`.ixd.toml`)
//!
//! # See Also
//!
//! - [GitHub README](https://github.com/moeshawky/ix) — install, quick start, daemon
//! - [Socket API](https://github.com/moeshawky/ix/blob/main/docs/SOCKET-API.md) — daemon IPC protocol
//!
//! # Feature Flags
//!
//! - **`notify`** (default) — File watcher + daemon (`ixd`) + Unix domain socket IPC
//! - **`decompress`** — gz/zst/bz2/xz decompression
//! - **`archive`** — zip/tar archive support
//! - **`full`** — All optional features

// Lint configuration — Tier 1 security hardened, pedantic with pragmatic allow
#![warn(clippy::pedantic)]
#![allow(clippy::cast_possible_truncation)] // 64-bit target; try_from guards runtime
#![allow(clippy::cast_possible_wrap)] // controlled internal values
#![allow(clippy::cast_sign_loss)] // controlled internal values
#![allow(clippy::module_name_repetitions)] // module::Type is idiomatic
#![allow(clippy::too_many_lines)] // scanner/executor are well-structured
#![warn(missing_docs)] // enterprise: every public item documented
#![cfg_attr(test, allow(missing_docs))] // tests: no doc needed for test fns
#![warn(clippy::semicolon_if_nothing_returned)]
#![warn(clippy::non_ascii_literal)]
#![warn(clippy::unimplemented)]
#![warn(clippy::use_self)]
#![warn(clippy::string_slice)]
#![warn(clippy::clone_on_ref_ptr)]

extern crate llmosafe;

pub mod api;
pub mod archive;
pub mod bloom;
pub mod builder;
/// Adaptive cache policy driven by `ResourceGuard` memory pressure.
#[cfg_attr(docsrs, doc(cfg(feature = "notify")))]
pub mod cache_policy;
pub mod config;
#[cfg(feature = "notify")]
#[cfg_attr(docsrs, doc(cfg(feature = "notify")))]
pub mod daemon;
#[cfg(feature = "notify")]
#[cfg_attr(docsrs, doc(cfg(feature = "notify")))]
pub mod daemon_sock;
pub mod decompress;
/// Error types for the ix crate.
pub mod error;
pub mod executor;
pub mod format;
#[cfg(feature = "notify")]
#[cfg_attr(docsrs, doc(cfg(feature = "notify")))]
pub mod idle;
/// Negative result cache — skips re-verification of known non-matching files.
pub mod neg_cache;
pub mod planner;
pub mod posting;
/// LRU cache for decoded posting lists, keyed by trigram.
pub mod posting_cache;
pub mod reader;
/// Compiled regex pool — caches `Regex` objects to avoid recompilation.
pub mod regex_pool;
pub mod scanner;
/// Alternative streaming file search implementations — line-based and
/// mmap-windowed verification with context support.
pub mod streaming;
pub mod string_pool;
pub mod trigram;
pub mod varint;
#[cfg(feature = "notify")]
#[cfg_attr(docsrs, doc(cfg(feature = "notify")))]
pub mod watcher;

#[cfg(feature = "notify")]
#[cfg_attr(docsrs, doc(cfg(feature = "notify")))]
pub use crate::builder::Builder;
#[cfg(feature = "notify")]
#[cfg_attr(docsrs, doc(cfg(feature = "notify")))]
pub use crate::daemon_sock::{
    ClientMessage, DaemonClient, DaemonServer, FileChange, FileOp, ServerMessage,
};
#[cfg(feature = "notify")]
#[cfg_attr(docsrs, doc(cfg(feature = "notify")))]
pub use crate::format::Beacon;
#[cfg(feature = "notify")]
#[cfg_attr(docsrs, doc(cfg(feature = "notify")))]
pub use crate::idle::IdleTracker;
#[cfg(feature = "notify")]
#[cfg_attr(docsrs, doc(cfg(feature = "notify")))]
pub use crate::watcher::Watcher;

/// Run the daemon watching the given directory for changes and rebuilding the index.
///
/// This function is deprecated. Use [`crate::daemon::run`] instead.
///
/// # Errors
///
/// Delegates to [`daemon::run`]; see that function for error conditions.
#[cfg(feature = "notify")]
#[deprecated(since = "0.7.0", note = "use ix::daemon::run instead")]
pub fn run_daemon(path: &std::path::Path) -> crate::error::Result<()> {
    crate::daemon::run(path)
}
