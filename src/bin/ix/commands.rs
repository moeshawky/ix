use std::path::{Path, PathBuf};

use crate::args::{Cli, CwdGuard, SearchParams};
use crate::output::{format_bytes, looks_like_regex, print_results};
use ix::builder::Builder;
use ix::config::Config;
use ix::executor::{QueryOptions, QueryStats};
use ix::reader::Reader;
use ix::scanner::Scanner;
use regex::Regex;

/// Execute search locally using mmap (fallback when IPC is unavailable).
pub(crate) fn execute_local_search(
    params: &SearchParams,
    index_path: &Path,
    index_root: &Path,
    options: &QueryOptions,
    search_path_abs: &Path,
) -> Result<(Vec<ix::executor::Match>, ix::executor::QueryStats), ix::error::Error> {
    use ix::reader::Reader;

    let reader = Reader::open(index_path)?;
    check_stale(&reader, index_root)?;

    // Change CWD to index_root so relative paths in the index resolve.
    // CwdGuard restores the original CWD on drop (including on error paths).
    let _cwd_guard = CwdGuard::new(index_root)?;

    let delta_path = index_path.parent().map(|p| p.join("shard.ix.delta"));
    let (m, s) = ix::api::execute(
        &reader,
        params.pattern,
        ix::planner::QueryOptions {
            is_regex: params.flags.is_regex,
            ignore_case: params.flags.ignore_case,
            multiline: params.flags.multiline,
            word_boundary: params.flags.word_boundary,
        },
        options,
        delta_path.as_deref(),
    )?;

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

    Ok((filtered_matches, s))
}

/// Try to execute search via IPC to the daemon.
/// Returns `Some((matches, stats))` on success, `None` if daemon is unavailable
/// or the IPC call fails (triggers local fallback).
#[cfg(all(feature = "notify", unix))]
pub(crate) fn try_ipc_search(
    params: &SearchParams,
    index_root: &Path,
    search_path_abs: &Path,
    beacon: Option<&ix::format::Beacon>,
) -> Option<(Vec<ix::executor::Match>, ix::executor::QueryStats)> {
    use ix::daemon_sock::{DaemonClient, SearchQuery};

    // Prefer the daemon's authoritative socket path (recorded in beacon.json)
    // so the client connects to the socket the daemon actually bound, even
    // when environment/root canonicalization differs between the processes.
    let preferred_socket = beacon.and_then(|b| b.socket_path.as_deref());
    let mut client = DaemonClient::connect_with_socket(index_root, preferred_socket).ok()?;

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
        progressive: false,
        chunk_size_bytes: params.chunk_size,
        chunk_overlap_bytes: params.chunk_overlap,
    };

    let results = client.search(query).ok()?;
    Some((results.matches, results.stats))
}

/// Build a regex from CLI flags (regex mode, case sensitivity, word boundary, multiline).
pub(crate) fn build_regex(
    pattern: &str,
    is_regex: bool,
    ignore_case: bool,
    word: bool,
    multiline: bool,
) -> ix::error::Result<Regex> {
    let flags = if multiline { "(?s)" } else { "" };
    let regex_pat = if is_regex {
        if ignore_case {
            format!("{flags}(?i){pattern}")
        } else {
            format!("{flags}{pattern}")
        }
    } else if word {
        // Word-boundary: wrap literal in \b word boundaries
        let escaped = regex::escape(pattern);
        if ignore_case {
            format!("{flags}(?i)\\b{escaped}\\b")
        } else {
            format!("{flags}\\b{escaped}\\b")
        }
    } else {
        let escaped = regex::escape(pattern);
        if ignore_case {
            format!("{flags}(?i){escaped}")
        } else {
            format!("{flags}{escaped}")
        }
    };
    Ok(Regex::new(&regex_pat)?)
}

