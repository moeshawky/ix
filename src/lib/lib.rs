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
//! use ix::{Reader, Executor};
//!
//! let reader = Reader::open(".ix/shard.ix")?;
//! let mut executor = Executor::new(&reader);
//! let matches = executor.execute(query);
//! ```
//!
//! # Module Build Order
//!
//! `format` → `varint` → `trigram` → `bloom` → `posting` →
//! `string_pool` → `builder` → `reader` → `planner` → `executor` → `scanner`
//!
//! Cache layer: `posting_cache` → `neg_cache` → `regex_pool` → `cache_policy`
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

pub mod archive;
pub mod bloom;
pub mod builder;
/// Adaptive cache policy driven by `ResourceGuard` memory pressure.
pub mod cache_policy;
pub mod config;
#[cfg(feature = "notify")]
pub mod daemon_sock;
pub mod decompress;
/// Error types for the ix crate.
pub mod error;
pub mod executor;
pub mod format;
#[cfg(feature = "notify")]
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
pub mod string_pool;
pub mod trigram;
pub mod varint;
#[cfg(feature = "notify")]
pub mod watcher;

#[cfg(feature = "notify")]
pub use crate::builder::Builder;
#[cfg(feature = "notify")]
pub use crate::daemon_sock::{
    ClientMessage, DaemonClient, DaemonServer, FileChange, FileOp, ServerMessage,
};
#[cfg(feature = "notify")]
pub use crate::format::Beacon;
#[cfg(feature = "notify")]
pub use crate::idle::IdleTracker;
#[cfg(feature = "notify")]
pub use crate::watcher::Watcher;

/// Run the daemon watching the given directory for changes and rebuilding the index.
///
/// # Errors
///
/// Returns an error if the root cannot be canonicalized, the index cannot be built,
/// or the file watcher fails.
///
/// # Panics
///
/// Panics if the Ctrl-C handler cannot be set (which is extremely rare).
#[cfg(feature = "notify")]
pub fn run_daemon(path: &std::path::Path) -> crate::error::Result<()> {
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    let root = path.canonicalize().map_err(crate::error::Error::Io)?;

    println!("ixd: watching {}...", root.display());

    let mut builder = Builder::new(&root)?;

    // Lazy startup: if index exists, just update. Otherwise build.
    let ix_dir = root.join(".ix");
    let index_file = ix_dir.join("shard.ix");
    if index_file.exists() {
        println!("ixd: existing index found, performing startup update...");
        // In a real implementation we would load the 'files' table from the shard
        // to know which files to check for changes. For now we just build to satisfy
        // the current Builder API, but we've already optimized Builder's memory.
        builder.build()?;
    } else {
        builder.build()?;
    }

    println!(
        "ixd: initial index ready ({} files, {} trigrams)",
        builder.files_len(),
        builder.trigrams_len()
    );

    let mut watcher = Watcher::new(&root);
    let rx = watcher.start()?;

    let ix_dir = root.join(".ix");
    if !ix_dir.exists() {
        fs::create_dir_all(&ix_dir)?;
    }
    let mut beacon = Beacon::new(&root);
    beacon.write_to(&ix_dir)?;

    let mut idle = IdleTracker::new();

    let running = Arc::new(AtomicBool::new(true));
    let r = std::sync::Arc::clone(&running);
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl-C handler");

    while running.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(changed_files) => {
                println!(
                    "ixd: {} files changed, updating index...",
                    changed_files.len()
                );

                beacon.status = "indexing".to_string();
                beacon.last_event_at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let _ = beacon.write_to(&ix_dir);

                idle.record_change();
                builder.update(&changed_files)?;

                beacon.status = "idle".to_string();
                let _ = beacon.write_to(&ix_dir);
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    println!("ixd: shutting down");
    let _ = fs::remove_file(ix_dir.join("beacon.json"));
    watcher.stop();
    Ok(())
}
