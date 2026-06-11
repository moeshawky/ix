use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "ix",
    version = env!("CARGO_PKG_VERSION"),
    about = "High-performance, safety-aware code search engine for humans and agents.",
after_help = r#"USAGE:

Existence check: ix -c "pattern" → Single integer (count)
Location: ix -l "pattern" → Unique file paths
Contextual: ix -C 3 "pattern" → ±3 lines around match
Structured: ix --json "pattern" → JSON Lines output
Deterministic: ix --fresh "pattern" → Force rebuild + search

SEARCH MODES (mutually exclusive):

1. Literal (default): ix "timeout" → exact substring match
2. Word-boundary: ix -w "timeout" → whole-word match (finds "timeout" but not "timeoutExceeded")
3. Regex: ix --regex "err(or|no).*timeout" → full regex pattern

EXAMPLES:

Index the current directory:
ix --build

Search for a literal string:
ix "ConnectionTimeout"

Search for whole word "timeout":
ix -w timeout

Search using a Regular Expression:
ix --regex "err(or|no).*timeout"

Search in a specific directory without using the index:
ix --no-index "TODO" ./src

NOTES:
 - Default is unlimited results (use -n N to cap at N results).
- Index stored in .ix/shard.ix relative to search path.
- Uses LLMOSafe for resource monitoring and back-pressure.
- Word-boundary (-w) uses regex internally but enforces whole-word semantics."#
)]
pub(crate) struct Cli {
    /// The pattern to search for (literal string by default).
    #[arg(value_name = "PATTERN")]
    pub(crate) pattern: Option<String>,

    /// The directories to search in (one or more).
    #[arg(value_name = "PATH", num_args = 0..)]
    pub(crate) path: Vec<PathBuf>,

    /// Build or update the .ix index for the target directory.
    #[arg(
  long,
  value_name = "PATH",
  num_args = 0..=1,
  default_missing_value = ".",
  help_heading = "Actions"
)]
    pub(crate) build: Option<PathBuf>,

    /// Interpret the pattern as a regular expression.
    #[arg(short, long)]
    pub(crate) regex: bool,

    /// Perform a case-insensitive search.
    #[arg(short, long)]
    pub(crate) ignore_case: bool,

    /// Match only word boundaries (e.g., "trigram" matches "the trigram is" but not "congratulations"). Requires literal mode.
    #[arg(short = 'w', long)]
    pub(crate) word: bool,

    /// Output results as JSON Lines (Schema: {file, line, col, content, `byte_offset`, `context_before`, `context_after`}).
    #[arg(long)]
    pub(crate) json: bool,

    /// Print search performance statistics to stderr.
    #[arg(long)]
    pub(crate) stats: bool,

    /// Print only the total match count.
    #[arg(short, long)]
    pub(crate) count: bool,

    /// Print only unique file paths of matching files.
    #[arg(short = 'l', long)]
    pub(crate) files_only: bool,

    /// Show N lines of context around each match.
    #[arg(short = 'C', long, default_value = "0")]
    pub(crate) context: usize,

    /// Stop after N results (0 for unlimited). Default: 0 (unlimited).
    #[arg(short = 'n', long, default_value = "0")]
    pub(crate) max_results: usize,

    /// Filter by file extensions (e.g. rs, py, ts).
    #[arg(short = 't', long = "type")]
    pub(crate) file_types: Vec<String>,

    /// Search inside compressed files (.gz, .zst, .bz2, .xz).
    #[arg(short = 'z', long)]
    pub(crate) decompress: bool,

    /// Number of search threads (0 = auto).
    #[arg(short = 'j', long, default_value = "0")]
    pub(crate) threads: usize,

    /// Enable multiline mode (dot matches newline). Requires --regex.
    #[arg(short = 'U', long)]
    pub(crate) multiline: bool,

    /// Search inside .zip and .tar.gz archives.
    #[arg(long)]
    pub(crate) archive: bool,

    /// Search binary files (normally skipped).
    #[arg(long)]
    pub(crate) binary: bool,

    /// Maximum file size to index in MB (0 = unlimited). Default: 100.
    #[arg(long, default_value = "100")]
    pub(crate) max_file_size: u64,

    /// Force full file-system scan, bypassing any existing .ix index.
    #[arg(long)]
    pub(crate) no_index: bool,

    /// Rebuild index before searching (ensures data freshness).
    #[arg(long)]
    pub(crate) fresh: bool,

    /// Force operation even if the search root is managed by a daemon.
    #[arg(long)]
    pub(crate) force: bool,

    /// Read pattern from stdin (pipe mode). Conflicts with --build.
    #[arg(long, conflicts_with = "build")]
    pub(crate) stdin: bool,

    /// Run as background daemon (ixd mode).
    #[arg(long, hide = true)]
    pub(crate) daemon: bool,

    /// Chunk size in bytes for streaming large files (0 = default `16 MiB`).
    #[arg(long, default_value = "0")]
    pub(crate) chunk_size: usize,

    /// Overlap between chunks in bytes (0 = default `1 MiB`).
    #[arg(long, default_value = "0")]
    pub(crate) chunk_overlap: usize,

    /// Run a subcommand: service management or index statistics.
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

