//! ix CLI entry point.
//!
//! Usage:
//! ix "pattern" [path]
//! ix --build [path]
//! ix --regex "pattern" [path]

#![warn(clippy::pedantic)]
#![allow(clippy::struct_excessive_bools)] // CLI and SearchParams have many toggles by nature
#![allow(clippy::cast_possible_truncation)] // line numbers fit in u32; file sizes bounded
#![allow(clippy::too_many_lines)] // output formatting is inherently verbose

use clap::Parser;
use ix::builder::Builder;
use ix::executor::{Match, QueryOptions, QueryStats};
use ix::reader::Reader;
use ix::scanner::Scanner;
use regex::Regex;
use regex_syntax::hir::HirKind;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
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
struct Cli {
    /// The pattern to search for (literal string by default).
    #[arg(value_name = "PATTERN")]
    pattern: Option<String>,

    /// The directories to search in (one or more).
    #[arg(value_name = "PATH", num_args = 0..)]
    path: Vec<PathBuf>,

    /// Build or update the .ix index for the target directory.
    #[arg(
  long,
  value_name = "PATH",
  num_args = 0..=1,
  default_missing_value = ".",
  help_heading = "Actions"
)]
    build: Option<PathBuf>,

    /// Interpret the pattern as a regular expression.
    #[arg(short, long)]
    regex: bool,

    /// Perform a case-insensitive search.
    #[arg(short, long)]
    ignore_case: bool,

    /// Match only word boundaries (e.g., "trigram" matches "the trigram is" but not "congratulations"). Requires literal mode.
    #[arg(short = 'w', long)]
    word: bool,

    /// Output results as JSON Lines (Schema: {file, line, col, content, `byte_offset`, `context_before`, `context_after`}).
    #[arg(long)]
    json: bool,

    /// Print search performance statistics to stderr.
    #[arg(long)]
    stats: bool,

    /// Print only the total match count.
    #[arg(short, long)]
    count: bool,

    /// Print only unique file paths of matching files.
    #[arg(short = 'l', long)]
    files_only: bool,

    /// Show N lines of context around each match.
    #[arg(short = 'C', long, default_value = "0")]
    context: usize,

    /// Stop after N results (0 for unlimited). Default: 0 (unlimited).
    #[arg(short = 'n', long, default_value = "0")]
    max_results: usize,

    /// Filter by file extensions (e.g. rs, py, ts).
    #[arg(short = 't', long = "type")]
    file_types: Vec<String>,

    /// Search inside compressed files (.gz, .zst, .bz2, .xz).
    #[arg(short = 'z', long)]
    decompress: bool,

    /// Number of search threads (0 = auto).
    #[arg(short = 'j', long, default_value = "0")]
    threads: usize,

    /// Enable multiline mode (dot matches newline). Requires --regex.
    #[arg(short = 'U', long)]
    multiline: bool,

    /// Search inside .zip and .tar.gz archives.
    #[arg(long)]
    archive: bool,

    /// Search binary files (normally skipped).
    #[arg(long)]
    binary: bool,

    /// Maximum file size to index in MB (0 = unlimited). Default: 100.
    #[arg(long, default_value = "100")]
    max_file_size: u64,

    /// Force full file-system scan, bypassing any existing .ix index.
    #[arg(long)]
    no_index: bool,

    /// Rebuild index before searching (ensures data freshness).
    #[arg(long)]
    fresh: bool,

    /// Force operation even if the search root is managed by a daemon.
    #[arg(long)]
    force: bool,

    /// Run as background daemon (ixd mode).
    #[arg(long, hide = true)]
    daemon: bool,

    /// Run a subcommand: service management or index statistics.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
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
enum ServiceAction {
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
struct SearchFlags {
    is_regex: bool,
    ignore_case: bool,
    word_boundary: bool,
    no_index: bool,
    fresh: bool,
    force: bool,
    json: bool,
    stats: bool,
    count: bool,
    files_only: bool,
    decompress: bool,
    multiline: bool,
    archive: bool,
    binary: bool,
}

struct SearchParams<'a> {
    pattern: &'a str,
    path: &'a Path,
    flags: SearchFlags,
    context: usize,
    max_results: usize,
    file_types: &'a [String],
    threads: usize,
    max_file_size: u64,
}

