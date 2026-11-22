use std::path::{Path, PathBuf};

use crate::args::{Cli, CwdGuard, SearchParams};
use crate::output::{format_bytes, looks_like_regex, print_results, u64_to_f64};
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
    let reader = Reader::open(index_path)?;
    check_stale(&reader, index_root)?;

    let index_root_canon = index_root
        .canonicalize()
        .unwrap_or_else(|_| index_root.to_path_buf());
    let is_scoped = search_path_abs != index_root_canon;

    // Change CWD to index_root so relative paths in the index resolve.
    // CwdGuard restores the original CWD on drop (including on error paths).
    let _cwd_guard = CwdGuard::new(index_root)?;

    let exec_opts = if is_scoped && options.max_results > 0 {
        QueryOptions {
            max_results: 0,
            ..options.clone()
        }
    } else {
        options.clone()
    };

    let delta_path = index_path.parent().map(|p| p.join("shard.ix.delta"));
    let (m, mut s) = ix::api::execute(
        &reader,
        params.pattern,
        ix::planner::QueryOptions {
            is_regex: params.flags.matching.is_regex,
            ignore_case: params.flags.matching.ignore_case,
            multiline: params.flags.matching.multiline,
            word_boundary: params.flags.matching.word_boundary,
        },
        &exec_opts,
        delta_path.as_deref(),
    )?;

    let mut filtered_matches: Vec<_> = m
        .into_iter()
        .filter(|m| {
            let abs_path = if m.file_path.is_absolute() {
                m.file_path.clone()
            } else {
                index_root_canon.join(&m.file_path)
            };
            abs_path.starts_with(search_path_abs)
                || abs_path
                    .canonicalize()
                    .is_ok_and(|c| c.starts_with(search_path_abs))
        })
        .collect();

    if is_scoped {
        if options.files_only {
            let mut seen = std::collections::HashSet::new();
            filtered_matches.retain(|m| seen.insert(m.file_path.clone()));
        }
        s.total_available = u32::try_from(filtered_matches.len()).unwrap_or(u32::MAX);
        if options.max_results > 0 && filtered_matches.len() > options.max_results {
            filtered_matches.truncate(options.max_results);
        }
        s.total_matches = u32::try_from(filtered_matches.len()).unwrap_or(u32::MAX);
    }

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
        is_regex: params.flags.matching.is_regex,
        ignore_case: params.flags.matching.ignore_case,
        word_boundary: params.flags.matching.word_boundary,
        max_results: params.max_results,
        context_lines: params.context,
        file_types: params.file_types.to_vec(),
        decompress: params.flags.index.decompress,
        multiline: params.flags.matching.multiline,
        archive: params.flags.index.archive,
        binary: params.flags.output.binary,
        count_only: params.flags.output.count,
        files_only: params.flags.output.files_only,
        search_path: Some(search_path_abs.to_path_buf()),
        progressive: false,
        chunk_size_bytes: params.chunk_size,
        chunk_overlap_bytes: params.chunk_overlap,
    };

    let results = match client.search(query) {
        Ok(r) => r,
        Err(e) => {
            // Surface the IPC failure instead of silently degrading (D
            // G-SEM-2): the user must be able to tell that results came
            // from the slower local path.
            eprintln!("ix: daemon search failed, falling back to local: {e}");
            return None;
        }
    };
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
    let flags = match (multiline, ignore_case) {
        (true, true) => "(?s)(?i)",
        (true, false) => "(?s)",
        (false, true) => "(?i)",
        (false, false) => "",
    };
    let regex_pat = if is_regex && word {
        format!("{flags}\\b(?:{pattern})\\b")
    } else if is_regex {
        format!("{flags}{pattern}")
    } else if word {
        // Word-boundary: wrap literal in \b word boundaries
        let escaped = regex::escape(pattern);
        format!("{flags}\\b{escaped}\\b")
    } else {
        let escaped = regex::escape(pattern);
        format!("{flags}{escaped}")
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
    let re = build_regex(
        pattern,
        cli.mode.regex,
        cli.mode.ignore_case,
        cli.mode.word,
        cli.mode.multiline,
    )?;

    let options = ix::executor::QueryOptions {
        count_only: cli.output.count,
        files_only: cli.output.files_only,
        max_results: cli.max_results,
        context_lines: cli.context,
        multiline: cli.mode.multiline,
        binary: cli.output.binary,
        chunk_size_bytes: cli.chunk_size,
        chunk_overlap_bytes: cli.chunk_overlap,
        ..Default::default()
    };

    // Warn about flags that have no effect in stdin mode.
    if cli.scan.archive {
        eprintln!("ix: warning: --archive has no effect in stdin mode");
    }
    if cli.scan.decompress {
        eprintln!("ix: warning: --decompress has no effect in stdin mode");
    }
    if !cli.file_types.is_empty() {
        eprintln!("ix: warning: --type has no effect in stdin mode");
    }
    if cli.scan.no_index {
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

    print_results(
        &matches,
        &stats,
        &options,
        cli.output.json,
        start_time,
        cli.output.stats,
    );

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
    {
        if !force {
            eprintln!(
                "Error: Search root is managed by ixd (PID {}). Updates are automatic. Use --force to override.",
                beacon.pid
            );
            std::process::exit(1);
        }
        eprintln!(
            "ix: warning: search root is managed by live ixd (PID {}); forcing out-of-band index rebuild",
            beacon.pid
        );
    }
    println!("Building index for {}...", path.display());
    let mut builder = Builder::new(path)?;
    builder.set_decompress(decompress);
    builder.set_max_file_size(max_file_size_mb.saturating_mul(1024 * 1024));

    // Apply config (exclude_patterns + watch_roots) so `ix --build`
    // and the daemon scope identically (audit F2).
    match Config::discover_under(path) {
        Ok(config) => builder = config.apply_to_builder(builder),
        Err(e) => {
            eprintln!("ix: warning: failed to load .ixd.toml config: {e}");
        }
    }

    let out = builder.build()?;
    if builder.stats().files_skipped_error > 0 {
        eprintln!(
            "[WARNING] {} file(s) skipped due to permission or I/O errors during build",
            builder.stats().files_skipped_error
        );
    }
    println!("Index built at {}", out.display());
    Ok(())
}

struct StatsReport<'a> {
    index_path: &'a Path,
    header: &'a ix::format::Header,
    shard_size: u64,
    version: String,
    build_time: String,
    build_time_iso: String,
    cdx_active: bool,
    cdx_ratio: Option<f64>,
    compression: f64,
    delta_size: Option<u64>,
    delta_entries: u64,
}

