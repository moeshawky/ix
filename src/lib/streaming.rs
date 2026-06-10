use crate::error::Result;
use crate::executor::{Match, QueryOptions};
use crate::format::is_binary;
use regex::Regex;
use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

/// Default per-chunk size for large-file chunked streaming: `16 MiB`.
const DEFAULT_CHUNK_SIZE: usize = 16 * 1024 * 1024;

/// Default overlap between adjacent chunks: `1 MiB`.
/// Ensures matches that straddle a chunk boundary are still found.
const DEFAULT_CHUNK_OVERLAP: usize = DEFAULT_CHUNK_SIZE / 16;

/// Accumulated statistics from a streaming file search.
#[derive(Debug, Default)]
pub struct StreamStats {
    /// Number of lines read during the stream.
    pub lines_read: u32,
    /// Total bytes consumed from the reader.
    pub bytes_read: u64,
    /// Total regex matches found across all lines.
    pub matches_found: u32,
}

/// Stream search through a reader, returning all matches.
///
/// Accumulated statistics are written to `stats` (lines read, bytes consumed,
/// matches found).
///
/// # Errors
///
/// Returns an I/O error if reading fails.
pub fn stream_file<R: Read>(
    reader: R,
    path: &Path,
    regex: &Regex,
    options: &QueryOptions,
    is_compressed: bool,
    stats: &mut StreamStats,
) -> Result<Vec<Match>> {
    if options.multiline {
        return stream_file_multiline(reader, path, regex, options, is_compressed, stats);
    }

    let mut buf_reader = BufReader::new(reader);
    let mut matches = Vec::new();
    let mut line_number = 0u32;
    let mut byte_offset = 0u64;

    if !is_compressed {
        let buffer = buf_reader.fill_buf()?;
        let is_bin = is_binary(buffer);
        if is_bin && !options.binary {
            return Ok(vec![]);
        }
    }

    let mut line = String::new();
    let mut context_before = VecDeque::new();
    let mut pending_matches: Vec<Match> = Vec::new();

    while buf_reader.read_line(&mut line)? > 0 {
        line_number += 1;
        let line_len = u64::try_from(line.len()).unwrap_or(u64::MAX);
        let trimmed_line = line.trim_end().to_string();

        for m in &mut pending_matches {
            if m.context_after.len() < options.context_lines {
                m.context_after.push(trimmed_line.clone());
            }
        }

        let (completed, still_pending): (Vec<_>, Vec<_>) = pending_matches
            .into_iter()
            .partition(|m| m.context_after.len() >= options.context_lines);
        matches.extend(completed);
        pending_matches = still_pending;

        if let Some(m) = regex.find(&line) {
            let context_before_vec: Vec<String> = context_before.iter().cloned().collect();

            let new_match = Match {
                file_path: path.to_path_buf(),
                line_number,
                col: u32::try_from(m.start() + 1).unwrap_or(u32::MAX),
                line_content: if options.count_only {
                    String::new()
                } else {
                    trimmed_line.clone()
                },
                byte_offset: byte_offset + u64::try_from(m.start()).unwrap_or(u64::MAX),
                context_before: context_before_vec,
                context_after: vec![],
                is_binary: false,
            };

            if options.context_lines > 0 {
                pending_matches.push(new_match);
            } else {
                matches.push(new_match);
            }

            if options.max_results > 0
                && (matches.len() + pending_matches.len()) >= options.max_results
                && (pending_matches.is_empty() || matches.len() >= options.max_results)
            {
                break;
            }
        }

        if options.context_lines > 0 {
            if context_before.len() == options.context_lines {
                if let Some(mut old_line) = context_before.pop_front() {
                    old_line.clear();
                    old_line.push_str(&trimmed_line);
                    context_before.push_back(old_line);
                }
            } else {
                context_before.push_back(trimmed_line.clone());
            }
        }

        byte_offset += line_len;
        line.clear();
    }

    matches.extend(pending_matches);
    stats.lines_read = line_number;
    stats.bytes_read = byte_offset;
    stats.matches_found = matches.len() as u32;
    Ok(matches)
}

