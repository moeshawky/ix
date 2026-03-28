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
    about = "Trigram code search — the grep replacement"
)]
struct Cli {
    /// Search pattern
    pattern: Option<String>,

    /// Directory to search in
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Build the index
    #[arg(long)]
    build: bool,

    /// Use regular expression for search
    #[arg(short, long)]
    regex: bool,

    /// Case-insensitive search
    #[arg(short, long)]
    ignore_case: bool,

    /// Force full scan (ignore index)
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

    if let Err(e) = do_search(&pattern, &cli.path, cli.regex, cli.no_index) {
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

fn do_search(pattern: &str, path: &Path, is_regex: bool, no_index: bool) -> ix::error::Result<()> {
    let index_path = path.join(".ix/shard.ix");

    if !no_index && index_path.exists() {
        let reader = Reader::open(&index_path)?;
        let plan = Planner::plan(pattern, is_regex);
        let executor = Executor::new(&reader);

        let (matches, stats) = executor.execute(&plan)?;

        for m in matches {
            println!(
                "{}:{}:{}:{}",
                m.file_path.display(),
                m.line_number,
                m.byte_offset,
                m.line_content
            );
        }

        tracing::debug!("Search stats: {:?}", stats);
    } else {
        let scanner = Scanner::new(path);
        let matches = scanner.scan(pattern, is_regex)?;

        for m in matches {
            println!(
                "{}:{}:{}:{}",
                m.file_path.display(),
                m.line_number,
                m.byte_offset,
                m.line_content
            );
        }
    }

    Ok(())
}
