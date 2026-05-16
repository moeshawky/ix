//! Full guarded daemon implementation with `LLMOSafe` memory safety.
//!
//! This module contains the single background-loop implementation used by
//! both the `ixd` binary and `ix --daemon`. It enforces a 60% RSS ceiling,
//! checks system entropy via [`ResourceGuard`], and provides file-change
//! notifications over a Unix domain socket (`daemon_sock`).
//!
//! # Safety
//!
//! - Signal handlers (`SIGTERM`/`SIGINT`) are installed via `sigaction` and
//!   only write to an `AtomicBool` — async-signal-safe by construction.
//! - `ResourceGuard` monitors memory pressure; the daemon defers or halts
//!   index updates via multidimensional safety policy (entropy, surprise,
//!   bias, and explicit pressure level).
//! - The `.ix/` directory is excluded from file-watch events to prevent
//!   infinite rebuild loops.

use crate::builder::Builder;
use crate::daemon_sock::{DaemonServer, DaemonStatus, FileChange, FileOp, ServerMessage};
use crate::format::Beacon;
use crate::idle::IdleTracker;
use crate::watcher::Watcher;
use llmosafe::{EscalationPolicy, PressureLevel, ResourceGuard, SafetyDecision};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const ENTROPY_CRITICAL: u16 = 1000;
const PRE_BUILD_WAIT_SECS: u64 = 5;
const WARN_COOLDOWN_MS: u64 = 300;

struct DaemonCtx<'a> {
    builder: &'a mut Builder,
    ix_dir: &'a Path,
    beacon: &'a mut Beacon,
    idle: &'a mut IdleTracker,
    guard: &'a ResourceGuard,
    daemon_sock: Option<&'a DaemonServer>,
    running: &'a Arc<AtomicBool>,
}

/// Run the daemon, watching `root` for file changes and rebuilding the index.
///
/// This is the single canonical daemon entry point used by both `ix --daemon`
/// and the standalone `ixd` binary.
///
/// # Errors
///
/// Returns an error if the root cannot be canonicalised, the index cannot be
/// built, the file watcher fails, or a concurrent daemon instance is detected.
#[allow(clippy::too_many_lines)]
pub fn run(root: &Path) -> crate::error::Result<()> {
    SHUTDOWN.store(false, Ordering::SeqCst);

    let root = root.canonicalize().map_err(crate::error::Error::Io)?;

    println!("ixd: watching {}...", root.display());

    let running = Arc::new(AtomicBool::new(true));
    install_signal_handlers();

    // Concurrent instance guard — refuse to start if another ixd owns this root.
    let ix_dir_early = root.join(".ix");
    let beacon_path = ix_dir_early.join("beacon.json");
    if beacon_path.exists()
        && let Ok(existing) = Beacon::read_from(&ix_dir_early)
    {
        let pid = nix::unistd::Pid::from_raw(existing.pid);
        if nix::sys::signal::kill(pid, None).is_ok() {
            return Err(crate::error::Error::Config(format!(
                "another instance is already watching {} (PID {}). \
                 Stop it first or remove {}/beacon.json.",
                root.display(),
                existing.pid,
                ix_dir_early.display()
            )));
        }
        eprintln!("ixd: removing stale beacon from PID {}", existing.pid);
        let _ = std::fs::remove_file(&beacon_path);
    }

    let guard = ResourceGuard::auto(0.6);

    let mut builder = match Builder::new(&root) {
        Ok(b) => b.with_resource_guard(guard.clone()),
        Err(e) => {
            eprintln!("ixd: cannot create index in {}: {}", root.display(), e);
            return Err(e);
        }
    };

    wait_for_memory(&guard);

    if let Err(e) = builder.build() {
        eprintln!("ixd: initial build failed: {e} — will watch for changes anyway");
    } else {
        println!(
            "ixd: initial build complete ({} files, {} trigrams)",
            builder.files_len(),
            builder.trigrams_len()
        );
    }

    let mut watcher = Watcher::new(&root);
    let rx = watcher.start()?;

    let ix_dir = root.join(".ix");
    if !ix_dir.exists() {
        fs::create_dir_all(&ix_dir)?;
    }
    let mut beacon = Beacon::new(&root);
    beacon.write_to(&ix_dir)?;

    let mut idle = IdleTracker::new();

    let mut daemon_sock = match DaemonServer::new(&root) {
        Ok(s) => {
            println!("ixd: socket at {}", s.path().display());
            beacon.socket_path = Some(s.path().to_path_buf());
            let _ = beacon.write_to(&ix_dir);
            Some(s)
        }
        Err(e) => {
            eprintln!("ixd: warning: could not create daemon socket: {e}");
            None
        }
    };
    if let Some(ref mut s) = daemon_sock
        && let Err(e) = s.start()
    {
        eprintln!("ixd: failed to start socket server: {e}");
    }

    run_main_loop(
        &mut builder,
        &rx,
        &ix_dir,
        &mut beacon,
        &mut idle,
        &guard,
        &running,
        daemon_sock.as_ref(),
    );

    eprintln!("ixd: shutting down...");
    watcher.stop();
    let _ = fs::remove_file(ix_dir.join("beacon.json"));

    Ok(())
}

