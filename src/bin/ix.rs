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

    /// Manage ixd as a system service.
    #[command(subcommand)]
    service: Option<ServiceCommand>,
}

#[derive(clap::Subcommand)]
enum ServiceCommand {
    /// Install ixd as a user-level systemd service.
    #[command(name = "service")]
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(clap::Subcommand)]
enum ServiceAction {
    /// Install ixd as a user-level systemd service. Writes a unit file to
    /// ~/.config/systemd/user/ixd.service and enables it with `systemctl --user`.
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
    );
    let mut executor = Executor::new(&reader);

    if let Some(delta_path) = index_path.parent().map(|p| p.join("shard.ix.delta")) {
        executor.set_delta_path(delta_path);
    }

    let rss = llmosafe::ResourceGuard::current_rss_bytes();
    let sys_mem = llmosafe::ResourceGuard::system_memory_bytes();
    if sys_mem > 0 {
        let pressure = u8::try_from(rss.saturating_mul(100).saturating_div(sys_mem)).unwrap_or(100);
        let zone = ix::cache_policy::PressureZone::from_pressure(pressure);
        let allow = !matches!(
            zone,
            ix::cache_policy::PressureZone::Orange | ix::cache_policy::PressureZone::Red
        );
        executor.posting_cache().set_admit(allow);
        executor.neg_cache().set_admit(allow);
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
fn try_ipc_search(
    params: &SearchParams,
    index_root: &Path,
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
    };

    let results = client.search(query).ok()?;
    Some((results.matches, results.stats))
}

fn main() {
    let cli = Cli::parse();

    if cli.threads > 0 {
        rayon::ThreadPoolBuilder::new()
            .num_threads(cli.threads)
            .build_global()
            .unwrap();
    }

    if let Some(service) = cli.service {
        if let Err(e) = handle_service(service) {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        return;
    }

    #[cfg(feature = "notify")]
    {
        if cli.daemon {
            let paths: Vec<PathBuf> = if cli.path.is_empty() {
                vec![PathBuf::from(".")]
            } else {
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

#[cfg(feature = "notify")]
fn handle_service(cmd: ServiceCommand) -> ix::error::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let home =
            std::env::var("HOME").map_err(|_| ix::error::Error::Config("HOME not set".into()))?;
        let service_dir = PathBuf::from(&home).join(".config/systemd/user");
        let service_file = service_dir.join("ixd.service");

        match cmd {
            ServiceCommand::Service {
                action: ServiceAction::Install { path },
            } => {
                let watch_path = path.unwrap_or_else(|| PathBuf::from(&home));
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
MemoryMax=512M
MemoryHigh=384M
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
                let status = std::process::Command::new("systemctl")
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
            ServiceCommand::Service {
                action: ServiceAction::Start,
            } => {
                let status = std::process::Command::new("systemctl")
                    .args(["--user", "enable", "--now", "ixd"])
                    .status()?;
                if !status.success() {
                    return Err(ix::error::Error::Config(
                        "Failed to start ixd service".into(),
                    ));
                }
                println!("ixd service started.");
            }
            ServiceCommand::Service {
                action: ServiceAction::Stop,
            } => {
                let status = std::process::Command::new("systemctl")
                    .args(["--user", "stop", "ixd"])
                    .status()?;
                if !status.success() {
                    return Err(ix::error::Error::Config(
                        "Failed to stop ixd service".into(),
                    ));
                }
                println!("ixd service stopped.");
            }
            ServiceCommand::Service {
                action: ServiceAction::Restart,
            } => {
                let status = std::process::Command::new("systemctl")
                    .args(["--user", "restart", "ixd"])
                    .status()?;
                if !status.success() {
                    return Err(ix::error::Error::Config(
                        "Failed to restart ixd service".into(),
                    ));
                }
                println!("ixd service restarted.");
            }
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
fn handle_service(_cmd: ServiceCommand) -> ix::error::Result<()> {
    eprintln!("Error: ix service commands require the 'notify' feature.");
    eprintln!("Install with: cargo install moeix --features notify");
    std::process::exit(1);
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

    let (matches, stats) = if let Some((path, index_root, beacon_opt)) = &index_info {
        // Check if daemon is live and try IPC search first
        let daemon_managed = beacon_opt.as_ref().is_some_and(ix::format::Beacon::is_live);

        if daemon_managed {
            // Try IPC search with silent fallback
            match try_ipc_search(params, index_root) {
                Some((m, s)) => (m, s),
                None => execute_local_search(params, path, index_root, &options, &search_path_abs)?,
            }
        } else {
            // No daemon, execute locally
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
                if !printed_lines.contains(&line_num) {
                    println!(
                        "{}:{}:- :{}",
                        m.file_path.display(),
                        line_num,
                        truncate(line)
                    );
                    printed_lines.insert(line_num);
                }
            }
        }

        if !printed_lines.contains(&m.line_number) {
            println!(
                "{}:{}: {}",
                m.file_path.display(),
                m.line_number,
                truncate(&m.line_content)
            );
            printed_lines.insert(m.line_number);
        }

        if context > 0 {
            for (i, line) in m.context_after.iter().enumerate() {
                let line_num = (m.line_number as usize + 1 + i) as u32;
                if !printed_lines.contains(&line_num) {
                    println!(
                        "{}:{}:- :{}",
                        m.file_path.display(),
                        line_num,
                        truncate(line)
                    );
                    printed_lines.insert(line_num);
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
    if last_mod > reader.header.created_at.saturating_add(grace_period_micros) {
        let last_built_secs =
            i64::try_from(reader.header.created_at / 1_000_000).unwrap_or(i64::MAX);
        let datetime = chrono::DateTime::from_timestamp(last_built_secs, 0)
            .unwrap_or(chrono::DateTime::UNIX_EPOCH);
        let time_str = datetime.format("%Y-%m-%d %H:%M:%S").to_string();
        eprintln!("ix: index is stale (last built: {time_str}). Run 'ix --build' to update.");
    }
    Ok(())
}
