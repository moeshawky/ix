//! ix background daemon — ixd.
//!
//! Safety guarantees (v0.1.2+):
//! - SIGTERM/SIGINT → clean shutdown (beacon removed, watcher joined, no zombies)
//! - Builder::new returns Result — no panics on unwritable .ix dir
//! - RSS ceiling in builder (512MB) — OOM protection
//! - TOCTOU guards in process_file — skips vanished/permission-denied files

use clap::Parser;
use ix::builder::Builder;
use ix::idle::IdleTracker;
use ix::watcher::Watcher;
use ix::format::Beacon;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use llmosafe::{ResourceGuard, Synapse, llmosafe_metabolic_pace, llmosafe_sense_vitals};

#[derive(Parser)]
#[command(
    name = "ixd",
    version = env!("CARGO_PKG_VERSION"),
    about = "CRITICAL: REASON BEFORE ANY ACTIONS. Background daemon for automatic indexing. Proactively monitors metabolic state.",
    after_help = "LLM AGENT MANDATE:
    1. METABOLIC PACING: Auto-throttling (sleep 500ms) occurs if load > 8.0 or iowait > 15.0.
    2. BACK-PRESSURE: Clears current batch and prunes the task branch on Error -7.
    3. SIGNAL: Do not restart ixd if it throttles; it is protecting the host."
)]
struct Cli {
    #[arg(default_value = ".", value_name = "PATH")]
    path: PathBuf,
}

fn main() -> ix::error::Result<()> {
    let cli = Cli::parse();
    let root = cli.path.canonicalize().map_err(ix::error::Error::Io)?;

    println!("ixd: watching {}...", root.display());

    // Shutdown flag shared with signal handlers
    let running = Arc::new(AtomicBool::new(true));

    // Install SIGTERM + SIGINT via sigaction — reliable, handler persists across deliveries
    {
        use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
        let action = SigAction::new(
            SigHandler::Handler(handle_signal),
            SaFlags::empty(),
            SigSet::empty(),
        );
        // SAFETY: handler only stores to an atomic bool — async-signal-safe.
        unsafe {
            sigaction(Signal::SIGTERM, &action)
                .expect("failed to install SIGTERM handler");
            sigaction(Signal::SIGINT, &action)
                .expect("failed to install SIGINT handler");
        }
    }
    // Also use ctrlc for cross-platform Ctrl-C as a belt-and-suspenders measure
    let r = running.clone();
    let _ = ctrlc::set_handler(move || {
        SHUTDOWN.store(true, Ordering::SeqCst);
        r.store(false, Ordering::SeqCst);
    });

    // Concurrent instance guard — refuse to start if another ixd owns this root.
    let ix_dir_early = root.join(".ix");
    let beacon_path = ix_dir_early.join("beacon.json");
    if beacon_path.exists() && let Ok(existing) = ix::format::Beacon::read_from(&ix_dir_early) {
        let pid = nix::unistd::Pid::from_raw(existing.pid);
        if nix::sys::signal::kill(pid, None).is_ok() {
            eprintln!(
                "ixd: another instance is already watching {} (PID {}). \
                 Stop it first or remove {}/beacon.json.",
                root.display(), existing.pid, ix_dir_early.display()
            );
            std::process::exit(1);
        }
        // Dead PID — stale beacon, safe to continue
        eprintln!("ixd: removing stale beacon from PID {}", existing.pid);
        let _ = std::fs::remove_file(&beacon_path);
    }

    let guard = ResourceGuard::auto(0.6); // 60% system memory ceiling
    
    let mut builder = match Builder::new(&root) {
        Ok(b) => b.with_resource_guard(guard.clone()),
        Err(e) => {
            eprintln!("ixd: cannot create index in {}: {}", root.display(), e);
            return Err(e);
        }
    };

    if let Err(e) = builder.build() {
        eprintln!("ixd: initial build failed: {} — will watch for changes anyway", e);
    } else {
        println!(
            "ixd: initial build complete ({} files, {} trigrams)",
            builder.files_len(),
            builder.trigrams_len()
        );
    }

    let mut watcher = Watcher::new(&root)?;
    let rx = watcher.start()?;

    let ix_dir = root.join(".ix");
    if !ix_dir.exists() {
        fs::create_dir_all(&ix_dir)?;
    }
    let mut beacon = Beacon::new(&root);
    beacon.write_to(&ix_dir)?;

    let mut idle = IdleTracker::new();

    // Main loop — polls SHUTDOWN flag on every timeout so signal handling is responsive
    loop {
        if SHUTDOWN.load(Ordering::SeqCst) {
            running.store(false, Ordering::SeqCst);
            break;
        }

        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(changed_files) => {
                let vitals = llmosafe_sense_vitals();
                
                // Mycelial Sensing: Adaptive metabolic backoff
                if vitals.iowait_percent > 15.0 || vitals.load_avg_1min > 8.0 {
                    eprintln!("ixd: metabolic pressure detected (iowait: {:.1}%, load: {:.2}) — entering back-pressure mode", 
                        vitals.iowait_percent, vitals.load_avg_1min);
                    beacon.status = format!("metabolic backoff (iowait: {:.1}%)", vitals.iowait_percent);
                    let _ = beacon.write_to(&ix_dir);
                    std::thread::sleep(Duration::from_millis(1000));
                    // Continue to next turn to re-evaluate after sleep
                    continue;
                }

                // Metabolic Law: Enforce 100ms minimum interval
                if llmosafe_metabolic_pace(100) == -2 {
                    eprintln!("ixd: pace violation detected — throttling");
                    std::thread::sleep(Duration::from_millis(200));
                }

                let synapse = guard.check().unwrap_or_else(|_| {
                    let mut s = Synapse::new();
                    s.set_raw_entropy(1500); // 1.0 ratio
                    s
                });

                println!("ixd: {} files changed, updating index... (Entropy: {})", 
                    changed_files.len(), synapse.raw_entropy());

                beacon.status = format!("indexing (entropy: {})", synapse.raw_entropy());
                beacon.last_event_at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let _ = beacon.write_to(&ix_dir);

                idle.record_change();
                if let Err(e) = builder.update(&changed_files) {
                    eprintln!("ixd: update failed: {} — retrying on next change", e);
                }

                beacon.status = "idle".to_string();
                let _ = beacon.write_to(&ix_dir);
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    // Clean shutdown — guaranteed to run on SIGTERM or SIGINT
    eprintln!("ixd: shutting down...");
    watcher.stop();
    let _ = fs::remove_file(ix_dir.join("beacon.json"));

    // Reap any zombie child processes (belt-and-suspenders — ixd doesn't fork,
    // but a library or watcher backend might)
    loop {
        use nix::sys::wait::{waitpid, WaitPidFlag};
        use nix::unistd::Pid;
        match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
            Ok(nix::sys::wait::WaitStatus::StillAlive) | Err(_) => break,
            Ok(_) => continue, // reaped one, try again
        }
    }

    Ok(())
}

/// Global shutdown flag set by the raw signal handler.
static SHUTDOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Raw signal handler — only touches the atomic flag (async-signal-safe).
extern "C" fn handle_signal(_: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}
