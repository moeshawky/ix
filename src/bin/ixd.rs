//! ix background daemon — ixd.
//!
//! Thin wrapper around [`ix::daemon::run_many`]. The full guarded daemon logic
//! (`LLMOSafe` `ResourceGuard`, entropy monitoring, Unix-domain socket,
//! file watching, beacon arbitration) lives in the library.
//!
//! ## Multi-root support (v0.8+)
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
//! Safety guarantees (v0.1.2+):
//! - `SIGTERM`/`SIGINT` → clean shutdown (beacon removed, watcher joined, no zombies)
//! - `Builder::new` returns `Result` — no panics on unwritable `.ix` dir
//! - RSS ceiling in builder (512MB) — OOM protection
//! - TOCTOU guards in `process_file` — skips vanished/permission-denied files

#![warn(clippy::pedantic)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::struct_field_names)]

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "ixd",
    version = env!("CARGO_PKG_VERSION"),
    about = "Background daemon that watches directories for changes and rebuilds the index.",
    after_help = "EXAMPLES:\n  \
                  ixd /path/to/repo\n  \
                  ixd /project-a /project-b /project-c\n\n\
                  DOCS:\n  \
                  https://github.com/moeshawky/ix/blob/main/docs/DAEMON-RUNBOOK.md\n  \
                  https://github.com/moeshawky/ix/blob/main/docs/.ixd.toml.md"
)]
struct Cli {
    /// One or more directories to watch (defaults to current directory if omitted).
    #[arg(default_value = ".", value_name = "PATH")]
    paths: Vec<PathBuf>,
}

fn main() -> ix::error::Result<()> {
    let cli = Cli::parse();
    ix::daemon::run_many(&cli.paths)
}
