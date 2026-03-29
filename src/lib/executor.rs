//! Query executor — search through the index and verify results.
//!
//! Handles literal searches, indexed regex, and full scans.

use crate::decompress::maybe_decompress;
use crate::error::Result;
use crate::planner::QueryPlan;
use crate::reader::{FileInfo, Reader};
use crate::trigram::Trigram;
use rayon::prelude::*;
use regex::Regex;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[derive(Debug)]
pub struct Match {
    pub file_path: PathBuf,
    pub line_number: u32,
    pub col: u32,
    pub line_content: String,
    pub byte_offset: u64,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
    pub is_binary: bool,
}

#[derive(Default, Debug)]
pub struct QueryStats {
    pub trigrams_queried: u32,
    pub posting_lists_decoded: u32,
    pub candidate_files: u32,
    pub files_verified: u32,
    pub bytes_verified: u64,
    pub total_matches: u32,
}

#[derive(Debug, Default)]
pub struct QueryOptions {
    pub count_only: bool,
    pub files_only: bool,
    pub max_results: usize,
    pub type_filter: Vec<String>,
    pub context_lines: usize,
    pub decompress: bool,
    pub threads: usize,
    pub multiline: bool,
    pub archive: bool,
    pub binary: bool,
}

pub struct Executor<'a> {
    index: &'a Reader,
}

impl<'a> Executor<'a> {
    pub fn new(index: &'a Reader) -> Self {
        Self { index }
    }

    pub fn execute(
        &self,
        plan: &QueryPlan,
        options: &QueryOptions,
    ) -> Result<(Vec<Match>, QueryStats)> {
        match plan {
            QueryPlan::Literal { pattern, trigrams } => {
                self.execute_literal(pattern, trigrams, options)
            }
            QueryPlan::RegexWithLiterals {
                regex,
                required_trigram_sets,
            } => self.execute_regex_indexed(regex, required_trigram_sets, options),
            QueryPlan::CaseInsensitive {
                regex,
                trigram_groups,
            } => self.execute_case_insensitive(regex, trigram_groups, options),
            QueryPlan::FullScan { regex } => self.execute_full_scan(regex, options),
        }
    }

    fn execute_literal(
        &self,
        pattern: &[u8],
        trigrams: &[Trigram],
        options: &QueryOptions,
    ) -> Result<(Vec<Match>, QueryStats)> {
        let mut stats = QueryStats::default();

        let mut infos = Vec::new();
        for &tri in trigrams {
            stats.trigrams_queried += 1;
            if let Some(info) = self.index.get_trigram(tri) {
                infos.push((tri, info));
            } else {
                return Ok((vec![], stats));
            }
        }

        // Sort by doc_frequency (rarest first)
        infos.sort_by_key(|(_, info)| info.doc_frequency);

        // ── Step 1: Decode rarest posting list ──
        let (_, rarest_info) = &infos[0];
        let postings = self.index.decode_postings(rarest_info)?;
        stats.posting_lists_decoded += 1;

        let mut candidates: HashSet<u32> = postings.entries.iter().map(|e| e.file_id).collect();

        // ── Step 2: Intersect with next rarest lists if candidate set is large ──
        // Only decode up to 3 lists to avoid excessive I/O
        for (_, info) in infos.iter().take(infos.len().min(3)).skip(1) {
            if candidates.len() < 100 {
                break;
            }

            let next_postings = self.index.decode_postings(info)?;
            stats.posting_lists_decoded += 1;

            let next_set: HashSet<u32> = next_postings.entries.iter().map(|e| e.file_id).collect();
            candidates.retain(|fid| next_set.contains(fid));
        }

        // ── Step 3: Filter remaining using Bloom filters ──
        for &(tri, _) in &infos[1..] {
            if candidates.is_empty() {
                break;
            }
            candidates.retain(|&fid| self.index.bloom_may_contain(fid, tri));
        }

        stats.candidate_files = candidates.len() as u32;

        let regex = Regex::new(&regex::escape(&String::from_utf8_lossy(pattern)))?;

        // Parallel verification
        let files_verified = AtomicU32::new(0);
        let bytes_verified = std::sync::atomic::AtomicU64::new(0);
        let matches_found = AtomicU32::new(0);

        let candidate_list: Vec<u32> = candidates.into_iter().collect();

        let mut all_matches: Vec<Match> = candidate_list
            .into_par_iter()
            .filter_map(|fid| {
                if options.max_results > 0 && matches_found.load(Ordering::Relaxed) >= options.max_results as u32 {
                    return None;
                }

                let file_info = self.index.get_file(fid).ok()?;

                // Filter by extension
                if !options.type_filter.is_empty() {
                    let ext = file_info
                        .path
                        .extension()
                        .and_then(|e: &std::ffi::OsStr| e.to_str())
                        .unwrap_or("");
                    if !options.type_filter.iter().any(|e: &String| e == ext) {
                        return None;
                    }
                }

                files_verified.fetch_add(1, Ordering::Relaxed);
                bytes_verified.fetch_add(file_info.size_bytes, Ordering::Relaxed);

                let matches = self.verify_file(&file_info, &regex, options).ok()?;
                matches_found.fetch_add(matches.len() as u32, Ordering::Relaxed);
                Some(matches)
            })
            .flatten()
            .collect();

        stats.files_verified = files_verified.into_inner();
        stats.bytes_verified = bytes_verified.into_inner();

        if options.max_results > 0 && all_matches.len() > options.max_results {
            all_matches.truncate(options.max_results);
        }

        stats.total_matches = all_matches.len() as u32;

        Ok((all_matches, stats))
        }


