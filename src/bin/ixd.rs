//! ix background daemon — ixd.
//!
//! Thin wrapper around [`ix::daemon::run_many`]. The full guarded daemon logic
//! (RAM-bounded `ResourceGuard`, entropy monitoring, Unix-domain socket,
//! file watching, beacon arbitration) lives in the library.
//!
//! ## Multi-root support (v0.9+)
//!
//! Pass multiple paths to watch several projects in one daemon process:
//!
//! ```text
//! ixd /home/user/project-a /home/user/project-b
//! ```
//!
//! Each root runs on its own thread with independent index, watcher, beacon,
//! and Unix domain socket. Signal handling and resource monitoring are shared.
//!
//! ## Daemon mode (`--daemon`)
//!
//! `ixd --daemon /path` detaches from the controlling terminal via the
//! standard Unix double-fork + `setsid` sequence and runs in the background,
//! so you no longer need an external `nohup ... & disown` wrapper. The
//! caller returns immediately (exit 0) once the daemon has launched; use
//! `ix service status` to confirm it is live. The default (no flag) keeps
//! the daemon in the foreground, which is what you want for debugging or
//! when it is managed by systemd.
//!
//! Safety guarantees:
//! - `SIGTERM`/`SIGINT` → clean shutdown (beacon removed, watcher joined, no zombies)
//! - `Builder::new` returns `Result` — no panics on unwritable `.ix` dir
//! - `ResourceGuard` at 60% RAM — OOM protection with 80% proportional RSS fallback
//! - TOCTOU guards in `process_file` — skips vanished/permission-denied files

#![warn(clippy::pedantic)]

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "ixd",
    version = env!("CARGO_PKG_VERSION"),
    about = "Background daemon that watches directories for changes and rebuilds the index.",
    after_help = "EXAMPLES:\n  \
                  ixd /path/to/repo\n  \
                  ixd --daemon /path/to/repo   # detach and run in the background\n  \
                  ixd /project-a /project-b /project-c\n\n\
                  DOCS:\n  \
                  https://github.com/moeshawky/ix/blob/main/docs/DAEMON-RUNBOOK.md\n  \
                  https://github.com/moeshawky/ix/blob/main/docs/.ixd.toml.md"
)]
struct Cli {
    /// One or more directories to watch (defaults to current directory if omitted).
    #[arg(default_value = ".", value_name = "PATH")]
    paths: Vec<PathBuf>,

    /// Detach from the controlling terminal and run in the background.
    ///
    /// Performs the Unix double-fork + `setsid` sequence: the calling process
    /// returns immediately (exit 0) once the daemon has launched, and the
    /// daemon continues detached with stdio redirected to `/dev/null`. Use
    /// `ix service status` to verify the daemon is live. Without this flag,
    /// ixd stays in the foreground (useful for debugging or systemd units).
    #[arg(long)]
    daemon: bool,

    /// Stop the running ixd daemon for the specified directories.
    #[arg(long)]
    stop: bool,

    /// Write daemon logs to this file path (defaults to an XDG state/runtime
    /// log file when `--daemon` is active so logs survive stdio redirection).
    #[arg(long, value_name = "PATH")]
    log_file: Option<PathBuf>,
}

#[cfg(unix)]
fn default_log_path(roots: &[PathBuf]) -> PathBuf {
    let primary = roots.first().map_or_else(
        || PathBuf::from("."),
        |p| p.canonicalize().unwrap_or_else(|_| p.clone()),
    );
    let hash = xxhash_rust::xxh64::xxh64(primary.to_string_lossy().as_bytes(), 0);
    if let Ok(state_home) = std::env::var("XDG_STATE_HOME")
        && !state_home.is_empty()
    {
        return PathBuf::from(state_home)
            .join("ixd")
            .join(format!("{hash:016x}.log"));
    }
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR")
        && !runtime_dir.is_empty()
    {
        return PathBuf::from(runtime_dir)
            .join("ixd")
            .join(format!("{hash:016x}.log"));
    }
    // SAFETY: libc::getuid() is a thread-safe POSIX syscall with no preconditions.
    let uid = unsafe { libc::getuid() };
    std::env::temp_dir().join(format!("ixd-{uid}-{hash:016x}.log"))
}

