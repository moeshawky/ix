//! ix background daemon — ixd.
//!
//! Thin wrapper around [`ix::daemon::run`]. The full guarded daemon logic
//! (`LLMOSafe` `ResourceGuard`, entropy monitoring, Unix-domain socket,
//! file watching, beacon arbitration) lives in the library.
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
    about = "Background daemon for automatic indexing with safety monitoring."
)]
struct Cli {
    #[arg(default_value = ".", value_name = "PATH")]
    path: PathBuf,
}

fn main() -> ix::error::Result<()> {
    let cli = Cli::parse();
    ix::daemon::run(&cli.path)
}
