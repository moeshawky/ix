use crate::error::Result;
use crate::executor::{Match, QueryOptions};
use crate::format::is_binary;
use regex::Regex;
use std::borrow::Cow;
use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

/// Default per-chunk size for large-file chunked streaming: `16 MiB`.
const DEFAULT_CHUNK_SIZE: usize = 16 * 1024 * 1024;

/// Default overlap between adjacent chunks: `1 MiB`.
/// Ensures matches that straddle a chunk boundary are still found.
const DEFAULT_CHUNK_OVERLAP: usize = DEFAULT_CHUNK_SIZE / 16;

/// Optional per-line and total byte caps for bounded streaming callers (e.g. archives).
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct StreamLimits {
    pub max_line_bytes: Option<usize>,
    pub max_total_bytes: Option<usize>,
    pub line_limit_label: Option<&'static str>,
    pub total_limit_label: Option<&'static str>,
}

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
/// Screens the initial stream bytes with `is_binary`; skips binary streams
/// unless `options.binary` is set (marking emitted matches with `is_binary`),
/// handles non-UTF-8 content lossily, and writes accumulated statistics to
/// `stats` (lines read, bytes consumed, matches found).
///
/// # Errors
///
/// Returns an I/O error if reading fails, or [`crate::error::Error::Config`]
/// if multiline input exceeds the `100 MiB` bound.
pub fn stream_file<R: Read>(
    reader: R,
    path: &Path,
    regex: &Regex,
    options: &QueryOptions,
    _is_compressed: bool,
    stats: &mut StreamStats,
) -> Result<Vec<Match>> {
    stream_file_bounded(reader, path, regex, options, StreamLimits::default(), stats)
}

/// Stream search through a reader with optional per-line and total byte bounds.
///
/// # Errors
///
/// Returns an error if reading fails or if `limits` are exceeded.
pub(crate) fn stream_file_bounded<R: Read>(
    reader: R,
    path: &Path,
    regex: &Regex,
    options: &QueryOptions,
    limits: StreamLimits,
    stats: &mut StreamStats,
) -> Result<Vec<Match>> {
    if options.multiline {
        return stream_file_multiline(reader, path, regex, options, limits, stats);
    }

    let mut buf_reader = BufReader::new(reader);
    let StreamHeaderDecision::Search {
        is_binary: file_is_binary,
    } = inspect_stream_header(&mut buf_reader, options.binary)?
    else {
        return Ok(vec![]);
    };

    let mut line_state = LineStreamState {
        is_binary: file_is_binary,
        ..Default::default()
    };
    let mut raw_line = Vec::new();

    loop {
        raw_line.clear();
        let n = read_bounded_line(
            &mut buf_reader,
            &mut raw_line,
            limits.max_line_bytes,
            limits.line_limit_label,
        )?;
        if n == 0 {
            break;
        }
        line_state.total_bytes = line_state.total_bytes.saturating_add(n);
        check_total_limit(line_state.total_bytes, limits)?;
        if process_stream_line(&raw_line, path, regex, options, &mut line_state) {
            break;
        }
    }

    line_state.matches.extend(line_state.pending_matches);
    if options.max_results > 0 && line_state.matches.len() > options.max_results {
        line_state.matches.truncate(options.max_results);
    }
    stats.lines_read = line_state.line_number;
    stats.bytes_read = line_state.byte_offset;
    stats.matches_found = u32::try_from(line_state.matches.len()).unwrap_or(u32::MAX);
    Ok(line_state.matches)
}

#[derive(Default)]
struct LineStreamState {
    matches: Vec<Match>,
    pending_matches: Vec<Match>,
    context_before: VecDeque<String>,
    line_number: u32,
    byte_offset: u64,
    total_bytes: usize,
    is_binary: bool,
}

enum StreamHeaderDecision {
    Skip,
    Search { is_binary: bool },
}

fn check_total_limit(total_bytes: usize, limits: StreamLimits) -> Result<()> {
    if let Some(max_total) = limits.max_total_bytes
        && total_bytes > max_total
    {
        let label = limits
            .total_limit_label
            .unwrap_or("input exceeds size limit");
        return Err(crate::error::Error::Config(format!(
            "{label} ({max_total} bytes)"
        )));
    }
    Ok(())
}