fn install_signal_handlers() {
    use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
    let action = SigAction::new(
        SigHandler::Handler(handle_signal),
        SaFlags::empty(),
        SigSet::empty(),
    );
    // SAFETY: handler only stores to an atomic bool — async-signal-safe.
    unsafe {
        sigaction(Signal::SIGTERM, &action).expect("failed to install SIGTERM handler");
        sigaction(Signal::SIGINT, &action).expect("failed to install SIGINT handler");
    }
}

fn wait_for_memory(guard: &ResourceGuard) {
    let pre_build_timeout = Duration::from_secs(30);
    let pre_build_start = std::time::Instant::now();
    while pre_build_start.elapsed() < pre_build_timeout {
        match guard.check_blocking() {
            Ok(_) => break,
            Err(e) => {
                eprintln!("ixd: memory pressure before initial build: {e:?} — waiting...");
                std::thread::sleep(Duration::from_secs(PRE_BUILD_WAIT_SECS));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_main_loop(
    builder: &mut Builder,
    rx: &crossbeam_channel::Receiver<Vec<PathBuf>>,
    ix_dir: &Path,
    beacon: &mut Beacon,
    idle: &mut IdleTracker,
    guard: &ResourceGuard,
    running: &Arc<AtomicBool>,
    daemon_sock: Option<&DaemonServer>,
) {
    loop {
        if SHUTDOWN.load(Ordering::SeqCst) {
            running.store(false, Ordering::SeqCst);
            // Graceful shutdown: notify clients before closing
            if let Some(sock) = daemon_sock {
                sock.shutdown_notify("signal", 1000);
            }
            break;
        }

        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(changed_files) => {
                let mut ctx = DaemonCtx {
                    builder,
                    ix_dir,
                    beacon,
                    idle,
                    guard,
                    daemon_sock,
                    running,
                };
                handle_changes(&mut ctx, &changed_files);
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn handle_changes(ctx: &mut DaemonCtx, changed_files: &[PathBuf]) {
    let (entropy, safety_decision) = evaluate_safety(ctx.guard);

    match &safety_decision {
        SafetyDecision::Halt(err, cooldown) => {
            eprintln!("ixd: critical safety decision (Halt: {err:?}) — pausing operations");
            ctx.beacon.status = "safety halt".to_string();
            let _ = ctx.beacon.write_to(ctx.ix_dir);
            std::thread::sleep(Duration::from_millis(u64::from(*cooldown)));
            return;
        }
        SafetyDecision::Exit(err) => {
            eprintln!("ixd: SAFETY EXIT (unrecoverable: {err:?}) — terminating");
            ctx.beacon.status = "safety exit".to_string();
            ctx.running.store(false, Ordering::SeqCst);
            return;
        }
        SafetyDecision::Escalate {
            entropy: esc_entropy,
            reason,
            cooldown_ms,
        } => {
            eprintln!(
                "ixd: safety escalation (entropy: {esc_entropy}, reason: {reason:?}) — throttling"
            );
            let deferred_status = DaemonStatus::Deferred {
                entropy: *esc_entropy,
            };
            ctx.beacon.status = deferred_status.to_string();
            let _ = ctx.beacon.write_to(ctx.ix_dir);
            if let Some(sock) = ctx.daemon_sock {
                sock.set_status(&deferred_status, ctx.builder.files_len());
            }
            std::thread::sleep(Duration::from_millis(u64::from(*cooldown_ms)));
            return;
        }
        SafetyDecision::Warn(reason) => {
            if safety_decision.severity() >= 2 {
                eprintln!(
                    "ixd: safety warning (severity {}): {reason}",
                    safety_decision.severity()
                );
                ctx.beacon.status = format!("warned: {reason}");
                let _ = ctx.beacon.write_to(ctx.ix_dir);
                std::thread::sleep(Duration::from_millis(WARN_COOLDOWN_MS));
            }
        }
        SafetyDecision::Proceed => {}
    }

    println!(
        "ixd: {} files changed, updating index... (Entropy: {entropy}, Decision: {safety_decision:?})",
        changed_files.len(),
    );

    let daemon_status = DaemonStatus::Indexing { entropy };
    ctx.beacon.status = daemon_status.to_string();
    ctx.beacon.last_event_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let _ = ctx.beacon.write_to(ctx.ix_dir);

    broadcast_status(
        ctx,
        &beacon_status_msg(&daemon_status, ctx.builder.files_len()),
    );

    ctx.idle.record_change();

    if let Err(e) = ctx.builder.update(changed_files) {
        eprintln!("ixd: update failed: {e} — retrying on next change");
    } else {
        tracing::debug!("ixd: index updated - caches will self-invalidate on next query");
    }

    broadcast_file_changes(ctx, changed_files);

    let idle_status = DaemonStatus::Idle;
    ctx.beacon.status = idle_status.to_string();
    let _ = ctx.beacon.write_to(ctx.ix_dir);
    if let Some(sock) = ctx.daemon_sock {
        sock.set_status(&idle_status, ctx.builder.files_len());
    }
}

fn evaluate_safety(guard: &ResourceGuard) -> (u16, SafetyDecision) {
    match guard.check_blocking() {
        Ok(synapse) => {
            let raw_entropy = synapse.raw_entropy();
            let surprise = synapse.raw_surprise();
            let has_bias = synapse.has_bias();
            let pressure = PressureLevel::from(guard.pressure());
            let policy = EscalationPolicy::default();
            let decision = policy.decide_with_pressure(raw_entropy, surprise, has_bias, pressure);
            (raw_entropy, decision)
        }
        Err(e) => {
            eprintln!("ixd: resource check error: {e:?} — proceeding with elevated caution");
            (
                ENTROPY_CRITICAL,
                SafetyDecision::Escalate {
                    entropy: ENTROPY_CRITICAL,
                    reason: llmosafe::llmosafe_integration::EscalationReason::ResourcePressure,
                    cooldown_ms: u32::from(ENTROPY_CRITICAL),
                },
            )
        }
    }
}

fn beacon_status_msg(status: &DaemonStatus, files: usize) -> ServerMessage {
    ServerMessage::Status {
        pid: std::process::id(),
        status: status.to_string(),
        files,
        daemon_status: Some(status.clone()),
    }
}

fn broadcast_status(ctx: &DaemonCtx, msg: &ServerMessage) {
    if let Some(sock) = ctx.daemon_sock {
        sock.broadcast(msg);
    }
}

fn broadcast_file_changes(ctx: &DaemonCtx, changed_files: &[PathBuf]) {
    let Some(sock) = ctx.daemon_sock else { return };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let changes: Vec<FileChange> = changed_files
        .iter()
        .map(|p| FileChange {
            path: p.clone(),
            mtime: now,
            op: FileOp::Modify,
        })
        .collect();
    sock.notify_changes(changes, ctx.builder.files_len());
}

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_signal(_: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}