    fn execute_regex_indexed(
        &self,
        regex: &Regex,
        required_trigram_sets: &[Vec<Trigram>],
        options: &QueryOptions,
    ) -> Result<(Vec<Match>, QueryStats)> {
        let mut stats = QueryStats::default();

        // For each required literal fragment, find candidate files
        let mut fragment_candidates = Vec::new();
        for trigram_set in required_trigram_sets {
            let mut infos = Vec::new();
            for &tri in trigram_set {
                stats.trigrams_queried += 1;
                if let Some(info) = self.index.get_trigram(tri) {
                    infos.push((tri, info));
                } else {
                    return Ok((vec![], stats));
                }
            }

            infos.sort_by_key(|(_, info)| info.doc_frequency);

            // Intersection within fragment
            let (_, rarest_info) = &infos[0];
            let postings = self.index.decode_postings(rarest_info)?;
            stats.posting_lists_decoded += 1;
            let mut set_candidates: HashSet<u32> =
                postings.entries.iter().map(|e| e.file_id).collect();

            // Intersect with up to 2 more lists if large
            for (_, info) in infos.iter().take(infos.len().min(3)).skip(1) {
                if set_candidates.len() < 100 {
                    break;
                }
                let next_postings = self.index.decode_postings(info)?;
                stats.posting_lists_decoded += 1;
                let next_set: HashSet<u32> =
                    next_postings.entries.iter().map(|e| e.file_id).collect();
                set_candidates.retain(|fid| next_set.contains(fid));
            }

            for &(tri, _) in &infos[1..] {
                set_candidates.retain(|&fid| self.index.bloom_may_contain(fid, tri));
            }
            fragment_candidates.push(set_candidates);
        }

        // Intersect candidates from all fragments
        let mut final_candidates: HashSet<u32> = match fragment_candidates.pop() {
            Some(c) => c,
            None => return Ok((vec![], stats)),
        };
        for set in fragment_candidates {
            final_candidates.retain(|fid: &u32| set.contains(fid));
        }

        stats.candidate_files = final_candidates.len() as u32;

        let files_verified = AtomicU32::new(0);
        let bytes_verified = AtomicU64::new(0);
        let matches_found = AtomicU32::new(0);

        let candidate_list: Vec<u32> = final_candidates.into_iter().collect();

        let mut all_matches: Vec<Match> = candidate_list
            .into_par_iter()
            .filter_map(|fid| {
                if options.max_results > 0
                    && matches_found.load(Ordering::Relaxed) >= options.max_results as u32
                {
                    return None;
                }

                let file_info = self.index.get_file(fid).ok()?;

                // Filter by extension
                if !options.type_filter.is_empty() {
                    let ext = file_info
                        .path
                        .extension()
                        .and_then(|e: &std::ffi::OsStr| e.to_str())
                        .unwrap_or("");
                    if !options.type_filter.iter().any(|e: &String| e == ext) {
                        return None;
                    }
                }

                files_verified.fetch_add(1, Ordering::Relaxed);
                bytes_verified.fetch_add(file_info.size_bytes, Ordering::Relaxed);

                let file_matches = self.verify_file(&file_info, regex, options).ok()?;
                matches_found.fetch_add(file_matches.len() as u32, Ordering::Relaxed);
                Some(file_matches)
            })
            .flatten()
            .collect();

        stats.files_verified = files_verified.into_inner();
        stats.bytes_verified = bytes_verified.into_inner();

        if options.max_results > 0 && all_matches.len() > options.max_results {
            all_matches.truncate(options.max_results);
        }

        stats.total_matches = all_matches.len() as u32;
        Ok((all_matches, stats))
    }