fn process_stream_line(
    raw_line: &[u8],
    path: &Path,
    regex: &Regex,
    options: &QueryOptions,
    state: &mut LineStreamState,
) -> bool {
    state.line_number = state.line_number.saturating_add(1);
    let line_len = u64::try_from(raw_line.len()).unwrap_or(u64::MAX);
    let line_cow = String::from_utf8_lossy(raw_line);
    let trimmed_line = line_cow.trim_end().to_string();

    advance_pending_context(
        &mut state.pending_matches,
        &mut state.matches,
        &trimmed_line,
        options.context_lines,
    );

    let mut reached_limit = false;
    if let Some(m) = regex.find(&line_cow) {
        let raw_match_start = if matches!(line_cow, Cow::Owned(_)) {
            LossyOffsetMapper::new(raw_line).map(m.start())
        } else {
            m.start()
        };
        let new_match = Match {
            file_path: path.to_path_buf(),
            line_number: state.line_number,
            col: u32::try_from(raw_match_start.saturating_add(1)).unwrap_or(u32::MAX),
            line_content: if options.count_only {
                String::new()
            } else {
                trimmed_line.clone()
            },
            byte_offset: state
                .byte_offset
                .saturating_add(u64::try_from(raw_match_start).unwrap_or(u64::MAX)),
            context_before: state.context_before.iter().cloned().collect(),
            context_after: vec![],
            is_binary: state.is_binary,
        };

        if options.context_lines > 0 {
            state.pending_matches.push(new_match);
        } else {
            state.matches.push(new_match);
        }

        if options.max_results > 0
            && (state.matches.len() + state.pending_matches.len()) >= options.max_results
            && (state.pending_matches.is_empty() || state.matches.len() >= options.max_results)
        {
            reached_limit = true;
        }
    }

    if !reached_limit {
        push_context_before(
            &mut state.context_before,
            &trimmed_line,
            options.context_lines,
        );
    }
    state.byte_offset = state.byte_offset.saturating_add(line_len);
    reached_limit
}

fn inspect_stream_header<R: Read>(
    buf_reader: &mut BufReader<R>,
    allow_binary: bool,
) -> Result<StreamHeaderDecision> {
    let buffer = buf_reader.fill_buf()?;
    if buffer.is_empty() {
        return Ok(StreamHeaderDecision::Skip);
    }
    let file_is_binary = is_binary(buffer);
    if !allow_binary && file_is_binary {
        return Ok(StreamHeaderDecision::Skip);
    }
    Ok(StreamHeaderDecision::Search {
        is_binary: file_is_binary,
    })
}

fn read_bounded_line<R: Read>(
    buf_reader: &mut BufReader<R>,
    raw_line: &mut Vec<u8>,
    max_line_bytes: Option<usize>,
    line_limit_label: Option<&'static str>,
) -> Result<usize> {
    if let Some(max_line) = max_line_bytes {
        let cap = u64::try_from(max_line)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let read = buf_reader.by_ref().take(cap).read_until(b'\n', raw_line)?;
        if read > max_line {
            let label = line_limit_label.unwrap_or("line exceeds size limit");
            return Err(crate::error::Error::Config(format!(
                "{label} ({max_line} bytes)"
            )));
        }
        Ok(read)
    } else {
        Ok(buf_reader.read_until(b'\n', raw_line)?)
    }
}

fn advance_pending_context(
    pending_matches: &mut Vec<Match>,
    matches: &mut Vec<Match>,
    trimmed_line: &str,
    context_lines: usize,
) {
    if pending_matches.is_empty() {
        return;
    }
    for m in &mut *pending_matches {
        if m.context_after.len() < context_lines {
            m.context_after.push(trimmed_line.to_string());
        }
    }
    let taken = std::mem::take(pending_matches);
    let (completed, still_pending): (Vec<_>, Vec<_>) = taken
        .into_iter()
        .partition(|m| m.context_after.len() >= context_lines);
    matches.extend(completed);
    *pending_matches = still_pending;
}