// UTF-8 boundary is guaranteed by searching for '\n' bytes before slicing
#[allow(clippy::string_slice)]
fn stream_file_multiline<R: Read>(
    reader: R,
    path: &Path,
    regex: &Regex,
    options: &QueryOptions,
    is_compressed: bool,
    stats: &mut StreamStats,
) -> Result<Vec<Match>> {
    let mut buf_reader = BufReader::new(reader);

    if !is_compressed {
        let buffer = buf_reader.fill_buf()?;
        let is_bin = is_binary(buffer);
        if is_bin && !options.binary {
            return Ok(vec![]);
        }
    }

    let mut content = String::new();
    buf_reader.read_to_string(&mut content)?;

    let mut matches = Vec::new();
    let line_offsets: Vec<usize> = content.lines().fold(Vec::new(), |mut acc, line| {
        let offset = if acc.is_empty() {
            0
        } else {
            acc.last().copied().unwrap_or(0) + line.len() + 1
        };
        acc.push(offset);
        acc
    });

    for m in regex.find_iter(&content) {
        let match_start = m.start();
        let line_number = u32::try_from(
            line_offsets
                .binary_search(&match_start)
                .unwrap_or_else(|i| i),
        )
        .unwrap_or(u32::MAX)
            + 1;

        let ln_idx = usize::try_from(line_number).unwrap_or(0);
        let line_start = line_offsets
            .get(ln_idx.saturating_sub(1))
            .copied()
            .unwrap_or(0);
        let line_end = line_offsets
            .get(ln_idx)
            .map_or(content.len(), |&off| off - 1);
        let line_content = content[line_start..line_end].trim_end().to_string();

        let col = u32::try_from(match_start - line_start + 1).unwrap_or(u32::MAX);

        matches.push(Match {
            file_path: path.to_path_buf(),
            line_number,
            col,
            line_content: if options.count_only {
                String::new()
            } else {
                line_content
            },
            byte_offset: u64::try_from(match_start).unwrap_or(u64::MAX),
            context_before: vec![],
            context_after: vec![],
            is_binary: false,
        });

        if options.max_results > 0 && matches.len() >= options.max_results {
            break;
        }
    }

    stats.bytes_read = content.len() as u64;
    stats.lines_read = line_offsets.len() as u32;
    stats.matches_found = matches.len() as u32;
    Ok(matches)
}

/// Search a single byte-range window, accumulating matches.
///
/// Returns `true` if `options.max_results` was reached (caller should
/// stop early), `false` otherwise.
// Return type must be Result for consistency with stream_file callers
#[allow(clippy::unnecessary_wraps)]
fn search_window(
    mmap_data: &[u8],
    path: &Path,
    regex: &Regex,
    options: &QueryOptions,
    win_start: usize,
    win_end: usize,
    matches: &mut Vec<Match>,
    seen_offsets: &mut HashSet<u64>,
) -> bool {
    let s = win_start.min(mmap_data.len());
    let e = win_end.min(mmap_data.len());
    if s >= e {
        return false;
    }

    let window = mmap_data.get(s..e).unwrap_or_default();

    let line_start = mmap_data
        .get(..s)
        .and_then(|before| before.iter().rposition(|&b| b == b'\n'))
        .map_or(0, |pos| pos + 1);

    let line_offsets =
        compute_line_offsets(mmap_data.get(line_start..e).unwrap_or_default(), line_start);

    let window_str = String::from_utf8_lossy(window);
    let is_lossy = matches!(window_str, std::borrow::Cow::Owned(_));

    for m in regex.find_iter(&window_str) {
        let abs_byte_start = if is_lossy {
            s + count_bytes_before_offset(window, m.start())
        } else {
            s + m.start()
        };

        if !seen_offsets.insert(u64::try_from(abs_byte_start).unwrap_or(u64::MAX)) {
            continue;
        }

        let line_number = match line_offsets.binary_search(&abs_byte_start) {
            // Exact match on a line-start offset → that line number.
            Ok(i) => u32::try_from(i).unwrap_or(u32::MAX) + 1,
            // `Err(i)` means the byte falls between line_offsets[i-1] and
            // line_offsets[i] (or after the last entry).  This is line `i`
            // (1-based), NOT `i + 1` — the old formula was wrong for files
            // without a trailing newline.
            Err(i) => u32::try_from(i).unwrap_or(u32::MAX),
        };

        let ln_idx = usize::try_from(line_number).unwrap_or(0);
        let line_start_off = line_offsets
            .get(ln_idx.saturating_sub(1))
            .copied()
            .unwrap_or(0);
        let line_end_off = line_offsets.get(ln_idx).map_or(mmap_data.len(), |&off| off);

        let line_bytes = trim_line_end(
            mmap_data
                .get(line_start_off..line_end_off)
                .unwrap_or_default(),
        );
        let line_str = String::from_utf8_lossy(line_bytes).to_string();

        let col = u32::try_from(abs_byte_start - line_start_off + 1).unwrap_or(u32::MAX);

        let (context_before, context_after) = if options.context_lines > 0 {
            let before = collect_context_before(
                &line_offsets,
                mmap_data,
                line_number,
                options.context_lines,
            );
            let after =
                collect_context_after(&line_offsets, mmap_data, line_number, options.context_lines);
            (before, after)
        } else {
            (vec![], vec![])
        };

        let new_match = Match {
            file_path: path.to_path_buf(),
            line_number,
            col,
            line_content: if options.count_only {
                String::new()
            } else {
                line_str
            },
            byte_offset: u64::try_from(abs_byte_start).unwrap_or(u64::MAX),
            context_before,
            context_after,
            is_binary: false,
        };

        matches.push(new_match);

        if options.max_results > 0 && matches.len() >= options.max_results {
            return true;
        }
    }

    false
}

