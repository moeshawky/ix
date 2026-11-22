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
use crate::cache_policy::{
    AdaptiveCachePolicy, CacheDirective, PressureZone, ceiling_bytes_for_fraction,
    system_memory_bytes,
};
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
use nix::fcntl::{Flock, FlockArg};
use std::fs::{self, File, OpenOptions};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::Duration;

const ENTROPY_CRITICAL: u16 = 1000;

/// Per-root daemon context carrying the builder, index state, caches, and
/// lifecycle handles used by the main event loop.
struct DaemonCtx<'a> {
    builder: &'a mut Builder,
    ix_dir: &'a Path,
    beacon: &'a mut Beacon,
    idle: Arc<Mutex<IdleTracker>>,
    guard: &'a ResourceGuard,
    cache_policy: &'a AdaptiveCachePolicy,
    /// LRU cache for decoded posting lists, keyed by trigram.
    posting_cache: &'a Arc<PostingCache>,
    /// Negative-result cache skipping re-verification of non-matching files.
    neg_cache: &'a Arc<NegCache>,
    daemon_sock: Option<&'a DaemonServer>,
    index_lock: &'a Arc<RwLock<()>>,
    log_prefix: &'a str,
}

/// Owned state produced by [`init_single_root`] and consumed by [`run_root_loop`].
struct InitializedRoot {
    _instance_lock: Flock<File>,
    builder: Builder,
    rx: crossbeam_channel::Receiver<Vec<PathBuf>>,
    watcher: Watcher,
    ix_dir: PathBuf,
    beacon: Beacon,
    idle: Arc<Mutex<IdleTracker>>,
    cache_policy: AdaptiveCachePolicy,
    posting_cache: Arc<PostingCache>,
    neg_cache: Arc<NegCache>,
    daemon_sock: Option<DaemonServer>,
    index_lock: Arc<RwLock<()>>,
    name: String,
}

enum SafetyOutcome {
    Proceed,
    SkipBatch,
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

    let guard = crate::cache_policy::resource_guard_auto(0.6);
    let instance_id = format::instance_id_now();
    let (startup_tx, startup_rx) = mpsc::sync_channel::<Result<(), String>>(unique.len());

    let mut handles = Vec::with_capacity(unique.len());
    for root in &unique {
        let root = root.clone();
        let name = root_name(&root);
        let name2 = name.clone();
        let guard = guard.clone();
        let tx = startup_tx.clone();
        let handle = std::thread::Builder::new()
            .name(format!("ixd-{name}"))
            .spawn(move || run_single_root(&root, &guard, instance_id, &tx))
            .map_err(|e| {
                crate::error::Error::Config(format!("cannot spawn watcher thread for {name2}: {e}"))
            })?;
        handles.push(handle);
    }
    drop(startup_tx);

    let mut startup_err: Option<String> = None;
    for _ in 0..unique.len() {
        match startup_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                if startup_err.is_none() {
                    startup_err = Some(err);
                }
            }
            Err(_) => {
                if startup_err.is_none() {
                    startup_err = Some("root thread exited before completing startup".into());
                }
            }
        }
    }

    if let Some(err) = startup_err {
        SHUTDOWN.store(true, Ordering::SeqCst);
        for handle in handles {
            if let Err(panic_payload) = handle.join() {
                tracing::error!(
                    "ixd: root watcher thread panicked during startup rollback: {panic_payload:?}"
                );
            }
        }
        return Err(crate::error::Error::Config(err));
    }

    let mut thread_panics = 0usize;
    for handle in handles {
        if let Err(panic_payload) = handle.join() {
            thread_panics += 1;
            tracing::error!("ixd: root watcher thread panicked: {panic_payload:?}");
        }
    }
    if thread_panics > 0 {
        return Err(crate::error::Error::Config(format!(
            "ixd: {thread_panics} root watcher thread(s) panicked"
        )));
    }

    Ok(())
}

fn run_single_root(
    root: &Path,
    guard: &ResourceGuard,
    instance_id: u64,
    startup_tx: &mpsc::SyncSender<Result<(), String>>,
) {
    let mut state = match init_single_root(root, guard, instance_id) {
        Ok(s) => {
            let _ = startup_tx.send(Ok(()));
            s
        }
        Err(e) => {
            let msg = format!("{e}");
            eprintln!("ixd [{}]: fatal start-up error: {msg}", root_name(root));
            let _ = startup_tx.send(Err(msg));
            return;
        }
    };

    run_root_loop(&mut state, guard);
}

fn load_root_config(
    root: &Path,
    name: &str,
) -> (Vec<PathBuf>, Vec<String>, Option<u64>, Option<Config>) {
    match Config::discover_under(root) {
        Ok(config) => {
            if !config.exclude_patterns.is_empty() || !config.watch_roots.is_empty() {
                eprintln!(
                    "ixd [{name}]: loaded config \u{2014} {} exclude patterns, {} watch roots",
                    config.exclude_patterns.len(),
                    config.watch_roots.len()
                );
            }
            let wr = config.watch_roots.clone();
            let ep = config.exclude_patterns.clone();
            let db = config.debounce_ms;
            (wr, ep, db, Some(config))
        }
        Err(e) => {
            eprintln!(
                "ixd [{name}]: warning: failed to load .ixd.toml: {e} \u{2014} using defaults"
            );
            tracing::warn!("ixd [{name}]: failed to load .ixd.toml: {e} \u{2014} using defaults");
            (Vec::new(), Vec::new(), None, None)
        }
    }
}

fn start_daemon_socket(
    root: &Path,
    name: &str,
    index_lock: &Arc<RwLock<()>>,
    caches: SearchCaches,
    idle: &Arc<Mutex<IdleTracker>>,
    initial_status: &DaemonStatus,
    files_len: usize,
    beacon: &mut Beacon,
) -> Option<DaemonServer> {
    let mut daemon_sock = match DaemonServer::new_with_search_lock(root, Arc::clone(index_lock)) {
        Ok(s) => {
            println!("ixd [{name}]: socket at {}", s.path().display());
            beacon.socket_path = Some(s.path().to_path_buf());
            Some(s)
        }
        Err(e) => {
            eprintln!("ixd [{name}]: warning: could not create daemon socket: {e}");
            None
        }
    };

    if let Some(ref mut s) = daemon_sock {
        s.set_caches(caches);
        s.set_idle_tracker(Arc::clone(idle));
        s.set_status(initial_status, files_len);
        if let Err(e) = s.start() {
            eprintln!("ixd [{name}]: failed to start socket server: {e}");
        }
    }
    daemon_sock
}

