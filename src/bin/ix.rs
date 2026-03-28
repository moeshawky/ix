//! ix CLI entry point.
//!
//! Usage:
//!   ix "pattern" [path]
//!   ix --build [path]
//!   ix --regex "pattern" [path]

use clap::Parser;
use ix::builder::Builder;
use ix::executor::Executor;
use ix::planner::Planner;
use ix::reader::Reader;
use ix::scanner::Scanner;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "ix",
    version = "0.1.0",
    about = "ix: High-performance, byte-level code search using a trigram index.",
    after_help = "LLM AGENT USAGE:
    Quick count:     ix -c \"pattern\"              → \"14\"
    Find files:      ix -l \"pattern\"              → file paths
    With context:    ix -C 3 \"pattern\"            → ±3 lines around matches
    Rust files only: ix -t rs \"pattern\"           → only .rs files
    JSON output:     ix --json \"pattern\"          → machine-parseable
    Safe default:    ix \"pattern\"                 → max 100 results
    All results:     ix -n 0 \"pattern\"            → unlimited

EXAMPLES:
    Index the current directory:
        ix --build

    Search for a literal string:
        ix \"ConnectionTimeout\"

    Search using a Regular Expression:
        ix --regex \"err(or|no).*timeout\"

    Search in a specific directory without using the index:
        ix --no-index \"TODO\" ./src"
)]
struct Cli {
    /// The pattern to search for (literal string by default).
    #[arg(value_name = "PATTERN")]
    pattern: Option<String>,

    /// The directory to search in.
    #[arg(default_value = ".", value_name = "PATH")]
    path: PathBuf,

    /// Build or update the .ix index for the target directory.
    #[arg(long, help_heading = "Actions")]
    build: bool,

    /// Interpret the pattern as a regular expression.
    #[arg(short, long)]
    regex: bool,

    /// Perform a case-insensitive search (currently only for non-indexed scans).
    #[arg(short, long)]
    ignore_case: bool,

    /// Output results as JSON Lines.
    #[arg(long)]
    json: bool,

    /// Print search statistics to stderr.
    #[arg(long)]
    stats: bool,

    /// Print only the total match count.
    #[arg(short, long)]
    count: bool,

    /// Print only the unique file paths of matching files.
    #[arg(short = 'l', long)]
    files_only: bool,

    /// Show N lines of context around each match.
    #[arg(short = 'C', long, default_value = "0")]
    context: usize,

    /// Stop after N results (0 for unlimited).
    #[arg(short = 'n', long, default_value = "100")]
    max_results: usize,

    /// Only search files with these extensions (e.g. rs, py, ts).
    #[arg(short = 't', long = "type")]
    file_types: Vec<String>,

    /// Force a full file-system scan, ignoring any existing .ix index.
    #[arg(long)]
    no_index: bool,
}

struct SearchParams<'a> {
    pattern: &'a str,
    path: &'a Path,
    is_regex: bool,
    no_index: bool,
    json: bool,
    stats_flag: bool,
    count_flag: bool,
    files_only: bool,
    context: usize,
    max_results: usize,
    file_types: &'a [String],
}