/// Execute search locally using mmap (fallback when IPC is unavailable).
fn execute_local_search(
    params: &SearchParams,
    index_path: &Path,
    index_root: &Path,
    options: &QueryOptions,
    search_path_abs: &Path,
) -> Result<(Vec<ix::executor::Match>, ix::executor::QueryStats), ix::error::Error> {
    use ix::executor::Executor;
    use ix::planner::Planner;
    use ix::reader::Reader;

    let reader = Reader::open(index_path)?;
    check_stale(&reader, index_root)?;

    std::env::set_current_dir(index_root)?;

    let plan = Planner::plan_with_options(
        params.pattern,
        ix::planner::QueryOptions {
            is_regex: params.flags.is_regex,
            ignore_case: params.flags.ignore_case,
            multiline: params.flags.multiline,
            word_boundary: params.flags.word_boundary,
        },
    )?;
    let mut executor = Executor::new(&reader);

    if let Some(delta_path) = index_path.parent().map(|p| p.join("shard.ix.delta")) {
        executor.set_delta_path(delta_path);
    }

    let rss = llmosafe::ResourceGuard::current_rss_bytes();
    let sys_mem = llmosafe::ResourceGuard::system_memory_bytes();
    if sys_mem > 0 {
        let rss_pct = rss.saturating_mul(100).saturating_div(sys_mem);
        if rss_pct < 60 {
            executor.posting_cache().set_admit(true);
            executor.neg_cache().set_admit(true);
        } else {
            let policy = ix::cache_policy::AdaptiveCachePolicy::new(0.6);
            let directive = policy.directive();
            executor
                .posting_cache()
                .set_admit(directive.allow_new_entries);
            executor.neg_cache().set_admit(directive.allow_new_entries);
        }
    }

    let (m, s) = executor.execute(&plan, options)?;

    let filtered_matches: Vec<_> = m
        .into_iter()
        .filter(|m| {
            let abs_path = if m.file_path.is_absolute() {
                m.file_path.clone()
            } else {
                index_root.join(&m.file_path)
            };
            abs_path.starts_with(search_path_abs)
        })
        .collect();

    let _ = std::env::set_current_dir(
        std::env::current_dir().unwrap_or_else(|_| params.path.to_path_buf()),
    );
    Ok((filtered_matches, s))
}

/// Try to execute search via IPC to the daemon.
/// Returns `Some((matches, stats))` on success, `None` if daemon is unavailable
/// or the IPC call fails (triggers local fallback).
#[cfg(unix)]
fn try_ipc_search(
    params: &SearchParams,
    index_root: &Path,
    search_path_abs: &Path,
) -> Option<(Vec<ix::executor::Match>, ix::executor::QueryStats)> {
    use ix::daemon_sock::{DaemonClient, SearchQuery};

    let mut client = DaemonClient::connect(index_root).ok()?;

    let query = SearchQuery {
        id: 1,
        pattern: params.pattern.to_string(),
        is_regex: params.flags.is_regex,
        ignore_case: params.flags.ignore_case,
        word_boundary: params.flags.word_boundary,
        max_results: params.max_results,
        context_lines: params.context,
        file_types: params.file_types.to_vec(),
        decompress: params.flags.decompress,
        multiline: params.flags.multiline,
        archive: params.flags.archive,
        binary: params.flags.binary,
        search_path: Some(search_path_abs.to_path_buf()),
        threads: params.threads,
    };

    let results = client.search(query).ok()?;
    Some((results.matches, results.stats))
}