    fn execute_case_insensitive(
        &self,
        regex: &Regex,
        trigram_groups: &[Vec<Trigram>],
        options: &QueryOptions,
    ) -> Result<(Vec<Match>, QueryStats)> {
        let mut stats = QueryStats::default();

        // For each position group: UNION posting lists of all variants found
        let mut group_candidates = Vec::new();
        for group in trigram_groups {
            let mut union_set: HashSet<u32> = HashSet::new();
            for &tri in group {
                stats.trigrams_queried += 1;
                if let Some(info) = self.index.get_trigram(tri)
                    && let Ok(postings) = self.index.decode_postings(&info)
                {
                    stats.posting_lists_decoded += 1;
                    for entry in &postings.entries {
                        union_set.insert(entry.file_id);
                    }
                }
                // Missing variant = skip, not abort
            }
            if !union_set.is_empty() {
                group_candidates.push(union_set);
            }
        }

        // Intersect across position groups
        let final_candidates = if let Some(mut base) = group_candidates.pop() {
            for set in group_candidates {
                base.retain(|fid| set.contains(fid));
            }
            base
        } else {
            // No trigrams found at all — fall back to all files
            let all: HashSet<u32> = (0..self.index.header.file_count).collect();
            all
        };

        stats.candidate_files = final_candidates.len() as u32;

        let files_verified = AtomicU32::new(0);
        let bytes_verified = AtomicU64::new(0);
        let matches_found = AtomicU32::new(0);

        let candidate_list: Vec<u32> = final_candidates.into_iter().collect();

        let mut all_matches: Vec<Match> = candidate_list
            .into_par_iter()
            .filter_map(|fid| {
                if options.max_results > 0
                    && matches_found.load(Ordering::Relaxed) >= options.max_results as u32
                {
                    return None;
                }

                let file_info = self.index.get_file(fid).ok()?;

                if !options.type_filter.is_empty() {
                    let ext = file_info
                        .path
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("");
                    if !options.type_filter.iter().any(|e| e == ext) {
                        return None;
                    }
                }

                files_verified.fetch_add(1, Ordering::Relaxed);
                bytes_verified.fetch_add(file_info.size_bytes, Ordering::Relaxed);

                let file_matches = self.verify_file(&file_info, regex, options).ok()?;
                matches_found.fetch_add(file_matches.len() as u32, Ordering::Relaxed);
                Some(file_matches)
            })
            .flatten()
            .collect();

        stats.files_verified = files_verified.into_inner();
        stats.bytes_verified = bytes_verified.into_inner();

        if options.max_results > 0 && all_matches.len() > options.max_results {
            all_matches.truncate(options.max_results);
        }

        stats.total_matches = all_matches.len() as u32;
        Ok((all_matches, stats))
    }