/// Search stdin using the streaming module.
///
/// Reads from `io::stdin().lock()` and pipes it through
/// [`ix::streaming::stream_file`], which handles binary detection, context
/// lines, and multiline mode consistently with file-based searches.
///
/// # Errors
///
/// Returns an error if reading from stdin fails.
pub(crate) fn do_stdin_stream_search(pattern: &str, cli: &Cli) -> ix::error::Result<()> {
    let re = build_regex(pattern, cli.regex, cli.ignore_case, cli.word, cli.multiline)?;

    let options = ix::executor::QueryOptions {
        count_only: cli.count,
        files_only: cli.files_only,
        max_results: cli.max_results,
        context_lines: cli.context,
        multiline: cli.multiline,
        binary: cli.binary,
        chunk_size_bytes: cli.chunk_size,
        chunk_overlap_bytes: cli.chunk_overlap,
        ..Default::default()
    };

    // Warn about flags that have no effect in stdin mode.
    if cli.archive {
        eprintln!("ix: warning: --archive has no effect in stdin mode");
    }
    if cli.decompress {
        eprintln!("ix: warning: --decompress has no effect in stdin mode");
    }
    if !cli.file_types.is_empty() {
        eprintln!("ix: warning: --type has no effect in stdin mode");
    }
    if cli.no_index {
        eprintln!("ix: warning: --no-index has no effect in stdin mode");
    }

    let start_time = std::time::Instant::now();
    let mut stream_stats = ix::streaming::StreamStats::default();
    let matches = ix::streaming::stream_file(
        std::io::stdin(),
        std::path::Path::new("-"),
        &re,
        &options,
        false,
        &mut stream_stats,
    )?;

    // Populate QueryStats from the StreamStats that stream_file filled in.
    // Without this, --stats in stdin mode would show zeros for all counters
    // except total_matches (which was set from matches.len()).
    let stats = ix::executor::QueryStats {
        files_verified: 1, // stdin is one input stream
        bytes_verified: stream_stats.bytes_read,
        lines_read: stream_stats.lines_read,
        total_matches: stream_stats.matches_found,
        ..Default::default()
    };

    print_results(&matches, &stats, &options, cli.json, start_time, cli.stats);

    if cli.max_results > 0 && matches.len() >= cli.max_results {
        eprintln!(
            "ix: output capped at {} results (use -n 0 for all)",
            cli.max_results
        );
    }

    Ok(())
}

pub(crate) fn do_build(
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

    // Load .ixd.toml config if present to apply exclude_patterns.
    // Only apply when a config file actually exists to avoid changing
    // default CLI behavior (the daemon uses its own defaults).
    if path.join(".ixd.toml").exists()
        && let Ok(config) = Config::discover_under(path)
        && !config.exclude_patterns.is_empty()
    {
        builder = builder.with_exclude_patterns(config.exclude_patterns);
    }

    let out = builder.build()?;
    println!("Index built at {}", out.display());
    Ok(())
}

#[allow(clippy::cast_precision_loss)]
pub(crate) fn do_stats(path: &Path, json: bool) -> ix::error::Result<()> {
    let (index_path, index_root, _beacon) = find_index(path).ok_or_else(|| {
        ix::error::Error::Config(format!(
            "no .ix/shard.ix found by walking up from {}",
            path.display()
        ))
    })?;

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

pub(crate) fn find_index(path: &Path) -> Option<(PathBuf, PathBuf, Option<ix::format::Beacon>)> {
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

pub(crate) fn do_search(params: &SearchParams) -> ix::error::Result<()> {
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
        multiline: params.flags.multiline,
        archive: params.flags.archive,
        binary: params.flags.binary,
        word_boundary: params.flags.word_boundary,
        chunk_size_bytes: params.chunk_size,
        chunk_overlap_bytes: params.chunk_overlap,
    };

    #[allow(unused_variables)]
    let (matches, stats) = if let Some((path, index_root, beacon_opt)) = &index_info {
        // Check if daemon is live and try IPC search first (notify+Unix only)
        #[cfg(unix)]
        {
            let daemon_managed = beacon_opt.as_ref().is_some_and(ix::format::Beacon::is_live);

            if daemon_managed {
                #[cfg(feature = "notify")]
                {
                    // Try IPC search with silent fallback
                    match try_ipc_search(params, index_root, &search_path_abs, beacon_opt.as_ref())
                    {
                        Some((m, s)) => (m, s),
                        None => execute_local_search(
                            params,
                            path,
                            index_root,
                            &options,
                            &search_path_abs,
                        )?,
                    }
                }
                #[cfg(not(feature = "notify"))]
                {
                    execute_local_search(params, path, index_root, &options, &search_path_abs)?
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

pub(crate) fn check_stale(reader: &Reader, index_root: &Path) -> ix::error::Result<()> {
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