fn main() {
    let cli = Cli::parse();

    if cli.threads > 0 {
        if let Err(e) = rayon::ThreadPoolBuilder::new()
            .num_threads(cli.threads)
            .build_global()
        {
            eprintln!("ix: warning: failed to initialize global thread pool: {e}");
        }
    }

    if let Some(cmd) = cli.command {
        match cmd {
            Command::Service { action } => {
                if let Err(e) = handle_service(&action) {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
            Command::Stats { path, json } => {
                if let Err(e) = do_stats(&path, json) {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
        return;
    }

    #[cfg(all(feature = "notify", unix))]
    {
        if cli.daemon {
            let paths: Vec<PathBuf> = if cli.path.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                #[allow(clippy::redundant_clone)]
                cli.path.clone()
            };
            for path in &paths {
                if let Err(e) = ix::daemon::run(path) {
                    eprintln!("Error watching {}: {e}", path.display());
                    std::process::exit(1);
                }
            }
            return;
        }
    }

    #[cfg(not(feature = "notify"))]
    {
        if cli.daemon {
            eprintln!(
                "Error: daemon mode requires the 'notify' feature. Install with: cargo install moeix --features notify"
            );
            std::process::exit(1);
        }
    }

    #[cfg(all(feature = "notify", not(unix)))]
    {
        if cli.daemon {
            eprintln!("Error: daemon mode is not supported on this platform");
            std::process::exit(1);
        }
    }

    // Determine path and handle build action
    let search_path = if let Some(ref build_path) = cli.build {
        // Build mode: path comes from --build flag, or CWD if not specified
        build_path.clone()
    } else if let Some(p) = cli.path.first() {
        p.clone()
    } else {
        PathBuf::from(".")
    };

    if cli.build.is_some() {
        if let Err(e) = do_build(&search_path, cli.decompress, cli.force, cli.max_file_size) {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        return;
    }

    if cli.multiline && !cli.regex {
        eprintln!("ix: --multiline requires --regex (-r)");
        std::process::exit(1);
    }

    let Some(ref pattern) = cli.pattern else {
        eprintln!("Error: no pattern provided");
        std::process::exit(1);
    };

    if search_path.to_str() == Some("(stdin)") {
        if let Err(e) = do_stdin_search(pattern, &cli) {
            eprintln!("Error searching stdin: {e}");
            std::process::exit(1);
        }
        return;
    }

    let params = SearchParams {
        pattern,
        path: &search_path,
        flags: SearchFlags {
            is_regex: cli.regex,
            ignore_case: cli.ignore_case,
            word_boundary: cli.word,
            no_index: cli.no_index,
            fresh: cli.fresh,
            force: cli.force,
            json: cli.json,
            stats: cli.stats,
            count: cli.count,
            files_only: cli.files_only,
            decompress: cli.decompress,
            multiline: cli.multiline,
            archive: cli.archive,
            binary: cli.binary,
        },
        context: cli.context,
        max_results: cli.max_results,
        file_types: &cli.file_types,
        threads: cli.threads,
        max_file_size: cli.max_file_size,
    };

    if let Err(e) = do_search(&params) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
fn find_systemctl() -> std::ffi::OsString {
    let usr_bin = std::path::Path::new("/usr/bin/systemctl");
    if usr_bin.exists() {
        return usr_bin.as_os_str().to_os_string();
    }
    let bin = std::path::Path::new("/bin/systemctl");
    if bin.exists() {
        return bin.as_os_str().to_os_string();
    }
    // Fallback to path lookup if neither exists
    std::ffi::OsString::from("systemctl")
}

#[cfg(not(target_os = "linux"))]
fn find_systemctl() -> std::ffi::OsString {
    std::ffi::OsString::from("systemctl")
}

#[cfg(feature = "notify")]
#[allow(clippy::unnecessary_wraps)]
fn handle_service(action: &ServiceAction) -> ix::error::Result<()> {
    if let ServiceAction::Status { path, json } = &action {
        handle_service_status(path.as_deref(), *json);
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        let home =
            std::env::var("HOME").map_err(|_| ix::error::Error::Config("HOME not set".into()))?;
        let service_dir = PathBuf::from(&home).join(".config/systemd/user");
        let service_file = service_dir.join("ixd.service");

        match action {
            ServiceAction::Install { path } => {
                let watch_path = path.clone().unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from(&home))
                });
                let watch_path_abs = watch_path.canonicalize().unwrap_or(watch_path);

                std::fs::create_dir_all(&service_dir)?;

                let ix_path = std::env::current_exe()?;
                let daemon_cmd = format!("{} --daemon", ix_path.display());

                let service_content = format!(
                    r"[Unit]
Description=ix background daemon
After=network.target

[Service]
ExecStart={} {}
Restart=on-failure
RestartSec=10
StartLimitBurst=3
StartLimitIntervalSec=60

[Install]
WantedBy=default.target
",
                    daemon_cmd,
                    watch_path_abs.display()
                );

                std::fs::write(&service_file, service_content)?;

                // Reload systemd
                let status = std::process::Command::new(find_systemctl())
                    .args(["--user", "daemon-reload"])
                    .status()?;
                if !status.success() {
                    return Err(ix::error::Error::Config(
                        "systemctl daemon-reload failed".into(),
                    ));
                }

                println!("ixd service installed at {}", service_file.display());
                println!("Watch path: {}", watch_path_abs.display());
                println!("Run 'ix service start' to start the daemon.");
            }
            ServiceAction::Start => {
                let status = std::process::Command::new(find_systemctl())
                    .args(["--user", "enable", "--now", "ixd"])
                    .status()?;
                if !status.success() {
                    return Err(ix::error::Error::Config(
                        "Failed to start ixd service".into(),
                    ));
                }
                println!("ixd service started.");
            }
            ServiceAction::Stop => {
                let status = std::process::Command::new(find_systemctl())
                    .args(["--user", "stop", "ixd"])
                    .status()?;
                if !status.success() {
                    return Err(ix::error::Error::Config(
                        "Failed to stop ixd service".into(),
                    ));
                }
                println!("ixd service stopped.");
            }
            ServiceAction::Restart => {
                let status = std::process::Command::new(find_systemctl())
                    .args(["--user", "restart", "ixd"])
                    .status()?;
                if !status.success() {
                    return Err(ix::error::Error::Config(
                        "Failed to restart ixd service".into(),
                    ));
                }
                println!("ixd service restarted.");
            }
            ServiceAction::Status { .. } => unreachable!(),
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("ix service commands are currently only supported on Linux (systemd).");
        Ok(())
    }
}

#[cfg(not(feature = "notify"))]
#[allow(clippy::unnecessary_wraps)]
fn handle_service(action: &ServiceAction) -> ix::error::Result<()> {
    if let ServiceAction::Status { path, json } = &action {
        handle_service_status(path.as_deref(), *json);
        return Ok(());
    }
    eprintln!("Error: ix service commands require the 'notify' feature.");
    eprintln!("Install with: cargo install moeix --features notify");
    std::process::exit(1);
}

fn handle_service_status(path: Option<&Path>, json: bool) {
    use std::time::{SystemTime, UNIX_EPOCH};

    let search_path = path.unwrap_or(Path::new("."));
    let beacon_opt = find_index(search_path).and_then(|(_, _, beacon)| beacon);

    match beacon_opt {
        Some(beacon) if beacon.is_live() => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let uptime = now.saturating_sub(beacon.start_time);
            if json {
                print_json_status("running", &beacon, Some(uptime));
            } else {
                println!("ixd daemon is running");
                println!("  PID: {}", beacon.pid);
                println!("  Uptime: {}", format_uptime(uptime));
                println!("  Status: {}", beacon.status);
                println!("  Root: {}", beacon.root.display());
                if let Some(ref sock) = beacon.socket_path {
                    println!("  Socket: {}", sock.display());
                }
            }
        }
        Some(beacon) => {
            #[cfg(unix)]
            let is_orphan = {
                use nix::sys::signal::kill;
                use nix::unistd::Pid;
                kill(Pid::from_raw(beacon.pid), None).is_ok()
            };
            #[cfg(not(unix))]
            let is_orphan = false;

            if is_orphan {
                if json {
                    println!("{{\"status\":\"orphan\",\"stale_pid\":{}}}", beacon.pid);
                } else {
                    println!("PID {} is not ixd (orphan beacon)", beacon.pid);
                }
            } else if json {
                println!("{{\"status\":\"dead\",\"stale_pid\":{}}}", beacon.pid);
            } else {
                println!(
                    "ixd daemon is not running (stale beacon from PID {})",
                    beacon.pid
                );
            }
        }
        None => {
            if json {
                println!("{{\"status\":\"not_running\"}}");
            } else {
                println!("ixd daemon is not running");
            }
        }
    }
}

fn print_json_status(status: &str, beacon: &ix::format::Beacon, uptime: Option<u64>) {
    let sock = beacon.socket_path.as_ref().map_or_else(
        || "null".to_string(),
        |p| {
            format!(
                "\"{}\"",
                p.display()
                    .to_string()
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
            )
        },
    );
    let u = uptime.map_or("null".to_string(), |v| v.to_string());
    let root = beacon
        .root
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    println!(
        "{{\"status\":\"{}\",\"pid\":{},\"uptime_secs\":{},\"daemon_status\":\"{}\",\"root\":\"{}\",\"socket\":{},\"instance_id\":{}}}",
        status, beacon.pid, u, beacon.status, root, sock, beacon.instance_id,
    );
}

fn format_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    let seconds = secs % 60;
    if days > 0 {
        format!("{days}d {hours}h {minutes}m {seconds}s")
    } else if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn do_stdin_search(pattern: &str, cli: &Cli) -> ix::error::Result<()> {
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    let regex_pat = if cli.regex {
        if cli.ignore_case {
            format!("(?i){pattern}")
        } else {
            pattern.to_string()
        }
    } else if cli.word {
        // Word-boundary: wrap literal in \b word boundaries
        let escaped = regex::escape(pattern);
        if cli.ignore_case {
            format!("(?i)\\b{escaped}\\b")
        } else {
            format!("\\b{escaped}\\b")
        }
    } else {
        let escaped = regex::escape(pattern);
        if cli.ignore_case {
            format!("(?i){escaped}")
        } else {
            escaped
        }
    };
    let re = Regex::new(&regex_pat)?;

    let lines: Vec<&str> = buffer.lines().collect();
    let mut matches = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if let Some(m) = re.find(line) {
            let context_before = if cli.context > 0 {
                let start = i.saturating_sub(cli.context);
                lines[start..i]
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect()
            } else {
                vec![]
            };

            let context_after = if cli.context > 0 {
                let end = (i + 1 + cli.context).min(lines.len());
                lines[i + 1..end]
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect()
            } else {
                vec![]
            };

            matches.push(Match {
                file_path: PathBuf::from("(stdin)"),
                line_number: (i + 1) as u32,
                col: (m.start() + 1) as u32,
                line_content: if cli.count {
                    String::new()
                } else {
                    line.to_string()
                },
                byte_offset: 0,
                context_before,
                context_after,
                is_binary: false,
            });

            if cli.max_results > 0 && matches.len() >= cli.max_results {
                break;
            }
        }
    }

    if cli.count {
        if cli.json {
            println!("{{\"count\": {}}}", matches.len());
        } else {
            println!("{}", matches.len());
        }
    } else if cli.files_only {
        if !matches.is_empty() {
            if cli.json {
                println!("{{\"files\": [\"(stdin)\"]}}");
            } else {
                println!("(stdin)");
            }
        }
    } else {
        let mut printed_lines = std::collections::HashSet::new();
        for m in &matches {
            print_match(m, cli.json, cli.context, &mut printed_lines);
        }

        if cli.max_results > 0 && matches.len() >= cli.max_results {
            eprintln!(
                "ix: output capped at {} results (use -n 0 for all)",
                cli.max_results
            );
        }
    }

    Ok(())
}

fn do_build(
    path: &Path,
    decompress: bool,
    force: bool,
    max_file_size_mb: u64,
) -> ix::error::Result<()> {
    // Beacon check
    if let Some((_, _, Some(beacon))) = find_index(path)
        && beacon.is_live()
        && !force
    {
        eprintln!(
            "Error: Search root is managed by ixd (PID {}). Updates are automatic. Use --force to override.",
            beacon.pid
        );
        std::process::exit(1);
    }
    println!("Building index for {}...", path.display());
    let mut builder = Builder::new(path)?;
    builder.set_decompress(decompress);
    builder.set_max_file_size(max_file_size_mb * 1024 * 1024);
    let out = builder.build()?;
    println!("Index built at {}", out.display());
    Ok(())
}

#[allow(clippy::cast_precision_loss)]
fn do_stats(path: &Path, json: bool) -> ix::error::Result<()> {
    let (index_path, index_root, _beacon) =
        find_index(path).ok_or_else(|| ix::error::Error::Config("no index found".into()))?;

    if !index_path.exists() {
        if json {
            println!(
                "{{\"error\": \"No index found\", \"hint\": \"Run `ix build` to create one.\"}}"
            );
        } else {
            println!("No index found. Run `ix build` to create one.");
        }
        return Ok(());
    }

    let reader = Reader::open(&index_path)?;
    let h = &reader.header;
    #[allow(clippy::cast_precision_loss)]
    let shard_size = std::fs::metadata(&index_path)?.len();

    let version = format!("{}.{}", h.version_major, h.version_minor);
    let ts_secs = i64::try_from(h.created_at / 1_000_000).unwrap_or(0);
    let ts_nanos = u32::try_from(h.created_at % 1_000_000 * 1000).unwrap_or(0);
    let build_time = chrono::DateTime::from_timestamp(ts_secs, ts_nanos).map_or_else(
        || "unknown".to_string(),
        |dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
    );
    let build_time_iso = chrono::DateTime::from_timestamp(ts_secs, ts_nanos).map_or_else(
        || "unknown".to_string(),
        |dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    );

    let cdx_active = h.has_cdx() && h.cdx_block_index_size > 0;
    let cdx_ratio: Option<f64> = if cdx_active && h.trigram_table_size > 0 {
        Some(h.cdx_block_index_size as f64 / h.trigram_table_size as f64)
    } else {
        None
    };
    #[allow(clippy::cast_precision_loss)]
    let compression: f64 = if h.source_bytes_total > 0 && shard_size > 0 {
        h.source_bytes_total as f64 / shard_size as f64
    } else {
        0.0
    };

    let ix_dir = index_root.join(".ix");
    let delta_path = ix_dir.join("shard.ix.delta");
    let delta_size = std::fs::metadata(&delta_path).map(|m| m.len()).ok();
    let delta_entries = delta_size
        .and_then(|_| {
            let dr = ix::reader::DeltaReader::open(&delta_path).ok()?;
            Some(u64::from(dr.total_file_entries))
        })
        .unwrap_or(0);

    if json {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "index_path".into(),
            serde_json::Value::String(index_path.display().to_string()),
        );
        obj.insert("index_version".into(), serde_json::Value::String(version));
        obj.insert(
            "build_time".into(),
            serde_json::Value::String(build_time_iso),
        );
        obj.insert(
            "build_timestamp_us".into(),
            serde_json::Value::Number(h.created_at.into()),
        );
        obj.insert("file_count".into(), serde_json::json!(h.file_count));
        obj.insert("trigram_count".into(), serde_json::json!(h.trigram_count));
        obj.insert(
            "source_bytes_total".into(),
            serde_json::json!(h.source_bytes_total),
        );
        obj.insert("shard_size_bytes".into(), serde_json::json!(shard_size));
        obj.insert("compression_ratio".into(), serde_json::json!(compression));
        obj.insert("sections".into(), serde_json::json!({
            "trigram_table": { "offset": h.trigram_table_offset, "size": h.trigram_table_size },
            "file_table": { "offset": h.file_table_offset, "size": h.file_table_size },
            "string_pool": { "offset": h.string_pool_offset, "size": h.string_pool_size },
            "posting_data": { "offset": h.posting_data_offset, "size": h.posting_data_size },
            "bloom_data": { "offset": h.bloom_offset, "size": h.bloom_size },
            "cdx_block_index": { "offset": h.cdx_block_index_offset, "size": h.cdx_block_index_size },
        }));
        obj.insert("cdx_active".into(), serde_json::json!(cdx_active));
        if let Some(ratio) = cdx_ratio {
            obj.insert("cdx_compression_ratio".into(), serde_json::json!(ratio));
        }
        obj.insert(
            "delta_size_bytes".into(),
            serde_json::json!(delta_size.unwrap_or(0)),
        );
        obj.insert("delta_entry_count".into(), serde_json::json!(delta_entries));
        println!("{}", serde_json::Value::Object(obj));
    } else {
        println!("Index: {}", index_path.display());
        println!("  Version: {version}");
        println!("  Built at: {build_time}");
        println!("  Files indexed: {}", h.file_count);
        println!("  Unique trigrams: {}", h.trigram_count);
        println!("  Source bytes: {}", format_bytes(h.source_bytes_total));
        println!("  Shard size: {}", format_bytes(shard_size));
        if compression > 0.0 {
            println!("  Overall compression: {compression:.2}x");
        }
        if cdx_active {
            println!("  CDX active: yes");
            if let Some(ratio) = cdx_ratio {
                println!("  CDX compression ratio: {ratio:.4}x");
            }
        }
        println!("  Sections:");
        println!(
            "    trigram_table:  offset={}, size={} ({})",
            h.trigram_table_offset,
            h.trigram_table_size,
            format_bytes(h.trigram_table_size)
        );
        println!(
            "    file_table:     offset={}, size={} ({})",
            h.file_table_offset,
            h.file_table_size,
            format_bytes(h.file_table_size)
        );
        println!(
            "    string_pool:    offset={}, size={} ({})",
            h.string_pool_offset,
            h.string_pool_size,
            format_bytes(h.string_pool_size)
        );
        println!(
            "    posting_data:   offset={}, size={} ({})",
            h.posting_data_offset,
            h.posting_data_size,
            format_bytes(h.posting_data_size)
        );
        println!(
            "    bloom_data:     offset={}, size={} ({})",
            h.bloom_offset,
            h.bloom_size,
            format_bytes(h.bloom_size)
        );
        println!(
            "    cdx_block_index: offset={}, size={} ({})",
            h.cdx_block_index_offset,
            h.cdx_block_index_size,
            format_bytes(h.cdx_block_index_size)
        );
        if let Some(ds) = delta_size {
            println!("  Delta file: {}  ({})", format_bytes(ds), format_bytes(ds));
            println!("  Delta entries: {delta_entries}");
        }
    }

    Ok(())
}

#[allow(clippy::cast_precision_loss)]
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.2} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.2} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn find_index(path: &Path) -> Option<(PathBuf, PathBuf, Option<ix::format::Beacon>)> {
    let mut current = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };

    if current.is_file()
        && let Some(parent) = current.parent()
    {
        current = parent.to_path_buf();
    }

    loop {
        let index_dir = current.join(".ix");
        if index_dir.exists() {
            let index_file = index_dir.join("shard.ix");
            let beacon = ix::format::Beacon::read_from(&index_dir).ok();
            if index_file.exists() || beacon.is_some() {
                return Some((index_file, current, beacon));
            }
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn truncate_safe(s: &mut String, max_bytes: usize) {
    if max_bytes >= s.len() {
        return;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

fn looks_like_regex(pattern: &str) -> bool {
    let mut parser = regex_syntax::ParserBuilder::new().utf8(false).build();
    let Ok(hir) = parser.parse(pattern) else {
        return false;
    };
    !matches!(hir.kind(), HirKind::Literal(_))
}

fn do_search(params: &SearchParams) -> ix::error::Result<()> {
    let original_cwd = std::env::current_dir()?;
    let search_path_abs = if params.path.is_absolute() {
        params.path.to_path_buf()
    } else {
        original_cwd.join(params.path)
    };

    let index_info = if params.flags.no_index {
        None
    } else {
        find_index(params.path)
    };

    if params.flags.fresh {
        // Normalize file paths to their parent directory for index building
        let build_path = if params.path.is_file() {
            params.path.parent().unwrap_or(params.path)
        } else {
            params.path
        };
        do_build(
            build_path,
            params.flags.decompress,
            params.flags.force,
            params.max_file_size,
        )?;
    }

    let start_time = std::time::Instant::now();

    let mut extensions = Vec::new();
    for t in params.file_types {
        match t.as_str() {
            "rs" => extensions.push("rs".to_string()),
            "py" => extensions.push("py".to_string()),
            "ts" => extensions.push("ts".to_string()),
            "js" => extensions.push("js".to_string()),
            "go" => extensions.push("go".to_string()),
            "c" => extensions.push("c".to_string()),
            "cpp" => {
                extensions.push("cpp".to_string());
                extensions.push("cc".to_string());
                extensions.push("cxx".to_string());
            }
            "h" => {
                extensions.push("h".to_string());
                extensions.push("hpp".to_string());
            }
            "md" => extensions.push("md".to_string()),
            "toml" => extensions.push("toml".to_string()),
            "yaml" => {
                extensions.push("yaml".to_string());
                extensions.push("yml".to_string());
            }
            "json" => extensions.push("json".to_string()),
            other => extensions.push(other.to_string()),
        }
    }

    if params.flags.archive && !params.flags.no_index {
        eprintln!("ix: --archive is only supported with --no-index (raw file scanner)");
    }

    if params.flags.no_index && params.max_file_size != 100 {
        eprintln!(
            "ix: --max-file-size is for index building; scanner uses its own file-size limits"
        );
    }

    let options = QueryOptions {
        count_only: params.flags.count,
        files_only: params.flags.files_only,
        max_results: params.max_results,
        type_filter: extensions,
        context_lines: params.context,
        decompress: params.flags.decompress,
        threads: params.threads,
        multiline: params.flags.multiline,
        archive: params.flags.archive,
        binary: params.flags.binary,
        word_boundary: params.flags.word_boundary,
    };

    #[allow(unused_variables)]
    let (matches, stats) = if let Some((path, index_root, beacon_opt)) = &index_info {
        // Check if daemon is live and try IPC search first (Unix only)
        #[cfg(unix)]
        {
            let daemon_managed = beacon_opt.as_ref().is_some_and(ix::format::Beacon::is_live);

            if daemon_managed {
                // Try IPC search with silent fallback
                match try_ipc_search(params, index_root, &search_path_abs) {
                    Some((m, s)) => (m, s),
                    None => {
                        execute_local_search(params, path, index_root, &options, &search_path_abs)?
                    }
                }
            } else {
                execute_local_search(params, path, index_root, &options, &search_path_abs)?
            }
        }
        #[cfg(not(unix))]
        {
            execute_local_search(params, path, index_root, &options, &search_path_abs)?
        }
    } else {
        let scanner = Scanner::new(params.path);
        let matches = scanner.scan(
            params.pattern,
            params.flags.is_regex,
            params.flags.ignore_case,
            &options,
        )?;
        let stats = QueryStats {
            total_matches: matches.len() as u32,
            ..Default::default()
        };
        (matches, stats)
    };

    let mut final_stats = stats;
    final_stats.total_matches = matches.len() as u32;

    if matches.is_empty()
        && !params.flags.is_regex
        && !params.flags.json
        && looks_like_regex(params.pattern)
    {
        eprintln!(
            "ix: literal mode returned 0 results. If this was meant as a regex, add --regex (-r)."
        );
    }

    let mut matches = matches;
    matches.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then(a.line_number.cmp(&b.line_number))
    });

    print_results(
        &matches,
        &final_stats,
        &options,
        params.flags.json,
        start_time,
        params.flags.stats,
    );

    Ok(())
}

fn print_results(
    matches: &[Match],
    stats: &QueryStats,
    options: &QueryOptions,
    json: bool,
    start_time: std::time::Instant,
    show_stats: bool,
) {
    if options.count_only {
        if json {
            println!("{{\"count\": {}}}", stats.total_matches);
        } else {
            println!("{}", stats.total_matches);
        }
    } else if options.files_only {
        let mut unique_files: std::collections::HashSet<PathBuf> =
            matches.iter().map(|m| m.file_path.clone()).collect();
        let mut sorted_files: Vec<_> = unique_files.drain().collect();
        sorted_files.sort();

        if json {
            let paths: Vec<String> = sorted_files
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            println!("{{\"files\": {paths:?}}}");
        } else {
            for f in sorted_files {
                println!("{}", f.display());
            }
        }
    } else {
        let mut last_file = PathBuf::new();
        let mut printed_lines = std::collections::HashSet::new();

        for m in matches {
            if m.file_path != last_file {
                if options.context_lines > 0 && !json && !last_file.as_os_str().is_empty() {
                    println!("--");
                }
                printed_lines.clear();
                last_file.clone_from(&m.file_path);
            } else if options.context_lines > 0 && !json {
                let match_start = (m.line_number as usize).saturating_sub(options.context_lines);
                let prev_end = printed_lines.iter().max().copied().unwrap_or(0) as usize;
                if match_start > prev_end + 1 && prev_end > 0 {
                    println!("--");
                }
            }

            print_match(m, json, options.context_lines, &mut printed_lines);
        }

        if options.max_results > 0 && stats.total_matches >= options.max_results as u32 {
            eprintln!(
                "ix: output capped at {} results (use -n 0 for all)",
                options.max_results
            );
        }
    }

    if show_stats {
        print_stats(stats, start_time.elapsed());
    }
}

fn print_match(
    m: &Match,
    json: bool,
    context: usize,
    printed_lines: &mut std::collections::HashSet<u32>,
) {
    if !json && m.is_binary {
        println!("Binary file {} matches", m.file_path.display());
        return;
    }

    let truncate = |s: &str| -> String {
        let mut string = s.to_string();
        if string.len() > 200 {
            truncate_safe(&mut string, 200);
            string.push_str("...");
        }
        string
    };

    if json {
        let line_content = truncate(&m.line_content);
        let context_before: Vec<String> = m.context_before.iter().map(|s| truncate(s)).collect();
        let context_after: Vec<String> = m.context_after.iter().map(|s| truncate(s)).collect();

        println!(
            "{{\"file\":\"{}\",\"line\":{},\"col\":{},\"content\":\"{}\",\"byte_offset\":{},\"context_before\":{:?},\"context_after\":{:?},\"is_binary\":{}}}",
            m.file_path.display(),
            m.line_number,
            m.col,
            line_content
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n"),
            m.byte_offset,
            context_before,
            context_after,
            m.is_binary
        );
    } else {
        if context > 0 {
            for (i, line) in m.context_before.iter().enumerate() {
                let line_num = (m.line_number as usize - m.context_before.len() + i) as u32;
                if printed_lines.insert(line_num) {
                    println!(
                        "{}:{}:- :{}",
                        m.file_path.display(),
                        line_num,
                        truncate(line)
                    );
                }
            }
        }

        if printed_lines.insert(m.line_number) {
            println!(
                "{}:{}: {}",
                m.file_path.display(),
                m.line_number,
                truncate(&m.line_content)
            );
        }

        if context > 0 {
            for (i, line) in m.context_after.iter().enumerate() {
                let line_num = (m.line_number as usize + 1 + i) as u32;
                if printed_lines.insert(line_num) {
                    println!(
                        "{}:{}:- :{}",
                        m.file_path.display(),
                        line_num,
                        truncate(line)
                    );
                }
            }
        }
    }
}

fn print_stats(stats: &QueryStats, elapsed: std::time::Duration) {
    eprintln!("--- ix stats ---");
    eprintln!("trigrams_queried: {}", stats.trigrams_queried);
    eprintln!("posting_lists_decoded: {}", stats.posting_lists_decoded);
    eprintln!("candidate_files: {}", stats.candidate_files);
    eprintln!("files_verified: {}", stats.files_verified);
    if stats.files_failed_verify > 0 {
        eprintln!("files_failed_verify: {}", stats.files_failed_verify);
    }
    eprintln!("bytes_verified: {}", stats.bytes_verified);
    eprintln!("total_matches: {}", stats.total_matches);
    if stats.posting_cache_hits > 0 || stats.posting_cache_misses > 0 {
        eprintln!(
            "posting_cache: {} hits / {} misses",
            stats.posting_cache_hits, stats.posting_cache_misses
        );
    }
    if stats.neg_cache_hits > 0 || stats.neg_cache_misses > 0 {
        eprintln!(
            "neg_cache: {} hits / {} misses",
            stats.neg_cache_hits, stats.neg_cache_misses
        );
    }
    eprintln!("search_time_ms: {}", elapsed.as_millis());
}

fn check_stale(reader: &Reader, index_root: &Path) -> ix::error::Result<()> {
    let last_mod = Reader::get_last_modified(index_root)?;
    // Add 5-second grace period to reduce false positives from concurrent edits
    let grace_period_micros: u64 = 5_000_000;
    let delta_mtime = std::fs::metadata(index_root.join(".ix").join("shard.ix.delta"))
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| {
            u64::try_from(
                t.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_micros(),
            )
            .ok()
        })
        .unwrap_or(0);
    let effective_created_at = std::cmp::max(reader.header.created_at, delta_mtime);
    if last_mod > effective_created_at.saturating_add(grace_period_micros) {
        let last_built_secs = i64::try_from(effective_created_at / 1_000_000).unwrap_or(i64::MAX);
        let datetime = chrono::DateTime::from_timestamp(last_built_secs, 0)
            .unwrap_or(chrono::DateTime::UNIX_EPOCH);
        let time_str = datetime.format("%Y-%m-%d %H:%M:%S").to_string();
        eprintln!("ix: index is stale (last built: {time_str}). Run 'ix --build' to update.");
    }
    Ok(())
}
