//! Full guarded daemon implementation with `LLMOSafe` memory safety.
//!
//! This module contains the single background-loop implementation used by
//! both the `ixd` binary and `ix --daemon`. It enforces a 60% RSS ceiling,
//! checks system entropy via `ResourceGuard`, and provides file-change
//! notifications over a Unix domain socket (`daemon_sock`).
//!
//! # Multi-root support (v0.9+)
//!
//! The daemon can watch multiple roots in one process. Each root runs on
//! its own thread with an independent [`Builder`], [`Watcher`], [`Beacon`],
//! and [`DaemonServer`]. Signal handling, a single `ResourceGuard`, and
//! the shutdown flag are shared across all roots.
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
use crate::cache_policy::AdaptiveCachePolicy;
use crate::config::Config;
use crate::daemon_sock::{
    DaemonServer, DaemonStatus, FileChange, FileOp, SearchCaches, ServerMessage,
};
use crate::format::{self, Beacon};
use crate::idle::IdleTracker;
use crate::neg_cache::NegCache;
use crate::posting_cache::PostingCache;
use crate::regex_pool::RegexPool;
use crate::watcher::Watcher;
use llmosafe::{
    DesignAssuranceLevel, EscalationPolicy, PressureLevel, ResourceGuard, SafetyDecision,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const ENTROPY_CRITICAL: u16 = 1000;

/// Per-root daemon context carrying the builder, index state, caches, and
/// lifecycle handles used by the main event loop.
struct DaemonCtx<'a> {
    builder: &'a mut Builder,
    ix_dir: &'a Path,
    beacon: &'a mut Beacon,
    idle: &'a mut IdleTracker,
    guard: &'a ResourceGuard,
    cache_policy: &'a AdaptiveCachePolicy,
    /// LRU cache for decoded posting lists, keyed by trigram.
    posting_cache: &'a Arc<PostingCache>,
    /// Negative-result cache skipping re-verification of non-matching files.
    neg_cache: &'a Arc<NegCache>,
    daemon_sock: Option<&'a DaemonServer>,
    running: &'a Arc<AtomicBool>,
    log_prefix: &'a str,
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
/// Delegates to [`run_many`] with a single-entry slice.
pub fn run(root: &Path) -> crate::error::Result<()> {
    run_many(&[root.to_path_buf()])
}

/// Run the daemon watching multiple roots simultaneously.
///
/// Each root runs on its own thread. Signal handlers and a shared
/// [`ResourceGuard`] are installed once for the whole process.
///
/// # Errors
///
/// Returns an error if any root fails fatally during start-up. Run-time
/// errors (e.g. a stalled watcher) are logged per-root and do not
/// propagate.
pub fn run_many(roots: &[PathBuf]) -> crate::error::Result<()> {
    if roots.is_empty() {
        return Err(crate::error::Error::Config(
            "at least one root directory is required".into(),
        ));
    }

    // Deduplicate — the concurrent guard catches same-root duplicates, but
    // this avoids spawning threads that will immediately fail.
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<PathBuf> = roots
        .iter()
        .filter_map(|r| {
            let canonical = match r.canonicalize() {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        "ixd: cannot canonicalize root {}: {e} -- using raw path",
                        r.display()
                    );
                    r.clone()
                }
            };
            if seen.insert(canonical.clone()) {
                Some(canonical)
            } else {
                None
            }
        })
        .collect();

    if unique.is_empty() {
        return Err(crate::error::Error::Config(
            "no valid root directories provided".into(),
        ));
    }

    SHUTDOWN.store(false, Ordering::SeqCst);
    install_signal_handlers()?;

    let guard = ResourceGuard::auto(0.6);
    let instance_id = format::instance_id_now();

    let mut handles = Vec::new();
    for root in &unique {
        let root = root.clone();
        let name = root_name(&root);
        let name2 = name.clone();
        let guard = guard.clone();
        let handle = std::thread::Builder::new()
            .name(format!("ixd-{name}"))
            .spawn(move || match run_single_root(&root, &guard, instance_id) {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("ixd [{name}]: fatal start-up error: {e}");
                }
            })
            .map_err(|e| {
                crate::error::Error::Config(format!("cannot spawn watcher thread for {name2}: {e}"))
            })?;
        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.join();
    }

    Ok(())
}