fn init_single_root(
    root: &Path,
    guard: &ResourceGuard,
    instance_id: u64,
) -> crate::error::Result<InitializedRoot> {
    let root = root.canonicalize().map_err(crate::error::Error::Io)?;
    let name = root_name(&root);
    println!("ixd [{name}]: watching ...");

    let ix_dir = root.join(".ix");
    let instance_lock = acquire_instance_lock(&ix_dir, &root)?;
    let beacon_path = ix_dir.join("beacon.json");
    check_concurrent_instance(&ix_dir, &beacon_path, &root, instance_id)?;

    let mut builder = Builder::new(&root)
        .inspect_err(|e| eprintln!("ixd [{name}]: cannot create index: {e}"))?
        .with_resource_guard(guard.clone());

    let (watch_roots, exclude_patterns, debounce_ms, config_opt) = load_root_config(&root, &name);
    if let Some(config) = config_opt {
        builder = config.apply_to_builder(builder);
    }

    let guard_ceiling_bytes = ceiling_bytes_for_fraction(0.6);
    let cache_policy = AdaptiveCachePolicy::new_with_guard(guard.clone(), guard_ceiling_bytes);

    // Budget PostingCache strictly below ResourceGuard's 60% ceiling so LRU
    // eviction trims caches before global pressure enters warning/critical zones.
    let posting_cache_ceiling = (system_memory_bytes() / 4)
        .max(64 * 1024 * 1024)
        .min(guard_ceiling_bytes / 2);
    let posting_cache = Arc::new(PostingCache::with_ceiling(posting_cache_ceiling));
    let neg_cache = Arc::new(NegCache::new(65_536));
    let regex_pool = Arc::new(RegexPool::new(256));
    let index_lock = Arc::new(RwLock::new(()));

    let caches = SearchCaches {
        posting_cache: Arc::clone(&posting_cache),
        neg_cache: Arc::clone(&neg_cache),
        regex_pool,
        resource_guard: Some(Arc::new(guard.clone())),
        resource_guard_ceiling: Some(guard_ceiling_bytes),
    };

    wait_for_memory(guard, &name);

    let build_failed_error: Option<String> = {
        let _write_guard = index_lock
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match builder.build() {
            Ok(_path) => {
                println!(
                    "ixd [{name}]: initial build complete ({} files, {} trigrams)",
                    builder.files_len(),
                    builder.trigrams_len()
                );
                None
            }
            Err(e) => {
                eprintln!(
                    "ixd [{name}]: initial build failed: {e} \u{2014} will watch for changes anyway"
                );
                Some(format!("{e}"))
            }
        }
    };

    let mut watcher = Watcher::new(&root, &watch_roots, &exclude_patterns);
    if let Some(ms) = debounce_ms {
        watcher = watcher.with_debounce(ms);
    }
    let rx = watcher.start()?;
    if !ix_dir.exists() {
        fs::create_dir_all(&ix_dir)?;
    }

    let initial_status = match build_failed_error {
        Some(err) => DaemonStatus::BuildFailed { error: err },
        None => DaemonStatus::Idle,
    };
    let mut beacon = Beacon::with_instance_id(&root, instance_id);
    beacon.status = initial_status.to_string();
    let idle = Arc::new(Mutex::new(IdleTracker::new()));
    let daemon_sock = start_daemon_socket(
        &root,
        &name,
        &index_lock,
        caches,
        &idle,
        &initial_status,
        builder.files_len(),
        &mut beacon,
    );
    beacon.write_to(&ix_dir)?;

    Ok(InitializedRoot {
        _instance_lock: instance_lock,
        builder,
        rx,
        watcher,
        ix_dir,
        beacon,
        idle,
        cache_policy,
        posting_cache,
        neg_cache,
        daemon_sock,
        index_lock,
        name,
    })
}

fn run_root_loop(state: &mut InitializedRoot, guard: &ResourceGuard) {
    let name = state.name.clone();
    {
        let mut ctx = DaemonCtx {
            builder: &mut state.builder,
            ix_dir: &state.ix_dir,
            beacon: &mut state.beacon,
            idle: Arc::clone(&state.idle),
            guard,
            cache_policy: &state.cache_policy,
            posting_cache: &state.posting_cache,
            neg_cache: &state.neg_cache,
            daemon_sock: state.daemon_sock.as_ref(),
            index_lock: &state.index_lock,
            log_prefix: &name,
        };
        run_main_loop(&mut ctx, &state.rx);
    }

    eprintln!("ixd [{name}]: shutting down...");
    state.watcher.stop();
    if let Err(e) = fs::remove_file(state.ix_dir.join("beacon.json")) {
        eprintln!("ixd [{name}]: warning: could not remove beacon file: {e}");
    }
}

/// Acquire an exclusive non-blocking `flock` on `.ix/ixd.lock`.
///
/// The lock file is intentionally left in place after drop so concurrent
/// callers always synchronize against the same inode.
fn acquire_instance_lock(ix_dir: &Path, root: &Path) -> crate::error::Result<Flock<File>> {
    fs::create_dir_all(ix_dir)?;
    let lock_path = ix_dir.join("ixd.lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    match Flock::lock(lock_file, FlockArg::LockExclusiveNonblock) {
        Ok(flock) => Ok(flock),
        Err((_, errno))
            if errno == nix::errno::Errno::EAGAIN || errno == nix::errno::Errno::EWOULDBLOCK =>
        {
            Err(crate::error::Error::Config(format!(
                "another ixd instance already holds the lock for {}",
                root.display()
            )))
        }
        Err((_, errno)) => Err(crate::error::Error::Io(std::io::Error::from_raw_os_error(
            errno as i32,
        ))),
    }
}

