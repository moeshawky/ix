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

mod args;
mod commands;
mod output;
mod service;

use args::{Cli, Command, SearchFlags, SearchParams};
use clap::Parser;
use std::path::PathBuf;

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
                if let Err(e) = service::handle_service(&action) {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
            Command::Stats { path, json } => {
                if let Err(e) = commands::do_stats(&path, json) {
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

    #[cfg(not(feature = "archive"))]
    {
        if cli.archive {
            eprintln!(
                "ix: error: --archive requires installing with `cargo install moeix --features archive`"
            );
            std::process::exit(1);
        }
    }

    #[cfg(not(feature = "decompress"))]
    {
        if cli.decompress {
            eprintln!(
                "ix: error: --decompress requires installing with `cargo install moeix --features decompress`"
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
        if let Err(e) =
            commands::do_build(&search_path, cli.decompress, cli.force, cli.max_file_size)
        {
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

    // Stdin mode: --stdin flag or "-" path
    let is_stdin = cli.stdin || cli.path.iter().any(|p| p == std::path::Path::new("-"));

    if is_stdin {
        if let Err(e) = commands::do_stdin_stream_search(pattern, &cli) {
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
        max_file_size: cli.max_file_size,
        chunk_size: cli.chunk_size,
        chunk_overlap: cli.chunk_overlap,
    };

    if let Err(e) = commands::do_search(&params) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