/// Core daemon logic for a single watched root.
///
/// This is the extracted body of [`run`]. It does **not** install signal
/// handlers or manage the process-wide shutdown flag — the caller
/// ([`run_many`] or [`run`]) provides a shared guard and a shared
/// shutdown signal via the static [`SHUTDOWN`] atomic.
fn run_single_root(
    root: &Path,
    guard: &ResourceGuard,
    instance_id: u64,
) -> crate::error::Result<()> {
    let root = root.canonicalize().map_err(crate::error::Error::Io)?;
    let name = root_name(&root);

    println!("ixd [{name}]: watching ...");

    let running = Arc::new(AtomicBool::new(true));

    // Concurrent instance guard — refuse another watcher for the same root.
    let ix_dir_early = root.join(".ix");
    let beacon_path = ix_dir_early.join("beacon.json");
    check_concurrent_instance(&ix_dir_early, &beacon_path, &root, instance_id)?;

    let mut builder = match Builder::new(&root) {
        Ok(b) => b.with_resource_guard(guard.clone()),
        Err(e) => {
            eprintln!("ixd [{name}]: cannot create index: {e}");
            return Err(e);
        }
    };

    // Discover and apply `.ixd.toml` configuration via shared helper
    // so `ix --build` and the daemon scope identically (audit F2).
    let (watch_roots, exclude_patterns, debounce_ms) =
        if let Ok(config) = Config::discover_under(&root) {
            if !config.exclude_patterns.is_empty() || !config.watch_roots.is_empty() {
                eprintln!(
                    "ixd [{name}]: loaded config — {} exclude patterns, {} watch roots",
                    config.exclude_patterns.len(),
                    config.watch_roots.len()
                );
            }
            let wr = config.watch_roots.clone();
            let ep = config.exclude_patterns.clone();
            let db = config.debounce_ms;
            builder = config.apply_to_builder(builder);
            (wr, ep, db)
        } else {
            (Vec::new(), Vec::new(), None)
        };

    // Cache policy for memory-pressure-driven cache management
    let ceiling_bytes = (ResourceGuard::system_memory_bytes().saturating_mul(3)) / 5;
    let cache_policy = AdaptiveCachePolicy::new_with_guard(guard.clone(), ceiling_bytes);

    // Cache layers managed by the adaptive cache policy. PostingCache
    // avoids re-decoding compressed posting lists; NegCache skips
    // re-verification of files known to produce no matches.
    let posting_cache = Arc::new(PostingCache::with_ceiling(ceiling_bytes));
    let neg_cache = Arc::new(NegCache::new(65_536));

    // Bundled cache handles for daemon-mode search queries via the socket server.
    let regex_pool = Arc::new(RegexPool::new(256));
    let caches = SearchCaches {
        posting_cache: Arc::clone(&posting_cache),
        neg_cache: Arc::clone(&neg_cache),
        regex_pool,
    };

    wait_for_memory(guard, &name);

    let build_failed_error: Option<String> = match builder.build() {
        Ok(_path) => {
            println!(
                "ixd [{name}]: initial build complete ({} files, {} trigrams)",
                builder.files_len(),
                builder.trigrams_len()
            );
            None
        }
        Err(e) => {
            eprintln!("ixd [{name}]: initial build failed: {e} — will watch for changes anyway");
            Some(format!("{e}"))
        }
    };

    let mut watcher = Watcher::new(&root, &watch_roots, &exclude_patterns);
    if let Some(ms) = debounce_ms {
        watcher = watcher.with_debounce(ms);
    }
    let rx = watcher.start()?;

    let ix_dir = root.join(".ix");
    if !ix_dir.exists() {
        fs::create_dir_all(&ix_dir)?;
    }
    let mut beacon = Beacon::with_instance_id(&root, instance_id);
    if let Some(ref err) = build_failed_error {
        beacon.status = DaemonStatus::BuildFailed { error: err.clone() }.to_string();
    }
    beacon.write_to(&ix_dir)?;

    let mut idle = IdleTracker::new();

    let mut daemon_sock = match DaemonServer::new(&root) {
        Ok(s) => {
            println!("ixd [{name}]: socket at {}", s.path().display());
            beacon.socket_path = Some(s.path().to_path_buf());
            if let Err(e) = beacon.write_to(&ix_dir) {
                eprintln!("ixd [{name}]: beacon write error: {e}");
            }
            Some(s)
        }
        Err(e) => {
            eprintln!("ixd [{name}]: warning: could not create daemon socket: {e}");
            None
        }
    };
    if let Some(ref mut s) = daemon_sock
        && let Err(e) = s.start()
    {
        eprintln!("ixd [{name}]: failed to start socket server: {e}");
    }

    // Wire the cache pipeline: SearchCaches are shared across all daemon
    // search queries via the socket server's shared state.
    if let Some(ref mut s) = daemon_sock {
        s.set_caches(caches.clone());
    }

    run_main_loop(
        &mut builder,
        &rx,
        &ix_dir,
        &mut beacon,
        &mut idle,
        guard,
        &cache_policy,
        &posting_cache,
        &neg_cache,
        &running,
        daemon_sock.as_ref(),
        &name,
    );

    eprintln!("ixd [{name}]: shutting down...");
    watcher.stop();
    if let Err(e) = fs::remove_file(ix_dir.join("beacon.json")) {
        eprintln!("ixd [{name}]: warning: could not remove beacon file: {e}");
    }

    Ok(())
}