#[derive(clap::Subcommand)]
pub(crate) enum Command {
    /// Manage ixd as a system service.
    #[command(name = "service")]
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Display detailed index statistics (version, file/trigram counts, section sizes, compression ratio).
    #[command(name = "stats")]
    Stats {
        /// Path to the directory (walks upward to find .ix/, defaults to CWD).
        #[arg(short = 'p', long = "path", value_name = "PATH", default_value = ".")]
        path: PathBuf,

        /// Output in JSON format for machine readability.
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Subcommand)]
pub(crate) enum ServiceAction {
    /// Install ixd as a user-level systemd service.
    Install {
        /// Directory to watch (defaults to $HOME).
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,
    },
    /// Start the ixd systemd service.
    Start,
    /// Stop the ixd systemd service.
    Stop,
    /// Restart the ixd systemd service.
    Restart,
    /// Check the status of the ixd daemon.
    Status {
        /// Directory to check (walks upward to find .ix/, defaults to CWD).
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,
        /// Output in JSON format for machine readability.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy)]
pub(crate) struct SearchFlags {
    pub(crate) is_regex: bool,
    pub(crate) ignore_case: bool,
    pub(crate) word_boundary: bool,
    pub(crate) no_index: bool,
    pub(crate) fresh: bool,
    pub(crate) force: bool,
    pub(crate) json: bool,
    pub(crate) stats: bool,
    pub(crate) count: bool,
    pub(crate) files_only: bool,
    pub(crate) decompress: bool,
    pub(crate) multiline: bool,
    pub(crate) archive: bool,
    pub(crate) binary: bool,
}

pub(crate) struct SearchParams<'a> {
    pub(crate) pattern: &'a str,
    pub(crate) path: &'a std::path::Path,
    pub(crate) flags: SearchFlags,
    pub(crate) context: usize,
    pub(crate) max_results: usize,
    pub(crate) file_types: &'a [String],
    pub(crate) max_file_size: u64,
    pub(crate) chunk_size: usize,
    pub(crate) chunk_overlap: usize,
}

/// Guard that changes the current working directory to `target` and restores
/// the original CWD on drop.
///
/// If restoration fails (extremely rare — e.g., original directory was deleted),
/// the error is logged to stderr as a best-effort warning.
pub(crate) struct CwdGuard {
    original: PathBuf,
}

impl CwdGuard {
    /// Save the current working directory and switch to `target`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if `current_dir()` or `set_current_dir()` fails.
    pub(crate) fn new(target: &std::path::Path) -> Result<Self, std::io::Error> {
        let original = std::env::current_dir()?;
        std::env::set_current_dir(target)?;
        Ok(Self { original })
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        if let Err(e) = std::env::set_current_dir(&self.original) {
            eprintln!("ix: warning: failed to restore working directory: {e}");
        }
    }
}

// --- JSON Output Shapes ----------------------------------------------------
// Each struct corresponds to a distinct JSON output format emitted by the CLI.
// Using serde_json ensures consistent escaping, null handling, and avoids
// the hand-built format! strings that produced different escaping at each call site.

/// JSON shape for daemon beacon status (`ix service status --json`).
#[derive(serde::Serialize)]
pub(crate) struct BeaconStatusJson {
    pub(crate) status: String,
    pub(crate) pid: i32,
    pub(crate) uptime_secs: Option<u64>,
    pub(crate) daemon_status: String,
    pub(crate) root: String,
    pub(crate) socket: Option<String>,
    pub(crate) instance_id: u64,
}

/// JSON shape for simple status responses (orphan, dead, `not_running`).
#[derive(serde::Serialize)]
pub(crate) struct SimpleStatusJson {
    pub(crate) status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stale_pid: Option<i32>,
}
