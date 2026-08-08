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
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::struct_field_names)]

use clap::Parser;
use std::os::fd::AsRawFd;
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
}

#[cfg(unix)]
fn main() -> ix::error::Result<()> {
    let cli = Cli::parse();

    // Resolve relative roots against the *current* working directory before
    // daemonizing — `daemonize()` chdirs to `/`, which would otherwise make a
    // relative path like `.` resolve to the filesystem root. `run_many`
    // canonicalizes again, so handing it absolute paths is idempotent.
    let roots = resolve_roots(&cli.paths)?;

    if cli.daemon {
        daemonize()?;
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

/// Detach ixd from the controlling terminal and run it in the background.
///
/// Uses the classic Unix daemonization sequence (double-fork + `setsid`):
///
/// 1. **First fork** — the parent exits immediately (exit 0) so the caller's
///    shell returns; the child is no longer a process-group leader's relative.
/// 2. **`setsid`** — the child becomes the leader of a new session and a new
///    process group, with no controlling terminal.
/// 3. **Second fork** — guarantees the final daemon can never reacquire a
///    controlling terminal (a session leader can; a non-leader cannot).
/// 4. **Stdio redirect** — stdin/stdout/stderr are pointed at `/dev/null`,
///    and the working directory moves to `/` so the daemon never holds a
///    mounted filesystem busy.
///
/// This must run *before* [`ix::daemon::run_many`] installs its signal
/// handlers and spawns watcher threads, so the daemon owns its signals and
/// threads cleanly.
///
/// # Errors
///
/// Returns an error if any of the fork, session, or `/dev/null` steps fail.
#[cfg(unix)]
fn daemonize() -> ix::error::Result<()> {
    use nix::unistd::{ForkResult, fork, setsid};
    use std::fs::OpenOptions;

    let cfg = |ctx: &str, e: nix::errno::Errno| {
        ix::error::Error::Config(format!("daemonize: {ctx}: {e}"))
    };

    // 1. First fork: the parent returns control to the caller's shell.
    // SAFETY: fork() is the standard Unix process-creation syscall. The only
    // soundness obligation is no async-signal-unsafe work between fork and
    // the following exec/exit; we do none.
    match unsafe { fork() }.map_err(|e| cfg("fork", e))? {
        ForkResult::Parent { .. } => std::process::exit(0),
        ForkResult::Child => {}
    }

    // 2. New session + process group, no controlling terminal.
    setsid().map_err(|e| cfg("setsid", e))?;

    // 3. Second fork: the daemon can never reacquire a terminal.
    // SAFETY: same as the first fork — no async-signal-unsafe work follows it.
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
    // Keep /dev/null open for the lifetime of the daemon (the duplicated fds
    // hold it); dropping the original handle is safe once dup2 has run.
    std::mem::forget(devnull);

    Ok(())
}