/// Check for a concurrent instance watching the same root.
///
/// Uses the beacon `instance_id` field to distinguish stale beacons left by
/// a previous run of the same PID from live beacons written by another thread
/// in the current process.
fn check_concurrent_instance(
    ix_dir: &Path,
    beacon_path: &Path,
    root: &Path,
    instance_id: u64,
) -> crate::error::Result<()> {
    if !beacon_path.exists() {
        return Ok(());
    }
    let Ok(existing) = Beacon::read_from(ix_dir) else {
        tracing::warn!(
            "ixd: beacon file at {} is corrupted or unreadable \u{2014} \
             proceeding as if no conflicting instance exists;\
             consider removing the beacon file if this persists",
            beacon_path.display()
        );
        return Ok(());
    };
    let pid = nix::unistd::Pid::from_raw(existing.pid);

    // Process with this PID exists.
    if nix::sys::signal::kill(pid, None).is_ok() {
        let our_pid = i32::try_from(std::process::id()).unwrap_or(-1);

        // Same PID + same instance_id → another root in THIS process is
        // already watching this directory.
        if existing.pid == our_pid && existing.instance_id == instance_id {
            return Err(crate::error::Error::Config(format!(
                "another thread in this process is already watching {}. \
                 Remove duplicate `{}/beacon.json` to force.",
                root.display(),
                ix_dir.display()
            )));
        }

        // Same PID + different instance_id → stale beacon from a prior
        // run that reused this PID.
        if existing.pid == our_pid && existing.instance_id != instance_id {
            eprintln!(
                "ixd: removing stale beacon from PID {} (instance {} → {})",
                existing.pid, existing.instance_id, instance_id
            );
            let _ = std::fs::remove_file(beacon_path);
            return Ok(());
        }

        // Different PID → another process is watching this root.
        return Err(crate::error::Error::Config(format!(
            "another instance is already watching {} (PID {}). \
             Stop it first or remove `{}/beacon.json`.",
            root.display(),
            existing.pid,
            ix_dir.display()
        )));
    }

    // PID not alive — stale beacon.
    eprintln!("ixd: removing stale beacon from PID {}", existing.pid);
    let _ = std::fs::remove_file(beacon_path);
    Ok(())
}