fn push_context_before(
    context_before: &mut VecDeque<String>,
    trimmed_line: &str,
    context_lines: usize,
) {
    if context_lines == 0 {
        return;
    }
    if context_before.len() == context_lines {
        if let Some(mut old_line) = context_before.pop_front() {
            old_line.clear();
            old_line.push_str(trimmed_line);
            context_before.push_back(old_line);
        }
    } else {
        context_before.push_back(trimmed_line.to_string());
    }
}

fn stream_file_multiline<R: Read>(
    reader: R,
    path: &Path,
    regex: &Regex,
    options: &QueryOptions,
    limits: StreamLimits,
    stats: &mut StreamStats,
) -> Result<Vec<Match>> {
    // Bound the read to avoid OOM on huge inputs (decompression/zip-bomb
    // parity with decompress.rs CountingReader). Unbounded read_to_end is a
    // DoS vector for --multiline.
    const MAX_MULTILINE_BYTES: usize = 100 * 1024 * 1024; // 100 MiB

    let max_bytes = limits
        .max_total_bytes
        .unwrap_or(MAX_MULTILINE_BYTES)
        .min(MAX_MULTILINE_BYTES);

    let mut buf_reader = BufReader::new(reader);
    let StreamHeaderDecision::Search {
        is_binary: header_is_binary,
    } = inspect_stream_header(&mut buf_reader, options.binary)?
    else {
        return Ok(vec![]);
    };

    let mut raw = Vec::new();
    let cap = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let bytes_read = buf_reader.take(cap).read_to_end(&mut raw)?;
    if bytes_read > max_bytes {
        let label = limits
            .total_limit_label
            .unwrap_or("multiline input exceeds size limit");
        return Err(crate::error::Error::Config(format!(
            "{label} ({max_bytes} bytes)"
        )));
    }
    if raw.is_empty() {
        return Ok(vec![]);
    }
    let file_is_binary = header_is_binary || is_binary(&raw);
    if !options.binary && file_is_binary {
        return Ok(vec![]);
    }

    let line_offsets = compute_line_offsets(&raw, 0);
    let content = String::from_utf8_lossy(&raw);
    let is_lossy = matches!(content, Cow::Owned(_));
    let mut mapper = is_lossy.then(|| LossyOffsetMapper::new(&raw));

    let mut matches = Vec::new();
    for m in regex.find_iter(&content) {
        let match_start = mapper
            .as_mut()
            .map_or_else(|| m.start(), |map| map.map(m.start()));
        let line_number = line_number_for_offset(&line_offsets, match_start);
        let ln_idx = usize::try_from(line_number).unwrap_or(0);
        let line_start = line_offsets
            .get(ln_idx.saturating_sub(1))
            .copied()
            .unwrap_or(0);
        let line_end = line_offsets.get(ln_idx).copied().unwrap_or(raw.len());
        let line_bytes = trim_line_end(raw.get(line_start..line_end).unwrap_or_default());
        let line_content = String::from_utf8_lossy(line_bytes).to_string();
        let col = u32::try_from(match_start.saturating_sub(line_start).saturating_add(1))
            .unwrap_or(u32::MAX);

        let (context_before, context_after) = if options.context_lines > 0 {
            (
                collect_context_before(&line_offsets, &raw, line_number, options.context_lines),
                collect_context_after(&line_offsets, &raw, line_number, options.context_lines),
            )
        } else {
            (vec![], vec![])
        };

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
            context_before,
            context_after,
            is_binary: file_is_binary,
        });

        if options.max_results > 0 && matches.len() >= options.max_results {
            break;
        }
    }

    stats.bytes_read = u64::try_from(raw.len()).unwrap_or(u64::MAX);
    stats.lines_read = u32::try_from(line_offsets.len()).unwrap_or(u32::MAX);
    stats.matches_found = u32::try_from(matches.len()).unwrap_or(u32::MAX);
    Ok(matches)
}

#[derive(Clone, Copy)]
struct WindowBounds {
    win_start: usize,
    win_end: usize,
    ctx_start: usize,
    ctx_end: usize,
    base_line_index: usize,
    is_binary: bool,
}

