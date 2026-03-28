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
    after_help = "EXAMPLES:
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

    /// Force a full file-system scan, ignoring any existing .ix index.
    #[arg(long)]
    no_index: bool,
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

    if let Err(e) = do_search(
        &pattern,
        &cli.path,
        cli.regex,
        cli.no_index,
        cli.json,
        cli.stats,
    ) {
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

fn do_search(
    pattern: &str,
    path: &Path,
    is_regex: bool,
    no_index: bool,
    json: bool,
    stats_flag: bool,
) -> ix::error::Result<()> {
    let index_path = path.join(".ix/shard.ix");
    let start_time = std::time::Instant::now();

    if !no_index && index_path.exists() {
        let reader = Reader::open(&index_path)?;
        let plan = Planner::plan(pattern, is_regex);
        let executor = Executor::new(&reader);

        let (matches, stats) = executor.execute(&plan)?;

        for m in matches {
            print_match(&m, json);
        }

        if stats_flag {
            print_stats(&stats, start_time.elapsed());
        }
    } else {
        let scanner = Scanner::new(path);
        let matches = scanner.scan(pattern, is_regex)?;

        for m in &matches {
            print_match(m, json);
        }

        if stats_flag {
            // Scanner doesn't provide QueryStats, so we provide an empty one or dummy
            let stats = ix::executor::QueryStats {
                total_matches: matches.len() as u32,
                ..Default::default()
            };
            print_stats(&stats, start_time.elapsed());
        }
    }

    Ok(())
}

fn print_match(m: &ix::executor::Match, json: bool) {
    let mut content = m.line_content.clone();
    if content.len() > 200 {
        content.truncate(200);
        content.push_str("...");
    }

    if json {
        println!(
            "{{\"file\":\"{}\",\"line\":{},\"content\":\"{}\",\"byte_offset\":{}}}",
            m.file_path.display(),
            m.line_number,
            content.replace('"', "\\\"").replace('\n', "\\n"),
            m.byte_offset
        );
    } else {
        println!(
            "{}:{}:{}:{}",
            m.file_path.display(),
            m.line_number,
            m.byte_offset,
            content
        );
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