pub(crate) fn do_stats(path: &Path, json: bool) -> ix::error::Result<()> {
    let (index_path, index_root, _beacon) =
        find_index(path).ok_or_else(|| ix::error::Error::MissingIndex(path.to_path_buf()))?;

    if !index_path.exists() {
        if json {
            println!(
                "{{\"error\": \"No index found\", \"hint\": \"Run `ix --build` to create one.\"}}"
            );
        } else {
            eprintln!("No index found. Run `ix --build` to create one.");
        }
        return Err(ix::error::Error::MissingIndex(index_path));
    }

    let reader = Reader::open(&index_path)?;
    let h = &reader.header;
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
        Some(u64_to_f64(h.cdx_block_index_size) / u64_to_f64(h.trigram_table_size))
    } else {
        None
    };
    let compression: f64 = if h.source_bytes_total > 0 && shard_size > 0 {
        u64_to_f64(h.source_bytes_total) / u64_to_f64(shard_size)
    } else {
        0.0
    };

    let delta_path = index_root.join(".ix").join("shard.ix.delta");
    let delta_size = match std::fs::metadata(&delta_path) {
        Ok(m) => Some(m.len()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let delta_entries = if delta_size.is_some() {
        let dr = ix::reader::DeltaReader::open(&delta_path)?;
        u64::from(dr.total_file_entries)
    } else {
        0
    };

    let report = StatsReport {
        index_path: &index_path,
        header: h,
        shard_size,
        version,
        build_time,
        build_time_iso,
        cdx_active,
        cdx_ratio,
        compression,
        delta_size,
        delta_entries,
    };

    if json {
        print_stats_json(&report);
    } else {
        print_stats_human(&report);
    }

    Ok(())
}

fn print_stats_json(r: &StatsReport<'_>) {
    let h = r.header;
    let mut obj = serde_json::Map::new();
    obj.insert(
        "index_path".into(),
        serde_json::Value::String(r.index_path.display().to_string()),
    );
    obj.insert(
        "index_version".into(),
        serde_json::Value::String(r.version.clone()),
    );
    obj.insert(
        "build_time".into(),
        serde_json::Value::String(r.build_time_iso.clone()),
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
    obj.insert("shard_size_bytes".into(), serde_json::json!(r.shard_size));
    obj.insert("compression_ratio".into(), serde_json::json!(r.compression));
    obj.insert(
        "sections".into(),
        serde_json::json!({
            "trigram_table": { "offset": h.trigram_table_offset, "size": h.trigram_table_size },
            "file_table": { "offset": h.file_table_offset, "size": h.file_table_size },
            "string_pool": { "offset": h.string_pool_offset, "size": h.string_pool_size },
            "posting_data": { "offset": h.posting_data_offset, "size": h.posting_data_size },
            "bloom_data": { "offset": h.bloom_offset, "size": h.bloom_size },
            "cdx_block_index": { "offset": h.cdx_block_index_offset, "size": h.cdx_block_index_size },
        }),
    );
    obj.insert("cdx_active".into(), serde_json::json!(r.cdx_active));
    if let Some(ratio) = r.cdx_ratio {
        obj.insert("cdx_compression_ratio".into(), serde_json::json!(ratio));
    }
    obj.insert(
        "delta_size_bytes".into(),
        serde_json::json!(r.delta_size.unwrap_or(0)),
    );
    obj.insert(
        "delta_entry_count".into(),
        serde_json::json!(r.delta_entries),
    );
    println!("{}", serde_json::Value::Object(obj));
}

fn print_stats_human(r: &StatsReport<'_>) {
    let h = r.header;
    println!("Index: {}", r.index_path.display());
    println!("  Version: {}", r.version);
    println!("  Built at: {}", r.build_time);
    println!("  Files indexed: {}", h.file_count);
    println!("  Unique trigrams: {}", h.trigram_count);
    println!("  Source bytes: {}", format_bytes(h.source_bytes_total));
    println!("  Shard size: {}", format_bytes(r.shard_size));
    if r.compression > 0.0 {
        println!("  Overall compression: {:.2}x", r.compression);
    }
    if r.cdx_active {
        println!("  CDX active: yes");
        if let Some(ratio) = r.cdx_ratio {
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
    if let Some(ds) = r.delta_size {
        println!("  Delta file: {ds} bytes ({})", format_bytes(ds));
        println!("  Delta entries: {}", r.delta_entries);
    }
}

fn normalize_lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push(comp.as_os_str());
                }
            }
            Component::Normal(s) => out.push(s),
        }
    }
    out
}