/// Search a single byte-range window, accumulating matches.
///
/// Returns `true` if `options.max_results` was reached (caller should
/// stop early), `false` otherwise.
fn search_window(
    mmap_data: &[u8],
    path: &Path,
    regex: &Regex,
    options: &QueryOptions,
    bounds: WindowBounds,
    matches: &mut Vec<Match>,
    seen_offsets: &mut HashSet<u64>,
) -> bool {
    let s = bounds.win_start.min(mmap_data.len());
    let e = bounds.win_end.min(mmap_data.len());
    if s >= e {
        return false;
    }

    let window = mmap_data.get(s..e).unwrap_or_default();
    let bounded_data = mmap_data.get(..bounds.ctx_end).unwrap_or(mmap_data);
    let line_offsets = compute_line_offsets(
        bounded_data
            .get(bounds.ctx_start..bounds.ctx_end)
            .unwrap_or_default(),
        bounds.ctx_start,
    );

    let window_str = String::from_utf8_lossy(window);
    let is_lossy = matches!(window_str, Cow::Owned(_));
    let mut mapper = is_lossy.then(|| LossyOffsetMapper::new(window));

    for m in regex.find_iter(&window_str) {
        let local_raw_start = mapper
            .as_mut()
            .map_or_else(|| m.start(), |map| map.map(m.start()));
        let abs_byte_start = s.saturating_add(local_raw_start);

        if !seen_offsets.insert(u64::try_from(abs_byte_start).unwrap_or(u64::MAX)) {
            continue;
        }

        let local_line_number = line_number_for_offset(&line_offsets, abs_byte_start);
        let new_match = build_window_match(
            path,
            bounded_data,
            &line_offsets,
            bounds,
            local_line_number,
            abs_byte_start,
            options,
        );
        matches.push(new_match);

        if options.max_results > 0 && matches.len() >= options.max_results {
            return true;
        }
    }

    false
}

fn build_window_match(
    path: &Path,
    bounded_data: &[u8],
    line_offsets: &[usize],
    bounds: WindowBounds,
    local_line_number: u32,
    abs_byte_start: usize,
    options: &QueryOptions,
) -> Match {
    let ln_idx = usize::try_from(local_line_number).unwrap_or(0);
    let line_start_off = line_offsets
        .get(ln_idx.saturating_sub(1))
        .copied()
        .unwrap_or(bounds.ctx_start);
    let line_end_off = line_offsets.get(ln_idx).copied().unwrap_or(bounds.ctx_end);

    let line_bytes = trim_line_end(
        bounded_data
            .get(line_start_off..line_end_off)
            .unwrap_or_default(),
    );
    let line_str = String::from_utf8_lossy(line_bytes).to_string();
    let col = u32::try_from(
        abs_byte_start
            .saturating_sub(line_start_off)
            .saturating_add(1),
    )
    .unwrap_or(u32::MAX);

    let (context_before, context_after) = if options.context_lines > 0 {
        (
            collect_context_before(
                line_offsets,
                bounded_data,
                local_line_number,
                options.context_lines,
            ),
            collect_context_after(
                line_offsets,
                bounded_data,
                local_line_number,
                options.context_lines,
            ),
        )
    } else {
        (vec![], vec![])
    };

    let file_line_number = u32::try_from(bounds.base_line_index)
        .unwrap_or(u32::MAX)
        .saturating_add(local_line_number);

    Match {
        file_path: path.to_path_buf(),
        line_number: file_line_number,
        col,
        line_content: if options.count_only {
            String::new()
        } else {
            line_str
        },
        byte_offset: u64::try_from(abs_byte_start).unwrap_or(u64::MAX),
        context_before,
        context_after,
        is_binary: bounds.is_binary,
    }
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
    let file_is_binary = is_binary(mmap_data);
    if !options.binary && file_is_binary {
        return Ok(vec![]);
    }

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
    // Guard against an infinite loop: if overlap >= chunk_size, every
    // iteration re-starts at or before its previous start and never
    // advances. Clamp overlap to chunk_size-1 so progress is guaranteed.
    let chunk_overlap = if chunk_size == 0 {
        0
    } else {
        chunk_overlap.min(chunk_size.saturating_sub(1))
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
            WindowBounds {
                win_start: 0,
                win_end: mmap_data.len(),
                ctx_start: 0,
                ctx_end: mmap_data.len(),
                base_line_index: 0,
                is_binary: file_is_binary,
            },
            &mut matches,
            &mut seen_offsets,
        );
        return Ok(matches);
    }

    let mut start = 0usize;
    let mut counted_pos = 0usize;
    let mut counted_newlines = 0usize;

    while start < mmap_data.len() {
        let mut end = start.saturating_add(chunk_size).min(mmap_data.len());
        // Align chunk end to the next newline so context lines
        // near the boundary are not truncated in the middle.
        if end < mmap_data.len() {
            if let Some(nl_pos) = mmap_data
                .get(end..)
                .and_then(|rem| rem.iter().position(|&b| b == b'\n'))
            {
                end += nl_pos + 1;
            } else {
                end = mmap_data.len();
            }
        }

        let (ctx_start, ctx_end) =
            context_window_bounds(mmap_data, start, end, options.context_lines);
        let base_line_index = advance_newline_count(
            mmap_data,
            &mut counted_pos,
            &mut counted_newlines,
            ctx_start,
        );

        if search_window(
            mmap_data,
            path,
            regex,
            options,
            WindowBounds {
                win_start: start,
                win_end: end,
                ctx_start,
                ctx_end,
                base_line_index,
                is_binary: file_is_binary,
            },
            &mut matches,
            &mut seen_offsets,
        ) {
            break;
        }
        if end >= mmap_data.len() {
            break;
        }

        start = align_next_chunk_start(mmap_data, start, end, chunk_overlap);
    }

    Ok(matches)
}