#[cfg(unix)]
fn open_log_file(path: &std::path::Path) -> ix::error::Result<std::fs::File> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    Ok(file)
}

#[cfg(unix)]
fn init_tracing_file(file: std::fs::File) {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .try_init();
}

#[cfg(unix)]
fn init_tracing_stderr() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(unix)]
fn main() -> ix::error::Result<()> {
    let cli = Cli::parse();

    // Resolve relative roots against the *current* working directory before
    // daemonizing — `daemonize()` chdirs to `/`, which would otherwise make a
    // relative path like `.` resolve to the filesystem root. `run_many`
    // canonicalizes again, so handing it absolute paths is idempotent.
    let roots = resolve_roots(&cli.paths)?;

    if cli.stop {
        init_tracing_stderr();
        return stop_daemons(&roots);
    }

    if cli.daemon {
        let log_path = cli.log_file.unwrap_or_else(|| default_log_path(&roots));
        return run_daemonized(&roots, &log_path);
    }

    if let Some(ref log_path) = cli.log_file {
        let log_file = open_log_file(log_path)?;
        init_tracing_file(log_file);
    } else {
        init_tracing_stderr();
    }
    ix::daemon::run_many(&roots)
}

#[cfg(not(unix))]
fn main() {
    eprintln!("ixd: the daemon is not supported on this platform");
    std::process::exit(1);
}

/// Make each root path absolute against the current working directory.
///
/// This preserves the caller's `cwd` intent before [`daemonize`] chdirs to `/`.
/// Existing absolute paths are returned unchanged. We deliberately do *not*
/// canonicalize here (which would require the path to exist) — [`ix::daemon::run_many`]
/// does that itself and tolerates missing roots with a warning.
#[cfg(unix)]
fn resolve_roots(paths: &[PathBuf]) -> ix::error::Result<Vec<PathBuf>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let cwd = std::env::current_dir().map_err(|e| {
        ix::error::Error::Config(format!("ixd: cannot determine current directory: {e}"))
    })?;
    paths
        .iter()
        .map(|p| {
            if p.is_absolute() {
                Ok(p.clone())
            } else {
                Ok(cwd.join(p))
            }
        })
        .collect()
}

/// Daemonize and run [`ix::daemon::run_many`], synchronizing startup readiness
/// back to the launching parent process over a Unix socket pair.
#[cfg(unix)]
fn run_daemonized(roots: &[PathBuf], log_path: &std::path::Path) -> ix::error::Result<()> {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    // Open the persistent log file BEFORE daemonize() redirects fds 0..2 to /dev/null.
    let log_file = open_log_file(log_path)?;
    let child_sock = daemonize(log_path)?;
    init_tracing_file(log_file);
    tracing::info!("ixd: daemonized, logging to {}", log_path.display());
    let shared_sock = Arc::new(Mutex::new(Some(child_sock)));
    let watcher_sock = Arc::clone(&shared_sock);

    let canonical_roots: Vec<PathBuf> = roots
        .iter()
        .map(|r| r.canonicalize().unwrap_or_else(|_| r.clone()))
        .collect();
    let my_pid = i32::try_from(std::process::id()).unwrap_or(0);

    let _ = std::thread::Builder::new()
        .name("ixd-startup-wait".into())
        .spawn(move || {
            notify_when_beacons_live(&canonical_roots, my_pid, &watcher_sock);
        });

    let res = ix::daemon::run_many(roots);

    if let Ok(mut guard) = shared_sock.lock()
        && let Some(mut sock) = guard.take()
    {
        let msg = match &res {
            Ok(()) => "ixd: daemon exited before completing startup".to_string(),
            Err(e) => format!("ixd: {e}"),
        };
        let _ = sock.write_all(&[1]);
        let _ = sock.write_all(msg.as_bytes());
    }

    res
}