    fn execute_full_scan(
        &self,
        regex: &Regex,
        options: &QueryOptions,
    ) -> Result<(Vec<Match>, QueryStats)> {
        let stats_candidate_files = self.index.header.file_count;

        let files_verified = AtomicU32::new(0);
        let bytes_verified = AtomicU64::new(0);
        let matches_found = AtomicU32::new(0);

        let mut all_matches: Vec<Match> = (0..self.index.header.file_count)
            .into_par_iter()
            .filter_map(|fid| {
                if options.max_results > 0
                    && matches_found.load(Ordering::Relaxed) >= options.max_results as u32
                {
                    return None;
                }

                let file_info = self.index.get_file(fid).ok()?;

                // Filter by extension
                if !options.type_filter.is_empty() {
                    let ext = file_info
                        .path
                        .extension()
                        .and_then(|e: &std::ffi::OsStr| e.to_str())
                        .unwrap_or("");
                    if !options.type_filter.iter().any(|e: &String| e == ext) {
                        return None;
                    }
                }

                files_verified.fetch_add(1, Ordering::Relaxed);
                bytes_verified.fetch_add(file_info.size_bytes, Ordering::Relaxed);

                let file_matches = self.verify_file(&file_info, regex, options).ok()?;
                matches_found.fetch_add(file_matches.len() as u32, Ordering::Relaxed);
                Some(file_matches)
            })
            .flatten()
            .collect();

        if options.max_results > 0 && all_matches.len() > options.max_results {
            all_matches.truncate(options.max_results);
        }

        let stats = QueryStats {
            candidate_files: stats_candidate_files,
            files_verified: files_verified.into_inner(),
            bytes_verified: bytes_verified.into_inner(),
            total_matches: all_matches.len() as u32,
            ..Default::default()
        };
        Ok((all_matches, stats))
    }

    fn is_binary(data: &[u8]) -> bool {
        if data.is_empty() {
            return false;
        }
        let check_len = data.len().min(512);
        let non_printable = data[..check_len]
            .iter()
            .filter(|&&b| !matches!(b, 0x09 | 0x0A | 0x0D | 0x20..=0x7E))
            .count();
        
        (non_printable as f32 / check_len as f32) > 0.3
    }

    /// Exposed for integration testing of the streaming logic.
    pub fn verify_stream_for_test<R: Read>(
        &self,
        reader: R,
        path: PathBuf,
        regex: &Regex,
        options: &QueryOptions,
    ) -> Result<Vec<Match>> {
        self.verify_stream(reader, path, regex, options)
    }

    fn verify_stream<R: Read>(
        &self,
        reader: R,
        path: PathBuf,
        regex: &Regex,
        options: &QueryOptions,
    ) -> Result<Vec<Match>> {
        let mut buf_reader = BufReader::new(reader);
        let mut matches = Vec::new();
        let mut line_number = 0u32;
        let mut byte_offset = 0u64;

        // Binary check on first 8KB
        {
            let buffer = buf_reader.fill_buf()?;
            let is_bin = Self::is_binary(buffer);
            if is_bin && !options.binary {
                return Ok(vec![]);
            }
        }

        let mut line = String::new();
        let mut context_before = std::collections::VecDeque::new();
        let mut pending_matches: Vec<Match> = Vec::new();

        while buf_reader.read_line(&mut line)? > 0 {
            line_number += 1;
            let line_len = line.len() as u64;
            let trimmed_line = line.trim_end().to_string();

            // Fill context_after for pending matches
            for m in &mut pending_matches {
                if m.context_after.len() < options.context_lines {
                    m.context_after.push(trimmed_line.clone());
                }
            }

            // Move completed matches to final list
            let (completed, still_pending): (Vec<_>, Vec<_>) = pending_matches
                .into_iter()
                .partition(|m| m.context_after.len() >= options.context_lines);
            matches.extend(completed);
            pending_matches = still_pending;

            if let Some(m) = regex.find(&line) {
                let context_before_vec: Vec<String> =
                    context_before.iter().map(|s: &String| s.trim_end().to_string()).collect();

                let new_match = Match {
                    file_path: path.clone(),
                    line_number,
                    col: (m.start() + 1) as u32,
                    line_content: if options.count_only {
                        String::new()
                    } else {
                        trimmed_line.clone()
                    },
                    byte_offset: byte_offset + m.start() as u64,
                    context_before: context_before_vec,
                    context_after: vec![],
                    is_binary: false,
                };

                if options.context_lines > 0 {
                    pending_matches.push(new_match);
                } else {
                    matches.push(new_match);
                }

                if options.max_results > 0 && (matches.len() + pending_matches.len()) >= options.max_results {
                    // Capped, but let's keep going to fill context if we really wanted to.
                    // Actually, if we hit max_results, we should just stop.
                    // But for streaming, stopping early might miss some context lines.
                    // Let's just break if we have enough matches.
                    if pending_matches.is_empty() || matches.len() >= options.max_results {
                        break;
                    }
                }
            }

            if options.context_lines > 0 {
                context_before.push_back(line.clone());
                if context_before.len() > options.context_lines {
                    context_before.pop_front();
                }
            }

            byte_offset += line_len;
            line.clear();
        }
        
        matches.extend(pending_matches);
        Ok(matches)
    }