/// Check for a concurrent instance watching the same root.
///
/// Rejects non-positive PIDs (never calls `kill(0, 0)` or `kill(-1, 0)`),
/// treats `EPERM` as a live instance owned by another user, and verifies
/// `/proc/{pid}/comm == "ixd"` on Linux to dismiss stale PID-recycled beacons.
fn check_concurrent_instance(
    ix_dir: &Path,
    beacon_path: &Path,
    root: &Path,
    instance_id: u64,
) -> crate::error::Result<()> {
    if !beacon_path.exists() {
        return Ok(());
    }
    let existing = match Beacon::read_from(ix_dir) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                "ixd: beacon file at {} is corrupted or unreadable ({e}) \u{2014} \
                 probing socket before clearing",
                beacon_path.display()
            );
            if crate::daemon_sock::DaemonClient::connect(root).is_ok() {
                return Err(crate::error::Error::Config(format!(
                    "another live ixd instance is responding on the socket for {} despite corrupt beacon",
                    root.display()
                )));
            }
            let _ = fs::remove_file(beacon_path);
            return Ok(());
        }
    };

    if existing.pid <= 0 {
        eprintln!(
            "ixd: removing invalid beacon with non-positive PID {}",
            existing.pid
        );
        let _ = fs::remove_file(beacon_path);
        return Ok(());
    }

    let pid = nix::unistd::Pid::from_raw(existing.pid);
    match nix::sys::signal::kill(pid, None) {
        Ok(()) => verify_live_pid_conflict(&existing, ix_dir, beacon_path, root, instance_id),
        Err(nix::errno::Errno::EPERM) => {
            #[cfg(target_os = "linux")]
            if let Ok(comm) = fs::read_to_string(format!("/proc/{}/comm", existing.pid))
                && comm.trim() != "ixd"
            {
                eprintln!(
                    "ixd: removing stale beacon from non-ixd PID {} ({})",
                    existing.pid,
                    comm.trim()
                );
                let _ = fs::remove_file(beacon_path);
                return Ok(());
            }
            Err(crate::error::Error::Config(format!(
                "another instance owned by another user is already watching {} (PID {})",
                root.display(),
                existing.pid,
            )))
        }
        Err(nix::errno::Errno::ESRCH) => {
            eprintln!("ixd: removing stale beacon from PID {}", existing.pid);
            let _ = fs::remove_file(beacon_path);
            Ok(())
        }
        Err(errno) => {
            tracing::warn!(
                "ixd: unexpected error probing PID {} from {}: {errno}",
                existing.pid,
                beacon_path.display()
            );
            Ok(())
        }
    }
}