fn install_signal_handlers() -> crate::error::Result<()> {
    use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
    let action = SigAction::new(
        SigHandler::Handler(handle_signal),
        SaFlags::empty(),
        SigSet::empty(),
    );
    // SAFETY: handler only stores to an atomic bool — async-signal-safe.
    unsafe {
        sigaction(Signal::SIGTERM, &action)
            .map_err(|e| crate::error::Error::Config(format!("SIGTERM handler: {e}")))?;
        sigaction(Signal::SIGINT, &action)
            .map_err(|e| crate::error::Error::Config(format!("SIGINT handler: {e}")))?;
    }
    Ok(())
}

fn wait_for_memory(guard: &ResourceGuard, log_prefix: &str) {
    const MAX_WAIT: u32 = 30;
    for attempt in 0..=MAX_WAIT {
        match guard.check_blocking() {
            Ok(_) => return,
            Err(e) => {
                if attempt == MAX_WAIT {
                    eprintln!(
                        "ixd [{log_prefix}]: memory check failed after {MAX_WAIT} attempts: {e:?} -- proceeding anyway"
                    );
                    return;
                }
                eprintln!(
                    "ixd [{log_prefix}]: memory pressure before initial build (attempt {}/{MAX_WAIT}): {e:?}",
                    attempt + 1
                );
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
}

/// Main event loop: receives file-change batches, dispatches to
/// [`handle_changes`], triggers idle compaction, and monitors shutdown.
///
/// Uses a 30 × 1 s timeout window instead of a single 30 s timeout so that
/// the `SIGTERM`/`SIGINT` signal (written to [`SHUTDOWN`] by the signal
/// handler) is detected within ~1 s rather than up to 30 s.  After 30
/// consecutive idle seconds the loop checks for a non-empty delta file and
/// calls [`compact`] if pressure permits.
///
/// # Parameters
///
/// * `builder` — index builder; mutated by change handling and compaction.
/// * `rx` — file-change channel from the [`Watcher`](crate::watcher::Watcher).
/// * `ix_dir` — `.ix/` directory holding the beacon and delta.
/// * `beacon` — daemon lifecycle beacon, updated on status transitions.
/// * `idle` — idle tracker marking dormant/active phases.
/// * `guard` — resource guard for memory-pressure decisions.
/// * `cache_policy` — adaptive cache policy driven by pressure.
/// * `posting_cache` / `neg_cache` — cache layers managed by the policy.
/// * `running` — set to `false` on graceful shutdown.
/// * `daemon_sock` — optional IPC socket for broadcasting status.
/// * `log_prefix` — human-readable root name for log messages.
// All parameters are independently sourced config values.
// The main loop is necessarily long \u{2014} it multiplexes event receipt,
// change handling, compaction, and shutdown in a single borrowed scope.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn run_main_loop(
    builder: &mut Builder,
    rx: &crossbeam_channel::Receiver<Vec<PathBuf>>,
    ix_dir: &Path,
    beacon: &mut Beacon,
    idle: &mut IdleTracker,
    guard: &ResourceGuard,
    cache_policy: &AdaptiveCachePolicy,
    posting_cache: &Arc<PostingCache>,
    neg_cache: &Arc<NegCache>,
    running: &Arc<AtomicBool>,
    daemon_sock: Option<&DaemonServer>,
    log_prefix: &str,
) {
    let (shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(1);
    SHUTDOWN_HANDLER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .replace(shutdown_tx);

    loop {
        let mut had_events = false;
        // 30 × 1s timeouts = ~30s idle detection; shutdown checked each second.
        for _ in 0..30 {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(changed_files) => {
                    had_events = true;
                    let mut ctx = DaemonCtx {
                        builder,
                        ix_dir,
                        beacon,
                        idle,
                        guard,
                        cache_policy,
                        posting_cache,
                        neg_cache,
                        daemon_sock,
                        running,
                        log_prefix,
                    };
                    handle_changes(&mut ctx, &changed_files, rx);
                    break;
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if shutdown_rx.try_recv().is_ok() || SHUTDOWN.load(Ordering::SeqCst) {
                        running.store(false, Ordering::SeqCst);
                        if let Some(sock) = daemon_sock {
                            sock.shutdown_notify("signal", 1000);
                        }
                        return;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    return;
                }
            }
        }
        // No events in 30 seconds → idle compaction window.
        if !had_events {
            let delta_file = ix_dir.join("shard.ix.delta");
            if delta_file.exists() && delta_file.metadata().map_or(0, |m| m.len()) > 0 {
                let pressure = guard.pressure();
                if pressure < 60 {
                    compact(
                        builder,
                        posting_cache,
                        neg_cache,
                        &delta_file,
                        beacon,
                        ix_dir,
                        daemon_sock,
                        log_prefix,
                        guard,
                    );
                    let mut deferred_batches = Vec::new();
                    while let Ok(batch) = rx.try_recv() {
                        deferred_batches.push(batch);
                    }
                    for batch in deferred_batches {
                        if let Err(e) = builder.update(&batch) {
                            tracing::warn!(
                                "ixd [{log_prefix}]: deferred update failed for {} files: {e}",
                                batch.len()
                            );
                        }
                    }
                }
            }
            // Shutdown check after compaction (also catches shutdown during
            // extended idle with empty delta).
            if shutdown_rx.try_recv().is_ok() || SHUTDOWN.load(Ordering::SeqCst) {
                running.store(false, Ordering::SeqCst);
                if let Some(sock) = daemon_sock {
                    sock.shutdown_notify("signal", 1000);
                }
                return;
            }
        }
    }
}

fn handle_changes(
    ctx: &mut DaemonCtx,
    changed_files: &[PathBuf],
    rx: &crossbeam_channel::Receiver<Vec<PathBuf>>,
) {
    let pressure = ctx.guard.pressure();
    // NOTE: `pressure()` and `check_blocking()` inside `evaluate_safety` are
    // not atomically synced — memory conditions can change between the two
    // calls. This is a pre-existing architecture concern (GAP-08), not a
    // regression. A future `ResourceGuard::check_with_pressure(p)` API would
    // close this gap.
    let (entropy, safety_decision) = evaluate_safety(ctx.guard, pressure, ctx.log_prefix);

    match &safety_decision {
        SafetyDecision::Halt(err, _cooldown) => {
            let prefix = ctx.log_prefix;
            eprintln!(
                "ixd [{prefix}]: critical safety decision (Halt: {err:?}) — pausing operations",
            );
            let halt_status = DaemonStatus::SafetyHalt;
            ctx.beacon.status = halt_status.to_string();
            if let Err(e) = ctx.beacon.write_to(ctx.ix_dir) {
                eprintln!("ixd [{prefix}]: beacon write error: {e}");
            }
            if let Some(sock) = ctx.daemon_sock {
                sock.set_status(&halt_status, ctx.builder.files_len());
            }
            broadcast_status(
                ctx,
                &beacon_status_msg(&halt_status, ctx.builder.files_len()),
            );
            if let Err(e) = ctx.guard.check_blocking() {
                eprintln!("ixd [{prefix}]: resource check failed: {e:?}");
            }
            return;
        }
        SafetyDecision::Exit(err) => {
            let prefix = ctx.log_prefix;
            eprintln!("ixd [{prefix}]: SAFETY EXIT (unrecoverable: {err:?}) — terminating");
            let exit_status = DaemonStatus::SafetyExit;
            ctx.beacon.status = exit_status.to_string();
            if let Err(e) = ctx.beacon.write_to(ctx.ix_dir) {
                eprintln!("ixd [{prefix}]: beacon write error: {e}");
            }
            if let Some(sock) = ctx.daemon_sock {
                sock.set_status(&exit_status, ctx.builder.files_len());
            }
            broadcast_status(
                ctx,
                &beacon_status_msg(&exit_status, ctx.builder.files_len()),
            );
            ctx.running.store(false, Ordering::SeqCst);
            return;
        }
        SafetyDecision::Escalate {
            entropy: esc_entropy,
            reason,
            ..
        } => {
            let prefix = ctx.log_prefix;
            eprintln!(
                "ixd [{prefix}]: safety escalation (entropy: {esc_entropy}, reason: {reason:?}) — throttling",
            );
            let escalated_status = DaemonStatus::Escalated {
                entropy: *esc_entropy,
            };
            ctx.beacon.status = escalated_status.to_string();
            if let Err(e) = ctx.beacon.write_to(ctx.ix_dir) {
                eprintln!("ixd [{prefix}]: beacon write error: {e}");
            }
            if let Some(sock) = ctx.daemon_sock {
                sock.set_status(&escalated_status, ctx.builder.files_len());
            }
            broadcast_status(
                ctx,
                &beacon_status_msg(&escalated_status, ctx.builder.files_len()),
            );
            if let Err(e) = ctx.guard.check_blocking() {
                eprintln!("ixd [{prefix}]: resource check failed: {e:?}");
            }
            return;
        }
        SafetyDecision::Warn(reason) => {
            if safety_decision.severity() >= 2 {
                let prefix = ctx.log_prefix;
                eprintln!(
                    "ixd [{prefix}]: safety warning (severity {}): {reason}",
                    safety_decision.severity()
                );
                let warned_status = DaemonStatus::Warned {
                    reason: reason.to_string(),
                };
                ctx.beacon.status = warned_status.to_string();
                if let Err(e) = ctx.beacon.write_to(ctx.ix_dir) {
                    eprintln!("ixd [{prefix}]: beacon write error: {e}");
                }
                if let Some(sock) = ctx.daemon_sock {
                    sock.set_status(&warned_status, ctx.builder.files_len());
                }
                broadcast_status(
                    ctx,
                    &beacon_status_msg(&warned_status, ctx.builder.files_len()),
                );
                if let Err(e) = ctx.guard.check_blocking() {
                    eprintln!("ixd [{prefix}]: resource check failed: {e:?}");
                }
            }
        }
        SafetyDecision::Proceed => {}
    }

    let prefix = ctx.log_prefix;
    println!(
        "ixd [{prefix}]: {} files changed, updating index... (Entropy: {entropy}, Decision: {safety_decision:?})",
        changed_files.len(),
    );

    let daemon_status = DaemonStatus::Indexing { entropy };
    ctx.beacon.status = daemon_status.to_string();
    ctx.beacon.last_event_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if let Err(e) = ctx.beacon.write_to(ctx.ix_dir) {
        eprintln!("ixd [{prefix}]: beacon write error: {e}");
    }

    broadcast_status(
        ctx,
        &beacon_status_msg(&daemon_status, ctx.builder.files_len()),
    );

    let prefix = ctx.log_prefix;
    let changed_count = changed_files.len();
    if let Err(e) = ctx.builder.update(changed_files) {
        eprintln!(
            "ixd [{prefix}]: update failed for {changed_count} files: {e} — retrying on next change"
        );
        tracing::warn!(
            "ixd [{}]: dropped {} changed files due to update error: {e}",
            prefix,
            changed_count,
        );
    } else {
        ctx.posting_cache.invalidate_all();
        ctx.neg_cache.clear();
        tracing::debug!("ixd [{prefix}]: index updated - caches invalidated");
    }

    let directive = ctx.cache_policy.directive_for_pressure(pressure);
    if directive.zone != crate::cache_policy::PressureZone::Green {
        tracing::debug!(
            "ixd [{}]: cache directive -- zone={:?}, pressure={}, evict={:.2}, admit={}",
            ctx.log_prefix,
            directive.zone,
            directive.pressure,
            directive.evict_fraction,
            directive.allow_new_entries
        );
    }

    // Apply the cache directive to live cache layers.
    // PostingCache admission is gated; NegCache supports fractional eviction
    // under memory pressure. Red zone (evict_fraction >= 1.0) flushes both.
    ctx.posting_cache.set_admit(directive.allow_new_entries);
    ctx.neg_cache.set_admit(directive.allow_new_entries);
    if directive.evict_fraction > 0.0 {
        ctx.neg_cache.evict_fraction(directive.evict_fraction);
        ctx.posting_cache.evict_fraction(directive.evict_fraction);
    }

    broadcast_file_changes(ctx, changed_files);

    // Wire DaemonStatus safety variants based on memory pressure zone.
    // Green: normal idle. Yellow/Orange: Warned with pressure reason.
    // Red: SafetyHalt — daemon survives but is cache-flushed.
    let post_index_status = match directive.zone {
        crate::cache_policy::PressureZone::Green => DaemonStatus::Idle,
        crate::cache_policy::PressureZone::Yellow => DaemonStatus::Warned {
            reason: format!("memory pressure: {}%", directive.pressure),
        },
        crate::cache_policy::PressureZone::Orange => DaemonStatus::Warned {
            reason: format!("critical memory pressure: {}%", directive.pressure),
        },
        crate::cache_policy::PressureZone::Red => DaemonStatus::SafetyHalt,
    };
    ctx.beacon.status = post_index_status.to_string();
    if let Err(e) = ctx.beacon.write_to(ctx.ix_dir) {
        eprintln!("ixd [{}]: beacon write error: {e}", ctx.log_prefix);
    }
    if let Some(sock) = ctx.daemon_sock {
        sock.set_status(&post_index_status, ctx.builder.files_len());
    }
    if post_index_status != DaemonStatus::Idle {
        broadcast_status(
            ctx,
            &beacon_status_msg(&post_index_status, ctx.builder.files_len()),
        );
    }

    let delta_file = ctx.ix_dir.join("shard.ix.delta");
    let delta_size = delta_file
        .exists()
        .then(|| delta_file.metadata().ok().map(|m| m.len()))
        .flatten()
        .unwrap_or(0);
    if delta_size > 0
        && (ctx.idle.state() == crate::idle::DaemonState::Dormant || delta_size > 50 * 1024 * 1024)
    {
        ctx.beacon.status = "compacting".to_string();
        if let Err(e) = ctx.beacon.write_to(ctx.ix_dir) {
            eprintln!("ixd [{}]: beacon write error: {e}", ctx.log_prefix);
        }
        match ctx.builder.build() {
            Ok(_) => {
                ctx.posting_cache.invalidate_all();
                ctx.neg_cache.clear();
                tracing::debug!(
                    "ixd [{}]: compaction complete - caches invalidated",
                    ctx.log_prefix
                );
            }
            Err(e) => {
                eprintln!("ixd [{}]: compaction failed: {}", ctx.log_prefix, e);
            }
        }
        let mut deferred_batches = Vec::new();
        while let Ok(batch) = rx.try_recv() {
            deferred_batches.push(batch);
        }
        for batch in deferred_batches {
            if let Err(e) = ctx.builder.update(&batch) {
                tracing::warn!(
                    "ixd [{}]: deferred update (post-compaction) failed for {} files: {e}",
                    ctx.log_prefix,
                    batch.len()
                );
            }
        }
        // Reset the beacon after inline compaction completes (success or failure),
        // mirroring `compact()` (lines 846-851). Without this reset, the beacon
        // remains stuck at "compacting" indefinitely, breaking health reporting
        // and causing downstream consumers (e.g. codegraph-ferrari) to treat the
        // daemon as perpetually busy. Caches were just invalidated on success, so
        // pressure post-compaction is lower than the pre-compaction reading —
        // `Idle` is the correct terminal state (the next `handle_changes` tick
        // recomputes the pressure zone from scratch at lines 700/726-735).
        let idle_status = DaemonStatus::Idle;
        ctx.beacon.status = idle_status.to_string();
        if let Err(e) = ctx.beacon.write_to(ctx.ix_dir) {
            eprintln!("ixd [{}]: beacon write error: {e}", ctx.log_prefix);
        }
        if let Some(sock) = ctx.daemon_sock {
            sock.set_status(&idle_status, ctx.builder.files_len());
        }
    }
    ctx.idle.record_change();
}

/// Rebuild the full index from the current state plus the accumulated delta.
///
/// Called during idle periods when the delta file exists and has non-zero size.
/// Performs a second pressure check before calling [`Builder::build`] to close
/// the TOCTOU window between the caller's pressure read and the expensive build.
/// If pressure has risen to ≥60%, the build is skipped and the delta file is
/// preserved for the next idle cycle.
///
/// # Parameters
///
/// * `builder` — index builder holding accumulated incremental state.
/// * `posting_cache` / `neg_cache` — cache layers invalidated on successful build.
/// * `delta_file` — path to the delta accumulation file (`shard.ix.delta`).
/// * `beacon` / `ix_dir` — daemon beacon written before and after the build.
/// * `daemon_sock` — optional IPC socket; status broadcast on idle transition.
/// * `log_prefix` — human-readable root name for log messages.
/// * `guard` — resource guard for the second pressure check.
#[allow(clippy::too_many_arguments)]
fn compact(
    builder: &mut Builder,
    posting_cache: &Arc<PostingCache>,
    neg_cache: &Arc<NegCache>,
    delta_file: &Path,
    beacon: &mut Beacon,
    ix_dir: &Path,
    daemon_sock: Option<&DaemonServer>,
    log_prefix: &str,
    guard: &ResourceGuard,
) {
    if !delta_file.exists() || delta_file.metadata().map_or(0, |m| m.len()) == 0 {
        return;
    }
    // Re-check pressure immediately before the expensive build — memory
    // conditions can change between the caller's check and this point.
    let pressure = guard.pressure();
    if pressure >= 60 {
        tracing::debug!(
            "ixd [{log_prefix}]: skipping compaction \u{2014} pressure {pressure}% >= 60%"
        );
        return;
    }
    beacon.status = "compacting".to_string();
    let _ = beacon.write_to(ix_dir);
    match builder.build() {
        Ok(_) => {
            posting_cache.invalidate_all();
            neg_cache.clear();
            tracing::debug!("ixd [{log_prefix}]: compaction complete - caches invalidated");
        }
        Err(e) => {
            eprintln!("ixd [{log_prefix}]: compaction failed: {e}");
        }
    }
    let idle_status = DaemonStatus::Idle;
    beacon.status = idle_status.to_string();
    let _ = beacon.write_to(ix_dir);
    if let Some(sock) = daemon_sock {
        sock.set_status(&idle_status, builder.files_len());
    }
}

fn evaluate_safety(guard: &ResourceGuard, pressure: u8, log_prefix: &str) -> (u16, SafetyDecision) {
    match guard.check_blocking() {
        Ok(synapse) => {
            let raw_entropy = synapse.raw_entropy();
            let surprise = synapse.raw_surprise();
            let has_bias = synapse.has_bias();
            let pressure_level = PressureLevel::from(pressure);
            let policy = EscalationPolicy::default().with_dal(DesignAssuranceLevel::A);
            let decision =
                policy.decide_with_pressure(raw_entropy, surprise, has_bias, pressure_level);
            (raw_entropy, decision)
        }
        Err(e) => {
            eprintln!(
                "ixd [{log_prefix}]: resource check error: {e:?} — proceeding with elevated caution",
            );
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

/// Unified helper for setting the beacon status string, writing the beacon
/// file to disk, updating the daemon socket status, and optionally
/// broadcasting the change to connected clients.
///
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

/// Short human-readable name for a root path (last component, or full path).
fn root_name(root: &Path) -> String {
    root.file_name()
        .and_then(|n| n.to_str())
        .map_or_else(|| root.display().to_string(), String::from)
}

static SHUTDOWN_HANDLER: std::sync::Mutex<Option<crossbeam_channel::Sender<()>>> =
    std::sync::Mutex::new(None);

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_signal(_: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}
