//! Query executor — search through the index and verify results.
//!
//! Handles literal searches, indexed regex, and full scans.

use std::fs::File;
use std::path::PathBuf;
use std::collections::HashSet;
use memmap2::Mmap;
use regex::Regex;
use crate::error::{Error, Result};
use crate::reader::{Reader, FileInfo};
use crate::planner::QueryPlan;
use crate::trigram::Trigram;

#[derive(Debug)]
pub struct Match {
    pub file_path: PathBuf,
    pub line_number: u32,
    pub line_content: String,
    pub byte_offset: u64,
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

pub struct Executor<'a> {
    index: &'a Reader,
}

impl<'a> Executor<'a> {
    pub fn new(index: &'a Reader) -> Self {
        Self { index }
    }

    pub fn execute(&self, plan: &QueryPlan) -> Result<(Vec<Match>, QueryStats)> {
        match plan {
            QueryPlan::Literal { pattern, trigrams } => self.execute_literal(pattern, trigrams),
            QueryPlan::RegexWithLiterals { regex, required_trigram_sets } => self.execute_regex_indexed(regex, required_trigram_sets),
            QueryPlan::FullScan { regex } => self.execute_full_scan(regex),
        }
    }

    fn execute_literal(&self, pattern: &[u8], trigrams: &[Trigram]) -> Result<(Vec<Match>, QueryStats)> {
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

        let (_, rarest_info) = &infos[0];
        let postings = self.index.decode_postings(rarest_info)?;
        stats.posting_lists_decoded += 1;

        let mut candidates: HashSet<u32> = postings.entries.iter().map(|e| e.file_id).collect();

        for &(tri, _) in &infos[1..] {
            candidates.retain(|&fid| self.index.bloom_may_contain(fid, tri));
            if candidates.is_empty() { break; }
        }

        stats.candidate_files = candidates.len() as u32;
        let mut matches = Vec::new();
        let regex = Regex::new(&regex::escape(&String::from_utf8_lossy(pattern))).unwrap();

        for fid in candidates {
            let file_info = self.index.get_file(fid)?;
            stats.files_verified += 1;
            stats.bytes_verified += file_info.size_bytes;
            if let Ok(file_matches) = self.verify_file(&file_info, &regex) {
                matches.extend(file_matches);
            }
        }

        stats.total_matches = matches.len() as u32;
        Ok((matches, stats))
    }

    fn execute_regex_indexed(&self, regex: &Regex, required_trigram_sets: &[Vec<Trigram>]) -> Result<(Vec<Match>, QueryStats)> {
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
            let (_, rarest_info) = &infos[0];
            let postings = self.index.decode_postings(rarest_info)?;
            stats.posting_lists_decoded += 1;
            
            let mut set_candidates: HashSet<u32> = postings.entries.iter().map(|e| e.file_id).collect();
            for &(tri, _) in &infos[1..] {
                set_candidates.retain(|&fid| self.index.bloom_may_contain(fid, tri));
            }
            fragment_candidates.push(set_candidates);
        }

        // Intersect candidates from all fragments
        let mut final_candidates = fragment_candidates.pop().unwrap();
        for set in fragment_candidates {
            final_candidates.retain(|fid| set.contains(fid));
        }

        stats.candidate_files = final_candidates.len() as u32;
        let mut matches = Vec::new();

        for fid in final_candidates {
            let file_info = self.index.get_file(fid)?;
            stats.files_verified += 1;
            stats.bytes_verified += file_info.size_bytes;
            if let Ok(file_matches) = self.verify_file(&file_info, regex) {
                matches.extend(file_matches);
            }
        }

        stats.total_matches = matches.len() as u32;
        Ok((matches, stats))
    }

    fn execute_full_scan(&self, regex: &Regex) -> Result<(Vec<Match>, QueryStats)> {
        let mut stats = QueryStats::default();
        let mut matches = Vec::new();

        for fid in 0..self.index.header.file_count {
            let file_info = self.index.get_file(fid)?;
            stats.files_verified += 1;
            stats.bytes_verified += file_info.size_bytes;
            if let Ok(file_matches) = self.verify_file(&file_info, regex) {
                matches.extend(file_matches);
            }
        }

        stats.candidate_files = self.index.header.file_count;
        stats.total_matches = matches.len() as u32;
        Ok((matches, stats))
    }

    fn verify_file(&self, info: &FileInfo, regex: &Regex) -> Result<Vec<Match>> {
        let file = File::open(&info.path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        let data = std::str::from_utf8(&mmap).map_err(|_| Error::Config("Binary file matched".into()))?;

        let mut matches = Vec::new();
        for (i, line) in data.lines().enumerate() {
            if let Some(m) = regex.find(line) {
                matches.push(Match {
                    file_path: info.path.clone(),
                    line_number: (i + 1) as u32,
                    line_content: line.to_string(),
                    byte_offset: m.start() as u64, // offset within line, close enough for now
                });
            }
        }
        Ok(matches)
    }
}