fn context_window_bounds(
    mmap_data: &[u8],
    win_start: usize,
    win_end: usize,
    context_lines: usize,
) -> (usize, usize) {
    let mut ctx_start = mmap_data
        .get(..win_start)
        .and_then(|before| before.iter().rposition(|&b| b == b'\n'))
        .map_or(0, |pos| pos + 1);
    for _ in 0..context_lines {
        if ctx_start == 0 {
            break;
        }
        let search_end = ctx_start.saturating_sub(1);
        ctx_start = mmap_data
            .get(..search_end)
            .and_then(|before| before.iter().rposition(|&b| b == b'\n'))
            .map_or(0, |pos| pos + 1);
    }

    let mut ctx_end = win_end.min(mmap_data.len());
    if ctx_end > 0
        && ctx_end < mmap_data.len()
        && mmap_data.get(ctx_end.saturating_sub(1)).copied() != Some(b'\n')
    {
        if let Some(rel) = mmap_data
            .get(ctx_end..)
            .and_then(|rem| rem.iter().position(|&b| b == b'\n'))
        {
            ctx_end += rel + 1;
        } else {
            ctx_end = mmap_data.len();
        }
    }
    for _ in 0..context_lines {
        if ctx_end >= mmap_data.len() {
            break;
        }
        if let Some(rel) = mmap_data
            .get(ctx_end..)
            .and_then(|rem| rem.iter().position(|&b| b == b'\n'))
        {
            ctx_end += rel + 1;
        } else {
            ctx_end = mmap_data.len();
        }
    }

    (ctx_start, ctx_end)
}

fn advance_newline_count(
    mmap_data: &[u8],
    counted_pos: &mut usize,
    counted_newlines: &mut usize,
    target_pos: usize,
) -> usize {
    if target_pos >= *counted_pos {
        let delta = mmap_data
            .get(*counted_pos..target_pos)
            .unwrap_or_default()
            .iter()
            .fold(0usize, |acc, &b| acc + usize::from(b == b'\n'));
        *counted_newlines += delta;
        *counted_pos = target_pos;
        *counted_newlines
    } else {
        let count = mmap_data
            .get(..target_pos)
            .unwrap_or_default()
            .iter()
            .fold(0usize, |acc, &b| acc + usize::from(b == b'\n'));
        *counted_pos = target_pos;
        *counted_newlines = count;
        count
    }
}

