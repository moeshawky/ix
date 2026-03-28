//! Query executor — search through the index and verify results.
//!
//! Handles literal searches, indexed regex, and full scans.

use crate::error::Result;
use crate::planner::QueryPlan;
use crate::reader::{FileInfo, Reader};
use crate::trigram::Trigram;
use memmap2::Mmap;
use regex::Regex;
use std::collections::HashSet;
use std::fs::File;
use std::path::PathBuf;

#[derive(Debug)]
pub struct Match {
    pub file_path: PathBuf,
    pub line_number: u32,
    pub col: u32,
    pub line_content: String,
    pub byte_offset: u64,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
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
        let mut matches = Vec::new();
        let regex = Regex::new(&regex::escape(&String::from_utf8_lossy(pattern))).unwrap();

        for fid in candidates {
            let file_info = self.index.get_file(fid)?;

            // Filter by extension
            if !options.type_filter.is_empty() {
                let ext = file_info
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                if !options.type_filter.iter().any(|e| e == ext) {
                    continue;
                }
            }

            stats.files_verified += 1;
            stats.bytes_verified += file_info.size_bytes;
            if let Ok(file_matches) =
                self.verify_file(&file_info, &regex, options.count_only, options.context_lines)
            {
                if options.count_only {
                    stats.total_matches += file_matches.len() as u32;
                } else {
                    for m in file_matches {
                        matches.push(m);
                        if options.max_results > 0 && matches.len() >= options.max_results {
                            stats.total_matches = matches.len() as u32;
                            return Ok((matches, stats));
                        }
                    }
                }
            }
        }

        if !options.count_only {
            stats.total_matches = matches.len() as u32;
        }
        Ok((matches, stats))
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
        let mut final_candidates = match fragment_candidates.pop() {
            Some(c) => c,
            None => return Ok((vec![], stats)),
        };
        for set in fragment_candidates {
            final_candidates.retain(|fid| set.contains(fid));
        }

        stats.candidate_files = final_candidates.len() as u32;
        let mut matches = Vec::new();

        for fid in final_candidates {
            let file_info = self.index.get_file(fid)?;

            // Filter by extension
            if !options.type_filter.is_empty() {
                let ext = file_info
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                if !options.type_filter.iter().any(|e| e == ext) {
                    continue;
                }
            }

            stats.files_verified += 1;
            stats.bytes_verified += file_info.size_bytes;
            if let Ok(file_matches) =
                self.verify_file(&file_info, regex, options.count_only, options.context_lines)
            {
                if options.count_only {
                    stats.total_matches += file_matches.len() as u32;
                } else {
                    for m in file_matches {
                        matches.push(m);
                        if options.max_results > 0 && matches.len() >= options.max_results {
                            stats.total_matches = matches.len() as u32;
                            return Ok((matches, stats));
                        }
                    }
                }
            }
        }

        if !options.count_only {
            stats.total_matches = matches.len() as u32;
        }
        Ok((matches, stats))
    }

    fn execute_full_scan(
        &self,
        regex: &Regex,
        options: &QueryOptions,
    ) -> Result<(Vec<Match>, QueryStats)> {
        let mut stats = QueryStats::default();
        let mut matches = Vec::new();

        for fid in 0..self.index.header.file_count {
            let file_info = self.index.get_file(fid)?;

            // Filter by extension
            if !options.type_filter.is_empty() {
                let ext = file_info
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                if !options.type_filter.iter().any(|e| e == ext) {
                    continue;
                }
            }

            stats.files_verified += 1;
            stats.bytes_verified += file_info.size_bytes;
            if let Ok(file_matches) =
                self.verify_file(&file_info, regex, options.count_only, options.context_lines)
            {
                if options.count_only {
                    stats.total_matches += file_matches.len() as u32;
                } else {
                    for m in file_matches {
                        matches.push(m);
                        if options.max_results > 0 && matches.len() >= options.max_results {
                            stats.total_matches = matches.len() as u32;
                            return Ok((matches, stats));
                        }
                    }
                }
            }
        }

        stats.candidate_files = self.index.header.file_count;
        if !options.count_only {
            stats.total_matches = matches.len() as u32;
        }
        Ok((matches, stats))
    }

    fn verify_file(
        &self,
        info: &FileInfo,
        regex: &Regex,
        count_only: bool,
        context: usize,
    ) -> Result<Vec<Match>> {
        let file = File::open(&info.path)?;
        let mmap = unsafe { Mmap::map(&file)? };

        // Use lossy conversion to handle files with mixed encoding
        let data = String::from_utf8_lossy(&mmap);
        let lines: Vec<&str> = data.lines().collect();

        let mut matches = Vec::new();
        let mut line_start_offset = 0;
        for (i, line) in lines.iter().enumerate() {
            if let Some(m) = regex.find(line) {
                let context_before = if context > 0 {
                    let start = i.saturating_sub(context);
                    lines[start..i].iter().map(|s| s.to_string()).collect()
                } else {
                    vec![]
                };

                let context_after = if context > 0 {
                    let end = (i + 1 + context).min(lines.len());
                    lines[i + 1..end].iter().map(|s| s.to_string()).collect()
                } else {
                    vec![]
                };

                matches.push(Match {
                    file_path: info.path.clone(),
                    line_number: (i + 1) as u32,
                    col: (m.start() + 1) as u32,
                    line_content: if count_only {
                        String::new()
                    } else {
                        line.to_string()
                    },
                    byte_offset: (line_start_offset + m.start()) as u64,
                    context_before,
                    context_after,
                });
            }
            line_start_offset += line.len() + 1; // +1 for newline
        }
        Ok(matches)
    }
}