#[cfg(unix)]
fn notify_when_beacons_live(
    canonical_roots: &[PathBuf],
    my_pid: i32,
    watcher_sock: &std::sync::Arc<std::sync::Mutex<Option<std::os::unix::net::UnixStream>>>,
) {
    use std::io::Write;
    use std::thread::sleep;
    use std::time::Duration;

    if my_pid <= 0 || canonical_roots.is_empty() {
        return;
    }

    loop {
        let already_closed = watcher_sock.lock().map_or(true, |g| g.is_none());
        if already_closed {
            return;
        }

        let all_ready = canonical_roots.iter().all(|root| {
            let ix_dir = root.join(".ix");
            ix::format::Beacon::read_from(&ix_dir).is_ok_and(|b| b.pid == my_pid && b.is_live())
        });

        if all_ready {
            if let Ok(mut guard) = watcher_sock.lock()
                && let Some(mut sock) = guard.take()
            {
                let _ = sock.write_all(&[0]);
            }
            return;
        }

        sleep(Duration::from_millis(50));
    }
}

/// Detach ixd from the controlling terminal and run it in the background.
///
/// Uses the classic Unix daemonization sequence (double-fork + `setsid`) paired
/// with a status socket so the launching parent waits until startup is
/// confirmed before exiting.
///
/// # Errors
///
/// Returns an error if any of the fork, session, socket, or `/dev/null` steps fail.
#[cfg(unix)]
fn daemonize(log_path: &std::path::Path) -> ix::error::Result<std::os::unix::net::UnixStream> {
    use nix::unistd::{ForkResult, fork, setsid};
    use std::fs::OpenOptions;
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let cfg = |ctx: &str, e: nix::errno::Errno| {
        ix::error::Error::Config(format!("daemonize: {ctx}: {e}"))
    };

    let (mut parent_sock, child_sock) = UnixStream::pair()?;

    // 1. First fork: the parent waits on `parent_sock` for startup readiness.
    // SAFETY: No background threads have been spawned yet (the process is
    // single-threaded prior to `run_many`), and the parent immediately drops
    // the child socket end and blocks on `parent_sock` before exiting.
    match unsafe { fork() }.map_err(|e| cfg("fork", e))? {
        ForkResult::Parent { .. } => {
            drop(child_sock);
            let _ = parent_sock.set_read_timeout(Some(Duration::from_secs(60)));
            let mut buf = Vec::new();
            match parent_sock.read_to_end(&mut buf) {
                Ok(_) if buf.first() == Some(&0) => std::process::exit(0),
                Ok(_) if buf.first() == Some(&1) => {
                    let msg = String::from_utf8_lossy(buf.get(1..).unwrap_or(&[]));
                    if msg.is_empty() {
                        eprintln!(
                            "ixd: daemon failed during startup (see log: {})",
                            log_path.display()
                        );
                    } else {
                        eprintln!("{msg} (see log: {})", log_path.display());
                    }
                    std::process::exit(1);
                }
                Ok(_) => {
                    eprintln!(
                        "ixd: daemon exited unexpectedly during startup (see log: {})",
                        log_path.display()
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!(
                        "ixd: timed out or failed waiting for daemon startup: {e} (see log: {})",
                        log_path.display()
                    );
                    std::process::exit(1);
                }
            }
        }
        ForkResult::Child => drop(parent_sock),
    }

    // 2. New session + process group, no controlling terminal.
    setsid().map_err(|e| cfg("setsid", e))?;

    // 3. Second fork: the daemon can never reacquire a terminal.
    // SAFETY: Still single-threaded prior to `run_many`; the intermediate
    // parent exits immediately without accessing shared state.
    match unsafe { fork() }.map_err(|e| cfg("fork", e))? {
        ForkResult::Parent { .. } => std::process::exit(0),
        ForkResult::Child => {}
    }

    // 4. Shed the inherited working directory and redirect stdio to /dev/null
    //    so logs/prints do not reach the launching terminal.
    let _ = std::env::set_current_dir("/");
    let devnull = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
        .map_err(|e| ix::error::Error::Config(format!("daemonize: cannot open /dev/null: {e}")))?;

    let raw = devnull.as_raw_fd();
    for fd in [0_i32, 1, 2] {
        // SAFETY: libc::dup2(oldfd, newfd) duplicates the /dev/null fd onto the
        // standard descriptors 0/1/2 — the canonical daemonize step. It is a
        // thin syscall wrapper with no preconditions beyond valid fds.
        let rc = unsafe { libc::dup2(raw, fd) };
        if rc < 0 {
            return Err(ix::error::Error::Config(format!(
                "daemonize: cannot redirect fd {fd} to /dev/null"
            )));
        }
    }
    if raw > 2 {
        drop(devnull);
    } else {
        std::mem::forget(devnull);
    }

    Ok(child_sock)
}

/// Stop running ixd daemons managing the specified roots.
#[cfg(unix)]
fn stop_daemons(roots: &[PathBuf]) -> ix::error::Result<()> {
    use nix::unistd::Pid;
    use std::collections::HashSet;

    let mut stopped_pids = HashSet::new();
    let mut stopped_any = false;
    let mut errors = Vec::new();

    for root in roots {
        let Some(beacon) = find_beacon_for_root(root) else {
            println!("ixd: no running daemon found for {}", root.display());
            continue;
        };

        if beacon.pid <= 0 || !beacon.is_live() {
            println!(
                "ixd: no running daemon found for {} (stale beacon from PID {})",
                root.display(),
                beacon.pid
            );
            continue;
        }

        let pid = beacon.pid;
        if stopped_pids.insert(pid) {
            println!("ixd: stopping daemon (PID {pid}) for {}...", root.display());
            match stop_single_pid(Pid::from_raw(pid), pid) {
                Ok(()) => stopped_any = true,
                Err(msg) => errors.push(msg),
            }
        } else {
            stopped_any = true;
        }
    }

    if !errors.is_empty() {
        return Err(ix::error::Error::Config(errors.join("; ")));
    }
    if !stopped_any {
        return Err(ix::error::Error::Config(
            "no running ixd daemon found".into(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn find_beacon_for_root(root: &std::path::Path) -> Option<ix::format::Beacon> {
    let ix_dir = root.join(".ix");
    if let Ok(b) = ix::format::Beacon::read_from(&ix_dir) {
        return Some(b);
    }
    let mut current = root.to_path_buf();
    while current.pop() {
        let candidate = current.join(".ix");
        if let Ok(b) = ix::format::Beacon::read_from(&candidate) {
            return Some(b);
        }
    }
    None
}

#[cfg(unix)]
fn stop_single_pid(nix_pid: nix::unistd::Pid, pid: i32) -> Result<(), String> {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill};
    use std::thread::sleep;
    use std::time::Duration;

    if let Err(e) = kill(nix_pid, Some(Signal::SIGTERM)) {
        return Err(format!("failed to signal PID {pid}: {e}"));
    }

    for _ in 0..50 {
        sleep(Duration::from_millis(100));
        if kill(nix_pid, None).is_err() {
            println!("ixd: daemon (PID {pid}) stopped.");
            return Ok(());
        }
    }

    eprintln!("ixd: warning: daemon (PID {pid}) did not exit within 5s; sending SIGKILL...");
    match kill(nix_pid, Some(Signal::SIGKILL)) {
        Ok(()) => {
            println!("ixd: daemon (PID {pid}) killed.");
            Ok(())
        }
        Err(Errno::ESRCH) => {
            println!("ixd: daemon (PID {pid}) stopped.");
            Ok(())
        }
        Err(e) => Err(format!("failed to SIGKILL PID {pid}: {e}")),
    }
}