    fn verify_file(
        &self,
        info: &FileInfo,
        regex: &Regex,
        options: &QueryOptions,
    ) -> Result<Vec<Match>> {
        let file = File::open(&info.path)?;
        let mmap = unsafe { memmap2::Mmap::map(&file)? };

        if options.decompress
            && let Some(reader) = maybe_decompress(&info.path, &mmap)?
        {
            return self.verify_stream(reader, info.path.clone(), regex, options);
        }

        // Binary check
        let is_bin = Self::is_binary(&mmap);
        if is_bin && !options.binary {
            return Ok(vec![]);
        }

        let mut matches = Vec::new();
        if options.multiline {
            let data = String::from_utf8_lossy(&mmap);
            for m in regex.find_iter(&data) {
                let byte_offset = m.start();
                let line_number = data[..byte_offset].chars().filter(|&c| c == '\n').count() + 1;
                let line_start = data[..byte_offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
                let line_end = data[byte_offset..].find('\n').map(|i| byte_offset + i).unwrap_or(data.len());
                let line_content = &data[line_start..line_end];

                let context_before = if options.context_lines > 0 {
                    let all_lines: Vec<&str> = data.lines().collect();
                    let current_line_idx = line_number - 1;
                    let start = current_line_idx.saturating_sub(options.context_lines);
                    all_lines[start..current_line_idx].iter().map(|s: &&str| s.to_string()).collect()
                } else {
                    vec![]
                };

                let context_after = if options.context_lines > 0 {
                    let all_lines: Vec<&str> = data.lines().collect();
                    let current_line_idx = line_number - 1;
                    let end = (current_line_idx + 1 + options.context_lines).min(all_lines.len());
                    all_lines[current_line_idx + 1..end].iter().map(|s: &&str| s.to_string()).collect()
                } else {
                    vec![]
                };

                matches.push(Match {
                    file_path: info.path.clone(),
                    line_number: line_number as u32,
                    col: (byte_offset - line_start + 1) as u32,
                    line_content: if options.count_only {
                        String::new()
                    } else {
                        line_content.to_string()
                    },
                    byte_offset: byte_offset as u64,
                    context_before,
                    context_after,
                    is_binary: is_bin,
                });

                if options.max_results > 0 && matches.len() >= options.max_results {
                    break;
                }
            }
        } else {
            let data = String::from_utf8_lossy(&mmap);
            let lines: Vec<&str> = data.lines().collect();
            let mut line_start_offset = 0;
            for (i, line) in lines.iter().enumerate() {
                if let Some(m) = regex.find(line) {
                    let context_before = if options.context_lines > 0 {
                        let start = i.saturating_sub(options.context_lines);
                        lines[start..i].iter().map(|s| s.to_string()).collect()
                    } else {
                        vec![]
                    };

                    let context_after = if options.context_lines > 0 {
                        let end = (i + 1 + options.context_lines).min(lines.len());
                        lines[i + 1..end].iter().map(|s| s.to_string()).collect()
                    } else {
                        vec![]
                    };

                    matches.push(Match {
                        file_path: info.path.clone(),
                        line_number: (i + 1) as u32,
                        col: (m.start() + 1) as u32,
                        line_content: if options.count_only {
                            String::new()
                        } else {
                            line.to_string()
                        },
                        byte_offset: (line_start_offset + m.start()) as u64,
                        context_before,
                        context_after,
                        is_binary: is_bin,
                    });

                    if options.max_results > 0 && matches.len() >= options.max_results {
                        break;
                    }
                }
                line_start_offset += line.len() + 1; // +1 for newline
            }
        }
        Ok(matches)
    }
}