fn align_next_chunk_start(
    mmap_data: &[u8],
    prev_start: usize,
    end: usize,
    chunk_overlap: usize,
) -> usize {
    let mut next_start = end.saturating_sub(chunk_overlap);
    let mut back_steps = 0u8;
    while back_steps < 3
        && next_start > prev_start.saturating_add(1)
        && mmap_data
            .get(next_start)
            .is_some_and(|&b| (b & 0xC0) == 0x80)
    {
        next_start -= 1;
        back_steps += 1;
    }
    if next_start <= prev_start {
        next_start = prev_start.saturating_add(1);
        let mut fwd_steps = 0u8;
        while fwd_steps < 3
            && next_start < end
            && mmap_data
                .get(next_start)
                .is_some_and(|&b| (b & 0xC0) == 0x80)
        {
            next_start += 1;
            fwd_steps += 1;
        }
    }
    next_start
}

/// Single-pass cursor mapping byte offsets in `String::from_utf8_lossy` output
/// back to raw byte offsets in the underlying slice.
struct LossyOffsetMapper<'a> {
    chunks: std::str::Utf8Chunks<'a>,
    current_valid: &'a str,
    current_invalid_len: usize,
    lossy_pos: usize,
    raw_pos: usize,
}

impl<'a> LossyOffsetMapper<'a> {
    fn new(raw: &'a [u8]) -> Self {
        Self {
            chunks: raw.utf8_chunks(),
            current_valid: "",
            current_invalid_len: 0,
            lossy_pos: 0,
            raw_pos: 0,
        }
    }

    fn map(&mut self, target_lossy: usize) -> usize {
        loop {
            let valid_end = self.lossy_pos + self.current_valid.len();
            if target_lossy <= valid_end {
                return self.raw_pos + (target_lossy - self.lossy_pos);
            }
            self.lossy_pos = valid_end;
            self.raw_pos += self.current_valid.len();
            self.current_valid = "";

            if self.current_invalid_len > 0 {
                let invalid_end = self.lossy_pos + 3; // '\u{FFFD}'.len_utf8()
                if target_lossy < invalid_end {
                    return self.raw_pos;
                }
                self.lossy_pos = invalid_end;
                self.raw_pos += self.current_invalid_len;
                self.current_invalid_len = 0;
                if target_lossy == self.lossy_pos {
                    return self.raw_pos;
                }
            }

            match self.chunks.next() {
                Some(chunk) => {
                    self.current_valid = chunk.valid();
                    self.current_invalid_len = chunk.invalid().len();
                }
                None => return self.raw_pos,
            }
        }
    }
}