pub(crate) fn find_index(path: &Path) -> Option<(PathBuf, PathBuf, Option<ix::format::Beacon>)> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let mut current = joined
        .canonicalize()
        .unwrap_or_else(|_| normalize_lexical(&joined));

    if current.is_file()
        && let Some(parent) = current.parent()
    {
        current = parent.to_path_buf();
    }

    loop {
        let index_dir = current.join(".ix");
        if index_dir.exists() {
            let index_file = index_dir.join("shard.ix");
            let beacon = match ix::format::Beacon::read_from(&index_dir) {
                Ok(b) => Some(b),
                Err(ix::error::Error::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound => {
                    None
                }
                Err(e) => {
                    tracing::warn!(
                        "ix: ignoring unreadable or corrupt beacon in {}: {e}",
                        index_dir.display()
                    );
                    None
                }
            };
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
    if params.flags.index.fresh && params.flags.index.no_index {
        return Err(ix::error::Error::Config(
            "--fresh cannot be combined with --no-index".into(),
        ));
    }

    let original_cwd = std::env::current_dir()?;
    let joined_path = if params.path.is_absolute() {
        params.path.to_path_buf()
    } else {
        original_cwd.join(params.path)
    };
    let search_path_abs = joined_path
        .canonicalize()
        .unwrap_or_else(|_| normalize_lexical(&joined_path));

    if params.flags.index.fresh {
        let enclosing_root = find_index(params.path).map(|(_, root, _)| root);
        let build_target = if let Some(ref root) = enclosing_root {
            root.as_path()
        } else if params.path.is_file() {
            params.path.parent().unwrap_or(params.path)
        } else {
            params.path
        };
        do_build(
            build_target,
            params.flags.index.decompress,
            params.flags.index.force,
            params.max_file_size,
        )?;
    }

    let start_time = std::time::Instant::now();

    // Shared file-type → extension map (audit F6): the same mapping the
    // daemon applies, so `--type` resolves identically locally and over IPC.
    let extensions = ix::file_types::expand(params.file_types);

    if params.flags.index.archive && !params.flags.index.no_index {
        eprintln!("ix: --archive is only supported with raw file scanner; bypassing index");
    }

    if params.flags.index.no_index && params.max_file_size != 100 {
        eprintln!(
            "ix: --max-file-size is for index building; scanner uses its own file-size limits"
        );
    }

    let skip_index = params.flags.index.no_index || params.flags.index.archive;
    let index_info = if skip_index {
        None
    } else {
        find_index(params.path)
    };

    let options = QueryOptions {
        count_only: params.flags.output.count,
        files_only: params.flags.output.files_only,
        max_results: params.max_results,
        type_filter: extensions,
        context_lines: params.context,
        decompress: params.flags.index.decompress,
        multiline: params.flags.matching.multiline,
        archive: params.flags.index.archive,
        binary: params.flags.output.binary,
        word_boundary: params.flags.matching.word_boundary,
        chunk_size_bytes: params.chunk_size,
        chunk_overlap_bytes: params.chunk_overlap,
    };

    let (mut matches, mut final_stats) =
        execute_indexed_or_scan(params, index_info.as_ref(), &options, &search_path_abs)?;

    final_stats.total_matches = u32::try_from(matches.len()).unwrap_or(u32::MAX);

    if matches.is_empty()
        && !params.flags.matching.is_regex
        && !params.flags.output.json
        && looks_like_regex(params.pattern)
    {
        eprintln!(
            "ix: literal mode returned 0 results. If this was meant as a regex, add --regex (-r)."
        );
    }

    matches.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then(a.line_number.cmp(&b.line_number))
    });

    print_results(
        &matches,
        &final_stats,
        &options,
        params.flags.output.json,
        start_time,
        params.flags.output.stats,
    );

    Ok(())
}

