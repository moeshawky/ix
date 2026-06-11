use std::path::PathBuf;

use ix::executor::{Match, QueryOptions, QueryStats};

pub(crate) fn format_uptime(secs: u64) -> String {
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

#[allow(clippy::cast_precision_loss)]
pub(crate) fn format_bytes(bytes: u64) -> String {
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

pub(crate) fn truncate_safe(s: &mut String, max_bytes: usize) {
    if max_bytes >= s.len() {
        return;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

pub(crate) fn looks_like_regex(pattern: &str) -> bool {
    use regex_syntax::hir::HirKind;
    let mut parser = regex_syntax::ParserBuilder::new().utf8(false).build();
    let Ok(hir) = parser.parse(pattern) else {
        return false;
    };
    !matches!(hir.kind(), HirKind::Literal(_))
}

pub(crate) fn print_results(
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
        eprintln!(
            "[WARNING] {} file(s) could not be verified (I/O error) — results may be incomplete",
            stats.files_failed_verify
        );
    }
    eprintln!("bytes_verified: {}", stats.bytes_verified);
    if stats.lines_read > 0 {
        eprintln!("lines_read: {}", stats.lines_read);
    }
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
