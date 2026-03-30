// ix — trigram code search library
//
// Build order: format → varint → trigram → bloom → posting →
//              string_pool → builder → reader → planner → executor → scanner

pub mod bloom;
pub mod builder;
pub mod config;
pub mod decompress;
pub mod archive;
pub mod error;
pub mod executor;
pub mod format;
#[cfg(feature = "notify")]
pub mod idle;
pub mod planner;
pub mod posting;
pub mod reader;
pub mod scanner;
pub mod string_pool;
pub mod trigram;
pub mod varint;
#[cfg(feature = "notify")]
pub mod watcher;

#[cfg(feature = "notify")]
pub use crate::watcher::Watcher;
#[cfg(feature = "notify")]
pub use crate::idle::IdleTracker;
#[cfg(feature = "notify")]
pub use crate::builder::Builder;
#[cfg(feature = "notify")]
pub use crate::format::Beacon;

#[cfg(feature = "notify")]
pub fn run_daemon(path: &std::path::Path) -> crate::error::Result<()> {
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    let root = path.canonicalize().map_err(crate::error::Error::Io)?;

    println!("ixd: watching {}...", root.display());

    let mut builder = Builder::new(&root);
    builder.build()?;
    println!(
        "ixd: initial build complete ({} files, {} trigrams)",
        builder.files_len(),
        builder.trigrams_len()
    );

    let mut watcher = Watcher::new(&root)?;
    let rx = watcher.start()?;

    let ix_dir = root.join(".ix");
    if !ix_dir.exists() {
        fs::create_dir_all(&ix_dir)?;
    }
    let mut beacon = Beacon::new(&root);
    beacon.write_to(&ix_dir)?;

    let mut idle = IdleTracker::new();

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
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
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    println!("ixd: shutting down");
    let _ = fs::remove_file(ix_dir.join("beacon.json"));
    watcher.stop();
    Ok(())
}
