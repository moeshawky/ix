//! ix CLI entry point.
//!
//! Usage:
//! ix "pattern" [path]
//! ix --build [path]
//! ix --regex "pattern" [path]

#![warn(clippy::pedantic)]

mod args;
mod commands;
mod output;
mod service;

use args::{Cli, Command, IndexFlags, MatchFlags, OutputFlags, SearchFlags, SearchParams};
use clap::Parser;
use std::path::PathBuf;

fn main() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .try_init();

    let cli = Cli::parse();

    if cli.threads > 0
        && let Err(e) = rayon::ThreadPoolBuilder::new()
            .num_threads(cli.threads)
            .build_global()
    {
        eprintln!("ix: warning: failed to initialize global thread pool: {e}");
    }

    if let Some(ref cmd) = cli.command {
        handle_subcommand(cmd);
        return;
    }

    if cli.daemon {
        handle_daemon(&cli.path);
        return;
    }

    validate_feature_flags(&cli);

    if let Some(ref build_path) = cli.build {
        handle_build(&cli, build_path);
        return;
    }

    // Multiple PATH arguments are accepted by the parser but only a single
    // search root is supported (the index is per-root). Fail loudly instead of
    // silently searching only the first (audit F1).
    if cli.path.len() > 1 {
        eprintln!(
            "ix: multiple PATH arguments are not supported; pass a single root (or search from their common parent)"
        );
        std::process::exit(2);
    }

    if cli.mode.multiline && !cli.mode.regex {
        eprintln!("ix: --multiline requires --regex (-r)");
        std::process::exit(1);
    }

    let Some(ref pattern) = cli.pattern else {
        eprintln!("Error: no pattern provided");
        std::process::exit(1);
    };

    // Stdin mode: --stdin flag or "-" path
    let is_stdin = cli.mode.stdin || cli.path.iter().any(|p| p == std::path::Path::new("-"));

    if is_stdin {
        if let Err(e) = commands::do_stdin_stream_search(pattern, &cli) {
            eprintln!("Error searching stdin: {e}");
            std::process::exit(1);
        }
        return;
    }

    let search_path = cli
        .path
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("."));

    let params = SearchParams {
        pattern,
        path: &search_path,
        flags: SearchFlags {
            matching: MatchFlags {
                is_regex: cli.mode.regex,
                ignore_case: cli.mode.ignore_case,
                word_boundary: cli.mode.word,
                multiline: cli.mode.multiline,
            },
            index: IndexFlags {
                no_index: cli.scan.no_index,
                fresh: cli.scan.fresh,
                force: cli.force,
                decompress: cli.scan.decompress,
                archive: cli.scan.archive,
            },
            output: OutputFlags {
                json: cli.output.json,
                stats: cli.output.stats,
                count: cli.output.count,
                files_only: cli.output.files_only,
                binary: cli.output.binary,
            },
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

fn handle_subcommand(cmd: &Command) {
    match cmd {
        Command::Service { action } => {
            if let Err(e) = service::handle_service(action) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        Command::Stats { path, json } => {
            if let Err(e) = commands::do_stats(path, *json) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    }
}

fn handle_daemon(paths: &[PathBuf]) {
    #[cfg(all(feature = "notify", unix))]
    {
        let res = if paths.is_empty() {
            ix::daemon::run_many(&[PathBuf::from(".")])
        } else {
            ix::daemon::run_many(paths)
        };
        if let Err(e) = res {
            eprintln!("Error watching roots: {e}");
            std::process::exit(1);
        }
    }

    #[cfg(not(feature = "notify"))]
    {
        let _ = paths;
        eprintln!(
            "Error: daemon mode requires the 'notify' feature. Install with: cargo install ix --features notify"
        );
        std::process::exit(1);
    }

    #[cfg(all(feature = "notify", not(unix)))]
    {
        let _ = paths;
        eprintln!("Error: daemon mode is not supported on this platform");
        std::process::exit(1);
    }
}

#[cfg(not(all(feature = "archive", feature = "decompress")))]
fn validate_feature_flags(cli: &Cli) {
    #[cfg(not(feature = "archive"))]
    {
        if cli.scan.archive {
            eprintln!(
                "ix: error: --archive requires installing with `cargo install ix --features archive`"
            );
            std::process::exit(1);
        }
    }

    #[cfg(not(feature = "decompress"))]
    {
        if cli.scan.decompress {
            eprintln!(
                "ix: error: --decompress requires installing with `cargo install ix --features decompress`"
            );
            std::process::exit(1);
        }
    }
}

#[cfg(all(feature = "archive", feature = "decompress"))]
const fn validate_feature_flags(_cli: &Cli) {}

fn handle_build(cli: &Cli, build_path: &std::path::Path) {
    if let Some(ref extra) = cli.pattern {
        eprintln!(
            "ix: unexpected positional argument '{extra}' with --build; pass at most one path directly after --build (e.g. `ix --build <PATH> --force`)"
        );
        std::process::exit(2);
    }
    if !cli.path.is_empty() {
        eprintln!("ix: --build accepts at most one PATH argument");
        std::process::exit(2);
    }
    if let Err(e) = commands::do_build(
        build_path,
        cli.scan.decompress,
        cli.force,
        cli.max_file_size,
    ) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