/// Stream search over mmap data with automatic chunking for large files.
///
/// When the mmap is larger than `chunk_size_bytes` (or
/// `DEFAULT_CHUNK_SIZE` when that field is 0), the data is split into
/// overlapping chunks to keep per-chunk memory and pattern-matching cost
/// bounded. A single `seen_offsets` set across all chunks deduplicates
/// matches that appear in the overlap region.
///
/// Chunk overlap is controlled by `chunk_overlap_bytes` (or
/// `DEFAULT_CHUNK_OVERLAP` when 0).
///
/// # Errors
///
/// Returns an I/O error if reading fails (delegated from `search_window`).
pub fn stream_file_chunked(
    mmap_data: &[u8],
    path: &Path,
    regex: &Regex,
    options: &QueryOptions,
) -> Result<Vec<Match>> {
    let chunk_size = if options.chunk_size_bytes > 0 {
        options.chunk_size_bytes
    } else {
        DEFAULT_CHUNK_SIZE
    };
    let chunk_overlap = if options.chunk_overlap_bytes > 0 {
        options.chunk_overlap_bytes
    } else {
        DEFAULT_CHUNK_OVERLAP
    };

    let mut matches = Vec::new();
    let mut seen_offsets: HashSet<u64> = HashSet::new();

    // Only chunk when the file is significantly larger than chunk_size.
    // Files between chunk_size and chunk_size + chunk_overlap are handled
    // with a single window, avoiding a tiny second chunk.
    if mmap_data.len() <= chunk_size.saturating_add(chunk_overlap) {
        search_window(
            mmap_data,
            path,
            regex,
            options,
            0,
            mmap_data.len(),
            &mut matches,
            &mut seen_offsets,
        );
        return Ok(matches);
    }

    let mut start = 0usize;
    while start < mmap_data.len() {
        let mut end = (start + chunk_size).min(mmap_data.len());
        // Align chunk end to the next newline so context lines
        // near the boundary are not truncated in the middle.
        if end < mmap_data.len() {
            let remaining = &mmap_data[end..];
            if let Some(nl_pos) = remaining.iter().position(|&b| b == b'\n') {
                end += nl_pos + 1; // include the newline itself
            } else {
                end = mmap_data.len(); // no newline found → rest of file
            }
        }
        if search_window(
            mmap_data,
            path,
            regex,
            options,
            start,
            end,
            &mut matches,
            &mut seen_offsets,
        ) {
            break;
        }
        if end >= mmap_data.len() {
            break;
        }
        start = end.saturating_sub(chunk_overlap);
    }

    Ok(matches)
}

fn count_bytes_before_offset(data: &[u8], str_offset: usize) -> usize {
    let s = String::from_utf8_lossy(data);
    let mut byte_pos = 0;
    for (char_pos, c) in s.chars().enumerate() {
        if char_pos >= str_offset {
            break;
        }
        byte_pos += c.len_utf8();
    }
    byte_pos
}

fn compute_line_offsets(data: &[u8], base_offset: usize) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(64);
    offsets.push(base_offset);
    for (i, &b) in data.iter().enumerate() {
        if b == b'\n' {
            let next = base_offset + i + 1;
            if next < data.len() + base_offset {
                offsets.push(next);
            }
        }
    }
    offsets
}

fn trim_line_end(data: &[u8]) -> &[u8] {
    let end = data
        .iter()
        .rposition(|&b| b != b'\n' && b != b'\r')
        .map_or(0, |i| i + 1);
    data.get(..end).unwrap_or_default()
}

fn collect_context_before(
    line_offsets: &[usize],
    data: &[u8],
    line_number: u32,
    context_lines: usize,
) -> Vec<String> {
    let match_idx = usize::try_from(line_number).unwrap_or(0).saturating_sub(1);
    let start = match_idx.saturating_sub(context_lines);
    let end = match_idx;
    let mut ctx = Vec::with_capacity(end.saturating_sub(start));
    for i in start..end {
        let lo = line_offsets.get(i).copied().unwrap_or(0);
        let hi = line_offsets.get(i + 1).copied().unwrap_or(data.len());
        let line_bytes = trim_line_end(data.get(lo..hi).unwrap_or_default());
        ctx.push(String::from_utf8_lossy(line_bytes).to_string());
    }
    ctx
}

fn collect_context_after(
    line_offsets: &[usize],
    data: &[u8],
    line_number: u32,
    context_lines: usize,
) -> Vec<String> {
    let after_start = usize::try_from(line_number).unwrap_or(0);
    let end = (after_start + context_lines).min(line_offsets.len().saturating_sub(1));
    let mut ctx = Vec::with_capacity(end.saturating_sub(after_start));
    for i in after_start..end {
        let lo = line_offsets.get(i).copied().unwrap_or(0);
        let hi = line_offsets.get(i + 1).copied().unwrap_or(data.len());
        let line_bytes = trim_line_end(data.get(lo..hi).unwrap_or_default());
        ctx.push(String::from_utf8_lossy(line_bytes).to_string());
    }
    ctx
}
