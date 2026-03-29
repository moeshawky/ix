//! ix background daemon — ixd.
//!
//! Keeps the index fresh when the system is idle.

use clap::Parser;
use ix::builder::Builder;
use ix::idle::{IdleTracker};
use ix::watcher::Watcher;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "ixd",
    version = "0.1.0",
    about = "ix background daemon. Automatically maintains a fresh trigram index.",
    after_help = "The daemon monitors the filesystem for changes and incrementally updates the index. 
It remains active in the background to ensure the index is always fresh."
)]
struct Cli {
    /// The directory to watch and index.
    #[arg(default_value = ".", value_name = "PATH")]
    path: PathBuf,
}

fn main() -> ix::error::Result<()> {
    let cli = Cli::parse();
    let root = cli.path.canonicalize().map_err(ix::error::Error::Io)?;

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

    let mut idle = IdleTracker::new();

    // Signal handling
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl-C handler");

    while running.load(Ordering::SeqCst) {
        // Wait for changes. We use a timeout to allow checking the 'running' flag.
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(changed_files) => {
                println!(
                    "ixd: {} files changed, updating index...",
                    changed_files.len()
                );

                // Update beacon status
                beacon.status = "indexing".to_string();
                beacon.last_event_at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let _ = beacon.write_to(&ix_dir);

                idle.record_change();
                builder.update(&changed_files)?;

                // Back to idle
                beacon.status = "idle".to_string();
                let _ = beacon.write_to(&ix_dir);
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                // Just loop to check 'running' flag
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