fn main() {
    let cli = Cli::parse();

    if cli.build {
        if let Err(e) = do_build(&cli.path) {
            eprintln!("Error building index: {}", e);
            std::process::exit(1);
        }
        return;
    }

    let pattern = match cli.pattern {
        Some(p) => p,
        None => {
            eprintln!("Error: no pattern provided");
            std::process::exit(1);
        }
    };

    let params = SearchParams {
        pattern: &pattern,
        path: &cli.path,
        is_regex: cli.regex,
        no_index: cli.no_index,
        json: cli.json,
        stats_flag: cli.stats,
        count_flag: cli.count,
        files_only: cli.files_only,
        context: cli.context,
        max_results: cli.max_results,
        file_types: &cli.file_types,
    };

    if let Err(e) = do_search(params) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn do_build(path: &Path) -> ix::error::Result<()> {
    println!("Building index for {}...", path.display());
    let mut builder = Builder::new(path);
    let out = builder.build()?;
    println!("Index built at {}", out.display());
    Ok(())
}

fn do_search(params: SearchParams) -> ix::error::Result<()> {
    let index_path = params.path.join(".ix/shard.ix");
    let start_time = std::time::Instant::now();

    // Map shorthand types to actual extensions
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

    let options = ix::executor::QueryOptions {
        count_only: params.count_flag,
        files_only: params.files_only,
        max_results: params.max_results,
        type_filter: extensions,
        context_lines: params.context,
    };

    let (matches, stats) = if !params.no_index && index_path.exists() {
        let reader = Reader::open(&index_path)?;
        let plan = Planner::plan(params.pattern, params.is_regex);
        let executor = Executor::new(&reader);
        executor.execute(&plan, &options)?
    } else {
        let scanner = Scanner::new(params.path);
        let matches = scanner.scan(params.pattern, params.is_regex, &options)?;
        let stats = ix::executor::QueryStats {
            total_matches: matches.len() as u32,
            ..Default::default()
        };
        (matches, stats)
    };

    if options.count_only {
        if params.json {
            println!("{{\"count\": {}}}", stats.total_matches);
        } else {
            println!("{}", stats.total_matches);
        }
    } else if options.files_only {
        let mut unique_files: std::collections::HashSet<PathBuf> =
            matches.iter().map(|m| m.file_path.clone()).collect();
        let mut sorted_files: Vec<_> = unique_files.drain().collect();
        sorted_files.sort();

        if params.json {
            let paths: Vec<String> = sorted_files
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            println!("{{\"files\": {:?}}}", paths);
        } else {
            for f in sorted_files {
                println!("{}", f.display());
            }
        }
    } else {
        let mut last_file = PathBuf::new();
        let mut last_line = 0;

        for m in &matches {
            if options.context_lines > 0 && !params.json {
                if m.file_path != last_file {
                    if !last_file.as_os_str().is_empty() {
                        println!("--");
                    }
                } else if m.line_number > last_line + 1 {
                    println!("--");
                }
            }

            print_match(m, params.json, options.context_lines);

            last_file = m.file_path.clone();
            last_line = m.line_number + m.context_after.len() as u32;
        }

        if options.max_results > 0 && stats.total_matches >= options.max_results as u32 {
            eprintln!(
                "ix: showing {} of {}+ matches (use -n 0 for all)",
                matches.len(),
                stats.total_matches
            );
        }
    }

    if params.stats_flag {
        print_stats(&stats, start_time.elapsed());
    }

    Ok(())
}

fn print_match(m: &ix::executor::Match, json: bool, context: usize) {
    let truncate = |s: &str| -> String {
        let max_bytes = 200;
        if s.len() <= max_bytes {
            return s.to_string();
        }
        let mut end = max_bytes;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    };

    if json {
        let content = truncate(&m.line_content);
        let context_before: Vec<String> = m.context_before.iter().map(|s| truncate(s)).collect();
        let context_after: Vec<String> = m.context_after.iter().map(|s| truncate(s)).collect();

        println!(
            "{{\"file\":\"{}\",\"line\":{},\"col\":{},\"content\":\"{}\",\"byte_offset\":{},\"context_before\":{:?},\"context_after\":{:?}}}",
            m.file_path.display(),
            m.line_number,
            m.col,
            content.replace('"', "\\\"").replace('\n', "\\n"),
            m.byte_offset,
            context_before,
            context_after
        );
    } else {
        if context > 0 {
            for (i, line) in m.context_before.iter().enumerate() {
                let line_num = m.line_number as usize - m.context_before.len() + i;
                println!("{}:{}:- :{}", m.file_path.display(), line_num, truncate(line));
            }
        }

        println!(
            "{}:{}:{}:{}",
            m.file_path.display(),
            m.line_number,
            m.byte_offset,
            truncate(&m.line_content)
        );

        if context > 0 {
            for (i, line) in m.context_after.iter().enumerate() {
                let line_num = m.line_number as usize + 1 + i;
                println!("{}:{}:- :{}", m.file_path.display(), line_num, truncate(line));
            }
        }
    }
}

fn print_stats(stats: &ix::executor::QueryStats, elapsed: std::time::Duration) {
    eprintln!("--- ix stats ---");
    eprintln!("trigrams_queried: {}", stats.trigrams_queried);
    eprintln!("posting_lists_decoded: {}", stats.posting_lists_decoded);
    eprintln!("candidate_files: {}", stats.candidate_files);
    eprintln!("files_verified: {}", stats.files_verified);
    eprintln!("bytes_verified: {}", stats.bytes_verified);
    eprintln!("total_matches: {}", stats.total_matches);
    eprintln!("search_time_ms: {}", elapsed.as_millis());
}
