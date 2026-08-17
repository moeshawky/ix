use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "ix",
    version = env!("CARGO_PKG_VERSION"),
    about = "High-performance code search via sparse-trigram indexing.",
after_help = r#"USAGE:

Discovery: ix -l "pattern" → Unique file paths
Existence:  ix -c "pattern" → Single integer (count)
Contextual: ix -C 3 "pattern" → ±3 lines around match
Structured: ix --json "pattern" → JSON Lines output (schema: {file, line, col, content, byte_offset, context_before, context_after})
Fresh:      ix --fresh "pattern" → Force rebuild + search
Pipe:       cat file | ix --stdin "pattern" → Search stdin

SEARCH MODES (mutually exclusive):

1. Literal (default):   ix "timeout" → exact substring
2. Word-boundary:       ix -w "timeout" → whole-word (matches "timeout" but not "timeoutExceeded")
3. Regex:               ix --regex "err(or|no).*timeout" → full regex syntax
   In regex mode the PATTERN is a regex: metacharacters (.*+?()[]|) are active and a
   space is a literal space. So `--regex "async expandQuery"` matches that exact
   contiguous text only. To match "async" and "expandQuery" with anything between
   them, use `--regex "async.*expandQuery"` (or `async[\s\S]*expandQuery` with -U).

FILTERING:

-t rs,py        Filter by file extension (repeatable)
--max-file-size N  Skip files larger than N MB (default: 100, 0 = unlimited)
--no-index          Bypass index, do a linear grep
--binary            Search binary files (normally skipped)
-z, --decompress    Search inside .gz, .zst, .bz2, .xz files
--archive           Search inside .zip and .tar.gz archives
    (--decompress and --archive require optional features; see install notes)

OUTPUT CONTROL:

-l, --files-only    Print only matching file paths
-c, --count         Print only total match count
--json              JSON Lines output (one object per match)
--stats             Print query performance statistics to stderr
-n N, --max-results N  Stop after N results (default: 0 = unlimited)

PERFORMANCE:

-j N, --threads N   Number of search threads (default: 0 = auto)
--chunk-size N      Chunk size in bytes for streaming large files (default: 16 MiB)
--chunk-overlap N   Overlap between chunks in bytes (default: 1 MiB)

ADVANCED:

-i, --ignore-case   Case-insensitive search
-U, --multiline     Dot matches newline (requires --regex)
--fresh             Rebuild index before searching
--force             Override daemon lock (operate on daemon-managed roots)

SUBCOMMANDS:

ix --build [PATH]       Build or update the .ix index (defaults to CWD)
ix service install [PATH]  Install ixd as a user-level systemd service
ix service start|stop|restart|status  Manage the systemd daemon
ix stats [PATH]         Display index statistics (files, trigrams, size)
    --json              Output stats in JSON format
ix service status --json  Machine-readable daemon status

EXAMPLES:

  # Build the index
  ix --build

  # Literal search
  ix "ConnectionTimeout"

  # Case-insensitive whole-word search
  ix -i -w timeout

  # Regex with file-type filter
  ix --regex "fn\s+\w+_handler" -t rs -t py

  # JSON output with context
  ix --json -C 2 "TODO"

  # Count occurrences without indexing
  ix --no-index -c "FIXME" ./src

  # Pipe stdin (e.g., from a build log)
  make 2>&1 | ix --stdin "error"

NOTES:
 - Index stored in .ix/shard.ix relative to search path.
 - Word-boundary (-w) uses regex internally but enforces whole-word semantics.
 - --decompress and --archive require: cargo install moeix --features full
 - --json schema: {file, line, col, content, byte_offset, context_before, context_after}"#
)]
pub(crate) struct Cli {
    /// The pattern to search for (literal string by default).
    #[arg(value_name = "PATTERN")]
    pub(crate) pattern: Option<String>,

    /// The directory to search in (a single root; `-` for stdin).
    ///
    /// The parser accepts `0..` positional paths so a stray extra path fails
    /// loudly (see main.rs) instead of being silently ignored.
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

    /// Interpret the pattern as a regular expression (metacharacters active; a space
    /// is a literal space, so a multi-word pattern matches contiguously unless you
    /// use e.g. `.*` between the words).
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
        #[arg(value_name = "PATH", default_value = ".")]
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