fn line_number_for_offset(line_offsets: &[usize], abs_byte_start: usize) -> u32 {
    match line_offsets.binary_search(&abs_byte_start) {
        Ok(i) => u32::try_from(i).unwrap_or(u32::MAX).saturating_add(1),
        Err(i) => u32::try_from(i).unwrap_or(u32::MAX),
    }
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
    let end = after_start
        .saturating_add(context_lines)
        .min(line_offsets.len());
    let mut ctx = Vec::with_capacity(end.saturating_sub(after_start));
    for i in after_start..end {
        let lo = line_offsets.get(i).copied().unwrap_or(0);
        let hi = line_offsets.get(i + 1).copied().unwrap_or(data.len());
        let line_bytes = trim_line_end(data.get(lo..hi).unwrap_or_default());
        ctx.push(String::from_utf8_lossy(line_bytes).to_string());
    }
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::QueryOptions;
    use regex::Regex;

    #[test]
    fn chunked_overlap_no_infinite_loop() {
        // Regression: chunk_overlap >= chunk_size used to infinite-loop.
        // With the clamp, this must terminate quickly.
        let data = vec![b'a'; 10_000];
        let regex = Regex::new("a").unwrap();
        let opts = QueryOptions {
            chunk_size_bytes: 50,
            chunk_overlap_bytes: 50, // overlap == size (was infinite loop)
            ..QueryOptions::default()
        };
        // stream_file_chunked is pub; exercise the clamp path through a
        // temp file.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        std::fs::write(&path, &data).unwrap();
        // Use the public streaming entry that takes a path? stream_file_chunked
        // takes mmap_data; replicate via reading the file into a Vec and
        // calling the function directly (it's pub).
        let mmap_data = std::fs::read(&path).unwrap();
        let res = stream_file_chunked(&mmap_data, &path, &regex, &opts);
        assert!(res.is_ok(), "chunked search should succeed: {res:?}");
        // If we reach here, no infinite loop.
    }

    #[test]
    fn multiline_unequal_lines_crlf_and_utf8_offsets() {
        // U3-F01: unequal line lengths, CRLF, and multi-byte UTF-8 (U+1F980).
        let input = "\u{1f980}a\r\nhello_world\r\nmatch_here\r\n";
        let regex = Regex::new(r"(?s)hello_world\r\nmatch_here").unwrap();
        let opts = QueryOptions {
            multiline: true,
            ..Default::default()
        };
        let mut stats = StreamStats::default();
        let matches = stream_file(
            std::io::Cursor::new(input.as_bytes()),
            Path::new("utf8_crlf.txt"),
            &regex,
            &opts,
            false,
            &mut stats,
        )
        .unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line_number, 2);
        assert_eq!(matches[0].col, 1);
        assert_eq!(matches[0].line_content, "hello_world");
        assert_eq!(stats.lines_read, 3);
    }

    #[test]
    fn chunked_cumulative_line_numbers_and_cross_chunk_context() {
        // U3-F02 + U3-F03: cumulative line_number across chunks and context across boundary.
        let mut content = String::new();
        for i in 1..=20 {
            use std::fmt::Write as _;
            let _ = writeln!(content, "line_{i:02}_padding_data");
        }
        let target_line = 12usize;
        let content = content.replace("line_12_padding_data", "line_12_TARGET_data");
        let regex = Regex::new("TARGET").unwrap();
        let opts = QueryOptions {
            context_lines: 2,
            chunk_size_bytes: 120,
            chunk_overlap_bytes: 30,
            ..Default::default()
        };
        let matches =
            stream_file_chunked(content.as_bytes(), Path::new("chunked.txt"), &regex, &opts)
                .unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(
            usize::try_from(matches[0].line_number).unwrap(),
            target_line
        );
        assert_eq!(
            matches[0].context_before,
            vec![
                "line_10_padding_data".to_string(),
                "line_11_padding_data".to_string(),
            ]
        );
        assert_eq!(
            matches[0].context_after,
            vec![
                "line_13_padding_data".to_string(),
                "line_14_padding_data".to_string(),
            ]
        );
    }

    #[test]
    fn chunked_collect_context_after_includes_final_line() {
        // U3-F03: match on line 1 of a 2-line file includes line 2 in context_after.
        let data = b"needle\nfinal_line\n";
        let regex = Regex::new("needle").unwrap();
        let opts = QueryOptions {
            context_lines: 2,
            ..Default::default()
        };
        let matches = stream_file_chunked(data, Path::new("two.txt"), &regex, &opts).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].context_after, vec!["final_line".to_string()]);
    }

    #[test]
    fn chunked_utf8_split_and_invalid_bytes_exact_offsets() {
        // U3-F04: multi-byte UTF-8 near chunk overlap + invalid UTF-8 bytes before match.
        let mut raw = Vec::new();
        raw.extend_from_slice(b"prefix_\xFF\xFE_");
        raw.extend_from_slice("\u{1f980}".repeat(20).as_bytes());
        raw.extend_from_slice(b"\nsecond_line_\xFF_needle\n");
        let expected_offset = raw.windows(6).position(|w| w == b"needle").unwrap();

        let regex = Regex::new("needle").unwrap();
        let opts = QueryOptions {
            chunk_size_bytes: 40,
            chunk_overlap_bytes: 10,
            ..Default::default()
        };
        let matches = stream_file_chunked(&raw, Path::new("lossy.bin"), &regex, &opts).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line_number, 2);
        assert_eq!(
            usize::try_from(matches[0].byte_offset).unwrap(),
            expected_offset
        );
    }

    #[test]
    fn stream_file_screens_binary_when_compressed_and_handles_non_utf8_and_max_results() {
        // U3-F05: is_compressed=true still screens binary unless options.binary=true.
        let bin = vec![0u8; 512];
        let regex = Regex::new("x").unwrap();
        let mut stats = StreamStats::default();
        let res = stream_file(
            std::io::Cursor::new(bin),
            Path::new("wrapped.gz"),
            &regex,
            &QueryOptions::default(),
            true,
            &mut stats,
        )
        .unwrap();
        assert!(res.is_empty());

        // U3-F06: non-UTF-8 byte past 8 KiB + max_results truncation with context_lines > 0.
        let mut payload = vec![b'a'; 9000];
        payload.push(b'\n');
        payload.extend_from_slice(b"ctx0\nmatch_one \xFF\nmatch_two\nmatch_three\nctx_end\n");
        let opts = QueryOptions {
            context_lines: 1,
            max_results: 2,
            ..Default::default()
        };
        let mut stats = StreamStats::default();
        let matches = stream_file(
            std::io::Cursor::new(payload),
            Path::new("latin1.txt"),
            &Regex::new("match_").unwrap(),
            &opts,
            false,
            &mut stats,
        )
        .unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(stats.matches_found, 2);
    }

    fn make_binary_test_payload() -> Vec<u8> {
        let mut payload = vec![0u8; 256];
        payload.extend_from_slice(b"alpha_needle\nbeta_needle\n");
        payload.extend_from_slice(&[0u8; 231]);
        assert!(is_binary(&payload));
        payload
    }

    fn assert_all_modes_binary_flag(payload: &[u8], expected_is_binary: bool) {
        let regex = Regex::new("_needle").unwrap();
        let mut stats = StreamStats::default();

        let bounded = stream_file(
            std::io::Cursor::new(payload),
            Path::new("sample"),
            &regex,
            &QueryOptions {
                binary: true,
                ..Default::default()
            },
            false,
            &mut stats,
        )
        .unwrap();
        assert_eq!(bounded.len(), 2);
        assert!(bounded.iter().all(|m| m.is_binary == expected_is_binary));

        let multiline = stream_file(
            std::io::Cursor::new(payload),
            Path::new("sample"),
            &regex,
            &QueryOptions {
                binary: true,
                multiline: true,
                ..Default::default()
            },
            false,
            &mut stats,
        )
        .unwrap();
        assert_eq!(multiline.len(), 2);
        assert!(multiline.iter().all(|m| m.is_binary == expected_is_binary));

        let chunked = stream_file_chunked(
            payload,
            Path::new("sample"),
            &regex,
            &QueryOptions {
                binary: true,
                chunk_size_bytes: 150,
                chunk_overlap_bytes: 20,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(chunked.len(), 2);
        assert!(chunked.iter().all(|m| m.is_binary == expected_is_binary));
    }

    #[test]
    fn binary_stream_skips_when_disallowed_across_modes() {
        let payload = make_binary_test_payload();
        let regex = Regex::new("_needle").unwrap();
        let mut stats = StreamStats::default();

        let bounded = stream_file(
            std::io::Cursor::new(&payload),
            Path::new("blob.bin"),
            &regex,
            &QueryOptions::default(),
            false,
            &mut stats,
        )
        .unwrap();
        assert!(bounded.is_empty());

        let multiline = stream_file(
            std::io::Cursor::new(&payload),
            Path::new("blob.bin"),
            &regex,
            &QueryOptions {
                multiline: true,
                ..Default::default()
            },
            false,
            &mut stats,
        )
        .unwrap();
        assert!(multiline.is_empty());

        let chunked = stream_file_chunked(
            &payload,
            Path::new("blob.bin"),
            &regex,
            &QueryOptions {
                chunk_size_bytes: 150,
                chunk_overlap_bytes: 20,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(chunked.is_empty());
    }

    #[test]
    fn binary_stream_sets_is_binary_across_all_streaming_modes() {
        let bin_payload = make_binary_test_payload();
        assert_all_modes_binary_flag(&bin_payload, true);
        assert_all_modes_binary_flag(b"alpha_needle\nbeta_needle\n", false);
    }
}