fn verify_live_pid_conflict(
    existing: &Beacon,
    ix_dir: &Path,
    beacon_path: &Path,
    root: &Path,
    instance_id: u64,
) -> crate::error::Result<()> {
    let our_pid = i32::try_from(std::process::id()).unwrap_or(-1);
    if existing.pid == our_pid {
        if existing.instance_id == instance_id {
            return Err(crate::error::Error::Config(format!(
                "another thread in this process is already watching {}. \
                 Remove duplicate `{}/beacon.json` to force.",
                root.display(),
                ix_dir.display()
            )));
        }
        eprintln!(
            "ixd: removing stale beacon from PID {} (instance {} \u{2192} {})",
            existing.pid, existing.instance_id, instance_id
        );
        let _ = fs::remove_file(beacon_path);
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    match fs::read_to_string(format!("/proc/{}/comm", existing.pid)) {
        Ok(comm) if comm.trim() != "ixd" => {
            eprintln!(
                "ixd: removing stale beacon from recycled PID {} (comm={})",
                existing.pid,
                comm.trim()
            );
            let _ = fs::remove_file(beacon_path);
            return Ok(());
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let _ = fs::remove_file(beacon_path);
            return Ok(());
        }
        Ok(_) | Err(_) => {}
    }

    Err(crate::error::Error::Config(format!(
        "another instance is already watching {} (PID {}). \
         Stop it first or remove `{}/beacon.json`.",
        root.display(),
        existing.pid,
        ix_dir.display()
    )))
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
    const MAX_WAIT: u32 = 5;
    for attempt in 0..=MAX_WAIT {
        match guard.check() {
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
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

fn check_shutdown_and_notify(daemon_sock: Option<&DaemonServer>) -> bool {
    if SHUTDOWN.load(Ordering::SeqCst) {
        if let Some(sock) = daemon_sock {
            sock.shutdown_notify("signal", 1000);
        }
        return true;
    }
    false
}

/// Main event loop: receives file-change batches, dispatches to
/// [`handle_changes`], triggers idle compaction only when not `Active`,
/// and monitors shutdown continuously.
fn run_main_loop(ctx: &mut DaemonCtx<'_>, rx: &crossbeam_channel::Receiver<Vec<PathBuf>>) {
    loop {
        if check_shutdown_and_notify(ctx.daemon_sock) {
            return;
        }

        let mut had_events = false;
        // 30 x 1s timeouts = ~30s idle detection; shutdown checked every second.
        for _ in 0..30 {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(changed_files) => {
                    if check_shutdown_and_notify(ctx.daemon_sock) {
                        return;
                    }
                    had_events = true;
                    if handle_changes(ctx, &changed_files, rx).is_break() {
                        return;
                    }
                    if check_shutdown_and_notify(ctx.daemon_sock) {
                        return;
                    }
                    break;
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if check_shutdown_and_notify(ctx.daemon_sock) {
                        return;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    return;
                }
            }
        }

        if !had_events {
            let delta_file = ctx.ix_dir.join("shard.ix.delta");
            let is_active = ctx
                .idle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .state()
                == crate::idle::DaemonState::Active;
            let pressure = ctx.cache_policy.live_pressure();
            if !is_active
                && pressure < 60
                && delta_file.exists()
                && delta_file.metadata().map_or(0, |m| m.len()) > 0
            {
                run_compaction(ctx, rx, &delta_file, &DaemonStatus::Idle);
            }
            if check_shutdown_and_notify(ctx.daemon_sock) {
                return;
            }
        }
    }
}

fn handle_changes(
    ctx: &mut DaemonCtx<'_>,
    changed_files: &[PathBuf],
    rx: &crossbeam_channel::Receiver<Vec<PathBuf>>,
) -> ControlFlow<()> {
    let pressure = ctx.cache_policy.live_pressure();
    let (entropy, safety_decision) = evaluate_safety(ctx.guard, pressure, ctx.log_prefix);
    let directive = ctx.cache_policy.directive_for_pressure(pressure);

    match handle_safety_decision(ctx, safety_decision, &directive) {
        ControlFlow::Break(()) => return ControlFlow::Break(()),
        ControlFlow::Continue(SafetyOutcome::SkipBatch) => return ControlFlow::Continue(()),
        ControlFlow::Continue(SafetyOutcome::Proceed) => {}
    }

    let prefix = ctx.log_prefix;
    println!(
        "ixd [{prefix}]: {} files changed, updating index... (Entropy: {entropy}, Decision: {safety_decision:?})",
        changed_files.len(),
    );

    let daemon_status = DaemonStatus::Indexing { entropy };
    sync_status(ctx, &daemon_status, true);

    let changed_count = changed_files.len();
    let write_guard = ctx
        .index_lock
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let update_res = ctx.builder.update(changed_files);
    if update_res.is_ok() {
        ctx.posting_cache.invalidate_all();
        ctx.neg_cache.clear();
    }
    drop(write_guard);

    if directive.zone != PressureZone::Green {
        tracing::debug!(
            "ixd [{}]: cache directive -- zone={:?}, pressure={}, evict={:.2}, admit={}",
            ctx.log_prefix,
            directive.zone,
            directive.pressure,
            directive.evict_fraction,
            directive.allow_new_entries
        );
    }
    apply_cache_directive(ctx, &directive);

    let post_index_status = match update_res {
        Ok(_) => {
            ctx.beacon.last_event_at = current_unix_secs();
            tracing::debug!("ixd [{prefix}]: index updated - caches invalidated");
            broadcast_file_changes(ctx, changed_files);
            status_for_zone(directive.zone, directive.pressure)
        }
        Err(e) => {
            eprintln!(
                "ixd [{prefix}]: update failed for {changed_count} files: {e} \u{2014} retrying on next change"
            );
            tracing::warn!(
                "ixd [{}]: dropped {} changed files due to update error: {e}",
                prefix,
                changed_count,
            );
            DaemonStatus::BuildFailed {
                error: format!("{e}"),
            }
        }
    };

    let should_broadcast = post_index_status != DaemonStatus::Idle;
    sync_status(ctx, &post_index_status, should_broadcast);

    let delta_file = ctx.ix_dir.join("shard.ix.delta");
    let delta_size = delta_file
        .exists()
        .then(|| delta_file.metadata().ok().map(|m| m.len()))
        .flatten()
        .unwrap_or(0);
    let is_dormant = ctx
        .idle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .state()
        == crate::idle::DaemonState::Dormant;

    if delta_size > 0
        && pressure < 60
        && directive.zone != PressureZone::Red
        && (is_dormant || delta_size > 50 * 1024 * 1024)
    {
        run_compaction(ctx, rx, &delta_file, &post_index_status);
    }

    ctx.idle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .record_change();
    ControlFlow::Continue(())
}

fn handle_safety_decision(
    ctx: &mut DaemonCtx<'_>,
    decision: SafetyDecision,
    directive: &CacheDirective,
) -> ControlFlow<(), SafetyOutcome> {
    let prefix = ctx.log_prefix;
    match decision {
        SafetyDecision::Halt(err, _cooldown) => {
            eprintln!(
                "ixd [{prefix}]: critical safety decision (Halt: {err:?}) \u{2014} pausing operations",
            );
            sync_status(ctx, &DaemonStatus::SafetyHalt, true);
            apply_cache_directive(ctx, directive);
            ControlFlow::Continue(SafetyOutcome::SkipBatch)
        }
        SafetyDecision::Exit(err) => {
            eprintln!("ixd [{prefix}]: SAFETY EXIT (unrecoverable: {err:?}) \u{2014} terminating");
            sync_status(ctx, &DaemonStatus::SafetyExit, true);
            SHUTDOWN.store(true, Ordering::SeqCst);
            if let Some(sock) = ctx.daemon_sock {
                sock.shutdown_notify("safety_exit", 1000);
            }
            ControlFlow::Break(())
        }
        SafetyDecision::Escalate {
            entropy: esc_entropy,
            reason,
            ..
        } => {
            eprintln!(
                "ixd [{prefix}]: safety escalation (entropy: {esc_entropy}, reason: {reason:?}) \u{2014} throttling",
            );
            let escalated_status = DaemonStatus::Escalated {
                entropy: esc_entropy,
            };
            sync_status(ctx, &escalated_status, true);
            apply_cache_directive(ctx, directive);
            ControlFlow::Continue(SafetyOutcome::SkipBatch)
        }
        SafetyDecision::Warn(reason) => {
            if decision.severity() >= 2 {
                eprintln!(
                    "ixd [{prefix}]: safety warning (severity {}): {reason}",
                    decision.severity()
                );
                let warned_status = DaemonStatus::Warned {
                    reason: reason.to_string(),
                };
                sync_status(ctx, &warned_status, true);
            }
            ControlFlow::Continue(SafetyOutcome::Proceed)
        }
        SafetyDecision::Proceed => ControlFlow::Continue(SafetyOutcome::Proceed),
    }
}

fn apply_cache_directive(ctx: &DaemonCtx<'_>, directive: &CacheDirective) {
    ctx.posting_cache.set_admit(directive.allow_new_entries);
    ctx.neg_cache.set_admit(directive.allow_new_entries);
    if directive.evict_fraction > 0.0 {
        ctx.neg_cache.evict_fraction(directive.evict_fraction);
        ctx.posting_cache.evict_fraction(directive.evict_fraction);
    }
}

fn status_for_zone(zone: PressureZone, pressure: u8) -> DaemonStatus {
    match zone {
        PressureZone::Green => DaemonStatus::Idle,
        PressureZone::Yellow => DaemonStatus::Warned {
            reason: format!("memory pressure: {pressure}%"),
        },
        PressureZone::Orange => DaemonStatus::Warned {
            reason: format!("critical memory pressure: {pressure}%"),
        },
        PressureZone::Red => DaemonStatus::SafetyHalt,
    }
}

/// Rebuild the full index from disk plus accumulated deltas under `index_lock`,
/// drain and apply any change batches received mid-build, and restore status.
fn run_compaction(
    ctx: &mut DaemonCtx<'_>,
    rx: &crossbeam_channel::Receiver<Vec<PathBuf>>,
    delta_file: &Path,
    post_status: &DaemonStatus,
) {
    if !delta_file.exists() || delta_file.metadata().map_or(0, |m| m.len()) == 0 {
        return;
    }
    let pressure = ctx.cache_policy.live_pressure();
    if pressure >= 60 {
        tracing::debug!(
            "ixd [{}]: skipping compaction \u{2014} pressure {pressure}% >= 60%",
            ctx.log_prefix
        );
        return;
    }

    sync_status(ctx, &DaemonStatus::Compacting, false);

    let write_guard = ctx
        .index_lock
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let build_res = ctx.builder.build();
    if build_res.is_ok() {
        ctx.posting_cache.invalidate_all();
        ctx.neg_cache.clear();
    }
    drop(write_guard);

    let terminal_status = match build_res {
        Ok(_) => {
            ctx.beacon.last_event_at = current_unix_secs();
            tracing::debug!(
                "ixd [{}]: compaction complete - caches invalidated",
                ctx.log_prefix
            );
            apply_deferred_batches(ctx, rx);
            post_status.clone()
        }
        Err(e) => {
            let prefix = ctx.log_prefix;
            eprintln!("ixd [{prefix}]: compaction failed: {e}");
            DaemonStatus::BuildFailed {
                error: format!("{e}"),
            }
        }
    };

    sync_status(ctx, &terminal_status, terminal_status != DaemonStatus::Idle);
}

fn apply_deferred_batches(ctx: &mut DaemonCtx<'_>, rx: &crossbeam_channel::Receiver<Vec<PathBuf>>) {
    let mut deferred_batches = Vec::new();
    while let Ok(batch) = rx.try_recv() {
        if !batch.is_empty() {
            deferred_batches.push(batch);
        }
    }

    for batch in deferred_batches {
        let write_guard = ctx
            .index_lock
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let update_res = ctx.builder.update(&batch);
        if update_res.is_ok() {
            ctx.posting_cache.invalidate_all();
            ctx.neg_cache.clear();
        }
        drop(write_guard);
        if let Err(e) = update_res {
            tracing::warn!(
                "ixd [{}]: deferred update failed for {} files: {e}",
                ctx.log_prefix,
                batch.len()
            );
        } else {
            ctx.beacon.last_event_at = current_unix_secs();
            if let Err(e) = ctx.beacon.write_to(ctx.ix_dir) {
                eprintln!("ixd [{}]: beacon write error: {e}", ctx.log_prefix);
            }
            broadcast_file_changes(ctx, &batch);
            ctx.idle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .record_change();
        }
    }
}

fn sync_status(ctx: &mut DaemonCtx<'_>, status: &DaemonStatus, broadcast: bool) {
    ctx.beacon.status = status.to_string();
    if let Err(e) = ctx.beacon.write_to(ctx.ix_dir) {
        eprintln!("ixd [{}]: beacon write error: {e}", ctx.log_prefix);
    }
    let files_len = ctx.builder.files_len();
    if let Some(sock) = ctx.daemon_sock {
        sock.set_status(status, files_len);
    }
    if broadcast {
        broadcast_status(ctx, &beacon_status_msg(status, files_len));
    }
}

fn evaluate_safety(guard: &ResourceGuard, pressure: u8, log_prefix: &str) -> (u16, SafetyDecision) {
    match guard.check() {
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
                "ixd [{log_prefix}]: resource check error: {e:?} \u{2014} proceeding with elevated caution",
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

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn beacon_status_msg(status: &DaemonStatus, files: usize) -> ServerMessage {
    ServerMessage::Status {
        pid: std::process::id(),
        status: status.to_string(),
        files,
        daemon_status: Some(status.clone()),
    }
}

fn broadcast_status(ctx: &DaemonCtx<'_>, msg: &ServerMessage) {
    if let Some(sock) = ctx.daemon_sock {
        sock.broadcast(msg);
    }
}

fn broadcast_file_changes(ctx: &DaemonCtx<'_>, changed_files: &[PathBuf]) {
    let Some(sock) = ctx.daemon_sock else { return };
    let now = current_unix_secs();
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

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_signal(_: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn check_concurrent_instance_removes_non_positive_pids() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let ix_dir = root.join(".ix");
        fs::create_dir_all(&ix_dir).expect("mkdir .ix");

        for pid in [0, -1, -42] {
            let mut beacon = Beacon::new(root);
            beacon.pid = pid;
            beacon.write_to(&ix_dir).expect("write beacon");
            let bp = ix_dir.join("beacon.json");
            assert!(
                check_concurrent_instance(&ix_dir, &bp, root, 999).is_ok(),
                "non-positive pid {pid} must be treated as stale, not signaled"
            );
            assert!(!bp.exists(), "invalid beacon with pid {pid} must be pruned");
        }
    }

    #[test]
    fn check_concurrent_instance_rejects_same_process_same_instance_and_accepts_stale_pid() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let ix_dir = root.join(".ix");
        fs::create_dir_all(&ix_dir).expect("mkdir .ix");
        let bp = ix_dir.join("beacon.json");

        // Same PID + same instance_id must fail loudly.
        let mut live_same = Beacon::with_instance_id(root, 4242);
        live_same.pid = i32::try_from(std::process::id()).unwrap_or(1);
        live_same.write_to(&ix_dir).expect("write");
        assert!(check_concurrent_instance(&ix_dir, &bp, root, 4242).is_err());

        // Same PID + different instance_id -> stale from prior run.
        assert!(check_concurrent_instance(&ix_dir, &bp, root, 7777).is_ok());
        assert!(!bp.exists());

        // Very high nonexistent PID -> ESRCH -> stale.
        let mut dead = Beacon::with_instance_id(root, 1);
        dead.pid = 2_000_000_000;
        dead.write_to(&ix_dir).expect("write");
        assert!(check_concurrent_instance(&ix_dir, &bp, root, 7777).is_ok());
        assert!(!bp.exists());
    }

    #[test]
    fn acquire_instance_lock_prevents_concurrent_lock_holders() {
        let dir = tempdir().expect("tempdir");
        let ix_dir = dir.path().join(".ix");
        let first = acquire_instance_lock(&ix_dir, dir.path()).expect("first flock succeeds");
        let second = acquire_instance_lock(&ix_dir, dir.path());
        assert!(second.is_err(), "second non-blocking flock must fail");
        drop(first);
        let third = acquire_instance_lock(&ix_dir, dir.path());
        assert!(third.is_ok(), "flock must be released once guard drops");
    }

    #[test]
    fn run_many_propagates_startup_error_when_root_locked() {
        let dir = tempdir().expect("tempdir");
        let canon = dir.path().canonicalize().expect("canon");
        let ix_dir = canon.join(".ix");
        let _held = acquire_instance_lock(&ix_dir, &canon).expect("hold lock");
        let err = run_many(&[canon]).expect_err("run_many must propagate startup lock error");
        assert!(err.to_string().contains("lock"));
    }

    #[test]
    fn safety_exit_decision_sets_shutdown_and_breaks_loop() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let ix_dir = root.join(".ix");
        fs::create_dir_all(&ix_dir).expect("mkdir");
        let mut builder = Builder::new(root).expect("builder");
        let mut beacon = Beacon::new(root);
        let guard = ResourceGuard::for_testing(1024 * 1024, 0, 10);
        let cache_policy = AdaptiveCachePolicy::new_with_guard(guard.clone(), 1024 * 1024);
        let posting_cache = Arc::new(PostingCache::new(1024));
        let neg_cache = Arc::new(NegCache::new(16));
        let index_lock = Arc::new(RwLock::new(()));
        let idle = Arc::new(Mutex::new(IdleTracker::new()));

        SHUTDOWN.store(false, Ordering::SeqCst);
        let mut ctx = DaemonCtx {
            builder: &mut builder,
            ix_dir: &ix_dir,
            beacon: &mut beacon,
            idle,
            guard: &guard,
            cache_policy: &cache_policy,
            posting_cache: &posting_cache,
            neg_cache: &neg_cache,
            daemon_sock: None,
            index_lock: &index_lock,
            log_prefix: "test",
        };
        let directive = ctx.cache_policy.directive_for_pressure(10);
        let flow = handle_safety_decision(
            &mut ctx,
            SafetyDecision::Exit(llmosafe::KernelError::ResourceExhaustion),
            &directive,
        );
        assert!(
            flow.is_break(),
            "SafetyDecision::Exit must break event loop"
        );
        assert!(SHUTDOWN.load(Ordering::SeqCst));
        SHUTDOWN.store(false, Ordering::SeqCst);
    }

    #[test]
    fn deferred_batches_invalidate_caches_and_update_idle() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let ix_dir = root.join(".ix");
        let file_a = root.join("a.txt");
        fs::write(&file_a, "initial text\n").expect("write");

        let mut builder = Builder::new(root).expect("builder");
        builder.build().expect("build");

        let mut beacon = Beacon::new(root);
        let guard = ResourceGuard::for_testing(1024 * 1024, 0, 10);
        let cache_policy = AdaptiveCachePolicy::new_with_guard(guard.clone(), 1024 * 1024);
        let posting_cache = Arc::new(PostingCache::new(64 * 1024));
        let neg_cache = Arc::new(NegCache::new(64));
        neg_cache.record_negative(123, 456);
        let index_lock = Arc::new(RwLock::new(()));
        let idle = Arc::new(Mutex::new(IdleTracker::new()));

        let (tx, rx) = crossbeam_channel::unbounded();
        fs::write(&file_a, "updated text\n").expect("rewrite");
        tx.send(vec![file_a]).expect("send batch");

        let mut ctx = DaemonCtx {
            builder: &mut builder,
            ix_dir: &ix_dir,
            beacon: &mut beacon,
            idle: Arc::clone(&idle),
            guard: &guard,
            cache_policy: &cache_policy,
            posting_cache: &posting_cache,
            neg_cache: &neg_cache,
            daemon_sock: None,
            index_lock: &index_lock,
            log_prefix: "test",
        };
        apply_deferred_batches(&mut ctx, &rx);
        assert!(
            neg_cache.is_empty(),
            "deferred batch update must clear neg_cache"
        );
    }

    #[derive(Clone, Copy)]
    enum MutationSite {
        HandleChanges,
        RunCompaction,
        ApplyDeferredBatches,
    }

    fn populate_test_caches(posting_cache: &PostingCache, neg_cache: &NegCache) {
        use crate::posting::{PostingEntry, PostingList};
        for b in *b"abcd" {
            posting_cache.insert(
                crate::trigram::from_bytes(b, b'e', b't'),
                PostingList {
                    entries: vec![PostingEntry {
                        file_id: 9999,
                        offsets: vec![0],
                    }],
                },
            );
        }
        let fp = crate::neg_cache::query_fingerprint_with_options(
            "beta_updated_token",
            false,
            false,
            false,
            false,
        );
        for h in 0..10u64 {
            neg_cache.record_negative(fp, 1000 + h);
        }
    }

    fn count_beta_matches(
        root: &Path,
        posting_cache: &Arc<PostingCache>,
        neg_cache: &Arc<NegCache>,
    ) -> usize {
        let reader = crate::reader::Reader::open(&root.join(".ix").join("shard.ix")).expect("open");
        let regex_pool = Arc::new(RegexPool::new(16));
        let mut executor = crate::executor::Executor::new_with_caches(
            &reader,
            Arc::clone(posting_cache),
            Arc::clone(neg_cache),
            regex_pool,
            Some(root.join(".ix").join("shard.ix.delta")),
        );
        let plan = crate::planner::Planner::plan_with_options(
            "beta_updated_token",
            crate::planner::QueryOptions::default(),
        )
        .expect("plan");
        let (matches, _) = executor
            .execute(&plan, &crate::executor::QueryOptions::default())
            .expect("execute");
        matches.len()
    }

    fn run_mutation_site(
        site: MutationSite,
        ctx: &mut DaemonCtx<'_>,
        file_for_writer: PathBuf,
        delta_file: &Path,
        rx: &crossbeam_channel::Receiver<Vec<PathBuf>>,
    ) {
        match site {
            MutationSite::HandleChanges => {
                let _ = handle_changes(ctx, &[file_for_writer], rx);
            }
            MutationSite::RunCompaction => {
                run_compaction(ctx, rx, delta_file, &DaemonStatus::Idle);
            }
            MutationSite::ApplyDeferredBatches => {
                apply_deferred_batches(ctx, rx);
            }
        }
    }

    struct LockBarrierState<'a> {
        index_lock: &'a Arc<RwLock<()>>,
        posting_cache: &'a Arc<PostingCache>,
        neg_cache: &'a Arc<NegCache>,
        n_entered_rx: mpsc::Receiver<()>,
        reader_ready_rx: mpsc::Receiver<()>,
        reader_done_rx: mpsc::Receiver<(bool, usize)>,
    }

    fn verify_neg_clear_and_reader_unblock(
        state: &LockBarrierState<'_>,
        p_release: crate::posting_cache::ClearPauseRelease,
        n_release: crate::posting_cache::ClearPauseRelease,
    ) {
        let hook_timeout = std::time::Duration::from_secs(5);
        state
            .reader_ready_rx
            .recv_timeout(hook_timeout)
            .expect("reader ready");
        p_release.release();

        state
            .n_entered_rx
            .recv_timeout(hook_timeout)
            .expect("writer entered neg_cache.clear");
        assert!(state.posting_cache.is_empty());
        assert!(!state.neg_cache.is_empty());
        assert!(
            matches!(
                state.index_lock.try_read(),
                Err(std::sync::TryLockError::WouldBlock)
            ),
            "index_lock.write() must remain held while neg_cache.clear() runs"
        );
        assert!(
            matches!(
                state.reader_done_rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ),
            "blocked reader must not acquire read lock before neg_cache.clear() completes"
        );

        n_release.release();
        let (caches_cleared, hits) = state
            .reader_done_rx
            .recv_timeout(hook_timeout)
            .expect("reader unblocked");
        assert!(
            caches_cleared,
            "reader acquiring index_lock.read() must see cleared posting_cache and neg_cache"
        );
        assert!(
            hits >= 1,
            "reader after cache clear must find post-mutation match"
        );
    }

    fn assert_mutation_holds_write_lock_across_cache_clears(site: MutationSite) {
        let hook_timeout = std::time::Duration::from_secs(5);
        let dir = tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let ix_dir = root.join(".ix");
        let file_a = root.join("a.txt");
        fs::write(&file_a, "alpha_initial_token\n").expect("write initial");

        let mut builder = Builder::new(&root).expect("builder");
        builder.build().expect("initial build");

        if matches!(site, MutationSite::RunCompaction) {
            fs::write(&file_a, "alpha_delta_token\n").expect("write delta");
            builder
                .update(std::slice::from_ref(&file_a))
                .expect("seed delta");
        }

        fs::write(&file_a, "beta_updated_token\n").expect("write updated");

        let mut beacon = Beacon::new(&root);
        let guard = ResourceGuard::for_testing(usize::MAX, 0, 0);
        let cache_policy = AdaptiveCachePolicy::new_with_guard(guard.clone(), usize::MAX);
        let posting_cache = Arc::new(PostingCache::new(64 * 1024));
        let neg_cache = Arc::new(NegCache::new(64));
        populate_test_caches(&posting_cache, &neg_cache);
        let index_lock = Arc::new(RwLock::new(()));
        let idle = Arc::new(Mutex::new(IdleTracker::new()));

        let (p_hook, p_entered_rx, p_release) = crate::posting_cache::ClearPauseHook::channel();
        let (n_hook, n_entered_rx, n_release) = crate::posting_cache::ClearPauseHook::channel();
        posting_cache.set_clear_pause_hook(Some(p_hook));
        neg_cache.set_clear_pause_hook(Some(n_hook));

        let (tx, rx) = crossbeam_channel::unbounded();
        if matches!(site, MutationSite::ApplyDeferredBatches) {
            tx.send(vec![file_a.clone()]).expect("send batch");
        }

        let (reader_ready_tx, reader_ready_rx) = mpsc::channel();
        let (reader_done_tx, reader_done_rx) = mpsc::channel();

        std::thread::scope(|scope| {
            // Keep RAII release guards inside scope so any assertion panic drops them
            // before std::thread::scope joins the writer thread.
            let p_release = p_release;
            let n_release = n_release;
            let delta_file = ix_dir.join("shard.ix.delta");
            let file_for_writer = file_a.clone();
            let idle_for_writer = Arc::clone(&idle);
            let posting_for_writer = Arc::clone(&posting_cache);
            let neg_for_writer = Arc::clone(&neg_cache);
            let lock_for_writer = Arc::clone(&index_lock);

            scope.spawn(move || {
                let mut ctx = DaemonCtx {
                    builder: &mut builder,
                    ix_dir: &ix_dir,
                    beacon: &mut beacon,
                    idle: idle_for_writer,
                    guard: &guard,
                    cache_policy: &cache_policy,
                    posting_cache: &posting_for_writer,
                    neg_cache: &neg_for_writer,
                    daemon_sock: None,
                    index_lock: &lock_for_writer,
                    log_prefix: "test",
                };
                run_mutation_site(site, &mut ctx, file_for_writer, &delta_file, &rx);
            });

            p_entered_rx
                .recv_timeout(hook_timeout)
                .expect("writer entered posting_cache.invalidate_all");
            assert!(!posting_cache.is_empty());
            assert!(!neg_cache.is_empty());
            assert!(
                matches!(
                    index_lock.try_read(),
                    Err(std::sync::TryLockError::WouldBlock)
                ),
                "index_lock.write() must remain held while posting_cache.invalidate_all() runs"
            );

            let lock_for_reader = Arc::clone(&index_lock);
            let posting_for_reader = Arc::clone(&posting_cache);
            let neg_for_reader = Arc::clone(&neg_cache);
            let root_for_reader = root.clone();
            scope.spawn(move || {
                reader_ready_tx.send(()).expect("reader ready");
                let _read_guard = lock_for_reader
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let caches_cleared = posting_for_reader.is_empty() && neg_for_reader.is_empty();
                let hits =
                    count_beta_matches(&root_for_reader, &posting_for_reader, &neg_for_reader);
                reader_done_tx
                    .send((caches_cleared, hits))
                    .expect("send reader outcome");
            });

            let barrier_state = LockBarrierState {
                index_lock: &index_lock,
                posting_cache: &posting_cache,
                neg_cache: &neg_cache,
                n_entered_rx,
                reader_ready_rx,
                reader_done_rx,
            };
            verify_neg_clear_and_reader_unblock(&barrier_state, p_release, n_release);
        });
    }

    #[test]
    fn write_lock_held_across_cache_clears_in_handle_changes() {
        assert_mutation_holds_write_lock_across_cache_clears(MutationSite::HandleChanges);
    }

    #[test]
    fn write_lock_held_across_cache_clears_in_run_compaction() {
        assert_mutation_holds_write_lock_across_cache_clears(MutationSite::RunCompaction);
    }

    #[test]
    fn write_lock_held_across_cache_clears_in_apply_deferred_batches() {
        assert_mutation_holds_write_lock_across_cache_clears(MutationSite::ApplyDeferredBatches);
    }

    #[test]
    fn handle_safety_decision_evicts_caches_before_skip_batch_on_halt_and_escalate() {
        use llmosafe::KernelError;
        use llmosafe::llmosafe_integration::EscalationReason;

        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let ix_dir = root.join(".ix");
        fs::create_dir_all(&ix_dir).expect("mkdir");
        let mut builder = Builder::new(root).expect("builder");
        let mut beacon = Beacon::new(root);
        let guard = ResourceGuard::for_testing(1024 * 1024, 0, 80);
        let cache_policy = AdaptiveCachePolicy::new_with_guard(guard.clone(), 1024 * 1024);
        let posting_cache = Arc::new(PostingCache::new(64 * 1024));
        let neg_cache = Arc::new(NegCache::new(64));
        let index_lock = Arc::new(RwLock::new(()));
        let idle = Arc::new(Mutex::new(IdleTracker::new()));

        populate_test_caches(&posting_cache, &neg_cache);
        assert_eq!(posting_cache.len(), 4);
        assert_eq!(neg_cache.len(), 10);

        let mut ctx = DaemonCtx {
            builder: &mut builder,
            ix_dir: &ix_dir,
            beacon: &mut beacon,
            idle,
            guard: &guard,
            cache_policy: &cache_policy,
            posting_cache: &posting_cache,
            neg_cache: &neg_cache,
            daemon_sock: None,
            index_lock: &index_lock,
            log_prefix: "test",
        };

        let orange_directive = ctx.cache_policy.directive_for_pressure(80);
        let halt_flow = handle_safety_decision(
            &mut ctx,
            SafetyDecision::Halt(KernelError::CognitiveInstability, 1000),
            &orange_directive,
        );
        assert!(matches!(
            halt_flow,
            ControlFlow::Continue(SafetyOutcome::SkipBatch)
        ));
        assert!(posting_cache.stats().evictions > 0);
        assert!(neg_cache.stats().evictions > 0);
        assert!(posting_cache.len() < 4);
        assert!(neg_cache.len() < 10);
        assert_eq!(ctx.beacon.status, DaemonStatus::SafetyHalt.to_string());
        // Admission disabled under Orange directive.
        let prev_neg_len = neg_cache.len();
        neg_cache.record_negative(777, 888);
        assert_eq!(neg_cache.len(), prev_neg_len);

        // Re-enable admission, repopulate caches, and verify Escalate + Red evicts everything before SkipBatch.
        posting_cache.set_admit(true);
        neg_cache.set_admit(true);
        populate_test_caches(&posting_cache, &neg_cache);
        let red_directive = ctx.cache_policy.directive_for_pressure(95);
        let escalate_flow = handle_safety_decision(
            &mut ctx,
            SafetyDecision::Escalate {
                entropy: 500,
                reason: EscalationReason::ResourcePressure,
                cooldown_ms: 5000,
            },
            &red_directive,
        );
        assert!(matches!(
            escalate_flow,
            ControlFlow::Continue(SafetyOutcome::SkipBatch)
        ));
        assert!(posting_cache.is_empty());
        assert!(neg_cache.is_empty());
        assert_eq!(
            ctx.beacon.status,
            DaemonStatus::Escalated { entropy: 500 }.to_string()
        );
        neg_cache.record_negative(777, 889);
        assert!(neg_cache.is_empty());
    }

    #[test]
    fn handle_changes_recovers_on_vm_rss_divergence_and_evicts_on_live_pressure() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let ix_dir = root.join(".ix");
        let file_a = root.join("a.txt");
        fs::write(&file_a, "alpha_initial_token\n").expect("write initial");

        let mut builder = Builder::new(root).expect("builder");
        builder.build().expect("initial build");

        let mut beacon = Beacon::new(root);
        let posting_cache = Arc::new(PostingCache::new(64 * 1024));
        let neg_cache = Arc::new(NegCache::new(64));
        let index_lock = Arc::new(RwLock::new(()));
        let idle = Arc::new(Mutex::new(IdleTracker::new()));
        let (_tx, rx) = crossbeam_channel::unbounded();

        // Phase A (VmRSS divergence recovery): guard.pressure() == 100 (simulated ru_maxrss >= 100%),
        // but live_pressure() against usize::MAX ceiling recovers to Green so handle_changes updates index.
        let guard_stuck = ResourceGuard::for_testing(usize::MAX, 0, 100);
        assert_eq!(guard_stuck.pressure(), 100);
        let policy_recovered = AdaptiveCachePolicy::new_with_guard(guard_stuck.clone(), usize::MAX);
        fs::write(&file_a, "beta_updated_token\n").expect("write recovered");

        {
            let mut ctx = DaemonCtx {
                builder: &mut builder,
                ix_dir: &ix_dir,
                beacon: &mut beacon,
                idle: Arc::clone(&idle),
                guard: &guard_stuck,
                cache_policy: &policy_recovered,
                posting_cache: &posting_cache,
                neg_cache: &neg_cache,
                daemon_sock: None,
                index_lock: &index_lock,
                log_prefix: "test",
            };
            let flow = handle_changes(&mut ctx, std::slice::from_ref(&file_a), &rx);
            assert!(flow.is_continue());
        }
        assert!(
            count_beta_matches(root, &posting_cache, &neg_cache) >= 1,
            "handle_changes must apply update when live VmRSS recovers despite guard.pressure() == 100"
        );

        // Phase B (Live pressure eviction before skip): live VmRSS exceeds 1 KiB ceiling even though
        // guard.pressure() == 0; handle_changes must evict caches and skip indexing the file batch.
        #[cfg(target_os = "linux")]
        {
            populate_test_caches(&posting_cache, &neg_cache);
            let guard_low = ResourceGuard::for_testing(1024, 0, 0);
            let policy_saturated = AdaptiveCachePolicy::new_with_guard(guard_low.clone(), 1024);
            fs::write(&file_a, "gamma_skipped_token\n").expect("write skipped");

            let mut ctx = DaemonCtx {
                builder: &mut builder,
                ix_dir: &ix_dir,
                beacon: &mut beacon,
                idle,
                guard: &guard_low,
                cache_policy: &policy_saturated,
                posting_cache: &posting_cache,
                neg_cache: &neg_cache,
                daemon_sock: None,
                index_lock: &index_lock,
                log_prefix: "test",
            };
            let flow = handle_changes(&mut ctx, std::slice::from_ref(&file_a), &rx);
            assert!(flow.is_continue());
            assert!(
                posting_cache.is_empty() && neg_cache.is_empty(),
                "handle_changes must evict caches before skipping batch under live pressure"
            );
            assert_eq!(
                ctx.beacon.status,
                DaemonStatus::Escalated { entropy: 0 }.to_string()
            );

            let reader = crate::reader::Reader::open(&ix_dir.join("shard.ix")).expect("open");
            let mut executor = crate::executor::Executor::new(&reader);
            executor.set_delta_path(ix_dir.join("shard.ix.delta"));
            let plan = crate::planner::Planner::plan_with_options(
                "gamma_skipped_token",
                crate::planner::QueryOptions::default(),
            )
            .expect("plan");
            let (skipped_matches, _) = executor
                .execute(&plan, &crate::executor::QueryOptions::default())
                .expect("execute");
            assert!(
                skipped_matches.is_empty(),
                "skipped batch under live pressure must not update index"
            );
        }
    }
}