fn execute_indexed_or_scan(
    params: &SearchParams,
    index_info: Option<&(PathBuf, PathBuf, Option<ix::format::Beacon>)>,
    options: &QueryOptions,
    search_path_abs: &Path,
) -> ix::error::Result<(Vec<ix::executor::Match>, QueryStats)> {
    let Some((path, index_root, beacon_opt)) = index_info else {
        let scanner = Scanner::new(params.path);
        let (matches, scan_stats) = scanner.scan_with_stats(
            params.pattern,
            params.flags.matching.is_regex,
            params.flags.matching.ignore_case,
            options,
        )?;
        if scan_stats.files_skipped_io > 0 {
            eprintln!(
                "[WARNING] {} file(s) could not be read during scan (I/O error) — results may be incomplete",
                scan_stats.files_skipped_io
            );
        }
        if scan_stats.archive_entries_skipped > 0 {
            eprintln!(
                "[WARNING] {} archive entry(s) skipped due to error or safety limit — results may be incomplete",
                scan_stats.archive_entries_skipped
            );
        }
        let count = u32::try_from(matches.len()).unwrap_or(u32::MAX);
        let stats = QueryStats {
            total_matches: count,
            total_available: count,
            files_failed_verify: scan_stats.files_skipped_io,
            ..Default::default()
        };
        return Ok((matches, stats));
    };

    #[cfg(all(feature = "notify", unix))]
    if beacon_opt.as_ref().is_some_and(ix::format::Beacon::is_live) {
        if let Some((m, s)) =
            try_ipc_search(params, index_root, search_path_abs, beacon_opt.as_ref())
        {
            return Ok((m, s));
        }
    }
    #[cfg(not(all(feature = "notify", unix)))]
    let _ = beacon_opt;

    execute_local_search(params, path, index_root, options, search_path_abs)
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
