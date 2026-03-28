//! ix background daemon — ixd.
//!
//! Keeps the index fresh when the system is idle.

use ix::builder::Builder;
use ix::idle::{DaemonState, IdleTracker};
use ix::watcher::Watcher;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn main() -> ix::error::Result<()> {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let root = PathBuf::from(root)
        .canonicalize()
        .map_err(ix::error::Error::Io)?;

    println!("ixd: watching {}...", root.display());

    let mut builder = Builder::new(&root);
    builder.build()?;
    println!(
        "ixd: initial build complete ({} files, {} trigrams)",
        builder.files_len(),
        builder.trigrams_len()
    );

    let mut watcher = Watcher::new(&root)?;
    let mut rx = watcher.start()?;

    let mut idle = IdleTracker::new();

    // Signal handling
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl-C handler");

    while running.load(Ordering::SeqCst) {
        // Wait for changes or 60s timeout for dormancy check
        match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(changed_files) => {
                println!(
                    "ixd: {} files changed, updating index...",
                    changed_files.len()
                );
                idle.record_change();
                builder.update(&changed_files)?;

                // If we were dormant, restart watcher
                if idle.state() != DaemonState::Dormant && !watcher.is_running() {
                    println!("ixd: system active, restarting watcher");
                    rx = watcher.start()?;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                // Dormancy check
                match idle.state() {
                    DaemonState::Dormant => {
                        if watcher.is_running() {
                            println!("ixd: system dormant, stopping watcher");
                            watcher.stop();
                        }
                    }
                    _ => {
                        // If we are active/idle but watcher was stopped, restart it
                        if !watcher.is_running() {
                            println!("ixd: system active, restarting watcher");
                            rx = watcher.start()?;
                        }
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    println!("ixd: shutting down");
    watcher.stop();
    Ok(())
}
