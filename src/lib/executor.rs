//! Query executor — search through the index and verify results.
//!
//! Handles literal searches, indexed regex, and full scans.
//! Supports optional caching layers: posting list cache, negative result
//! cache, and regex compilation pool.

use crate::decompress::maybe_decompress;
use crate::error::Result;
use crate::format::is_binary;
use crate::neg_cache::NegCache;
use crate::planner::QueryPlan;
use crate::posting_cache::PostingCache;
use crate::reader::{DeltaReader, FileInfo, Reader};
use crate::regex_pool::RegexPool;
use crate::streaming;
use crate::trigram::Trigram;
use rayon::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::Sender;

/// A batch of search results produced progressively during a query.
///
/// Sent through a channel by `Executor::execute_progressive` as each file
/// is verified, rather than collecting all results before returning.
#[derive(Debug, Clone)]
pub struct ProgressiveBatch {
    /// Matches found in the batch of files processed so far.
    pub file_matches: Vec<Match>,
    /// Number of files verified in this batch.
    pub files_verified: u32,
    /// Bytes of file content read during verification of this batch.
    pub bytes_read: u64,
}

/// A single regex match found in a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Match {
    /// Absolute path to the file containing the match.
    pub file_path: PathBuf,
    /// 1-based line number.
    pub line_number: u32,
    /// 1-based column within the line (byte offset).
    pub col: u32,
    /// The entire content of the matching line.
    pub line_content: String,
    /// Byte offset from the start of the file.
    pub byte_offset: u64,
    /// Lines preceding the match (context).
    pub context_before: Vec<String>,
    /// Lines following the match (context).
    pub context_after: Vec<String>,
    /// Whether the file was detected as binary.
    pub is_binary: bool,
}

/// Performance counters collected during query execution.
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct QueryStats {
    /// Number of trigrams looked up in the trigram table.
    pub trigrams_queried: u32,
    /// Number of posting lists that were fully decoded from disk.
    pub posting_lists_decoded: u32,
    /// Number of candidate files after intersection and bloom filtering.
    pub candidate_files: u32,
    /// Number of files whose content was verified against the regex.
    pub files_verified: u32,
    /// Number of files that could not be verified due to I/O errors.
    pub files_failed_verify: u64,
    /// Total bytes of file content read during verification.
    pub bytes_verified: u64,
    /// Number of lines read during the search (stdin/streaming mode).
    pub lines_read: u32,
    /// Total number of matches produced.
    pub total_matches: u32,
    /// Total matches available before `max_results` truncation.
    /// Set to the same value as `total_matches` when no truncation occurred.
    #[serde(default)]
    pub total_available: u32,
    /// Posting list cache hits (decode avoided).
    pub posting_cache_hits: u64,
    /// Posting list cache misses (decode required).
    pub posting_cache_misses: u64,
    /// Negative cache hits (file verification skipped).
    pub neg_cache_hits: u64,
    /// Negative cache misses (file verification required).
    pub neg_cache_misses: u64,
}

/// Thread-safe accumulators for [`QueryStats`] during parallel verification.
struct QueryStatsAccum {
    files_verified: AtomicU32,
    files_failed_verify: AtomicU64,
    bytes_verified: AtomicU64,
    matches_found: AtomicU32,
    neg_cache_hits: AtomicU64,
    neg_cache_misses: AtomicU64,
}

impl QueryStatsAccum {
    const fn new() -> Self {
        Self {
            files_verified: AtomicU32::new(0),
            files_failed_verify: AtomicU64::new(0),
            bytes_verified: AtomicU64::new(0),
            matches_found: AtomicU32::new(0),
            neg_cache_hits: AtomicU64::new(0),
            neg_cache_misses: AtomicU64::new(0),
        }
    }

    // Cannot be const because it consumes self (destructuring)
    #[allow(clippy::missing_const_for_fn)]
    fn into_stats(self, candidate_files: u32, total_matches: u32, stats: &mut QueryStats) {
        stats.files_verified = self.files_verified.into_inner();
        stats.files_failed_verify = self.files_failed_verify.into_inner();
        stats.bytes_verified = self.bytes_verified.into_inner();
        stats.neg_cache_hits += self.neg_cache_hits.into_inner();
        stats.neg_cache_misses += self.neg_cache_misses.into_inner();
        stats.candidate_files = candidate_files;
        stats.total_matches = total_matches;
    }
}

/// Result of verifying a candidate file against a regex pattern.
///
/// Encodes three distinct outcomes so callers can distinguish
/// successful matches, negative-cache skips, and I/O failures.
#[derive(Debug)]
pub enum VerificationResult {
    /// One or more regex matches were found in the file.
    Matches(Vec<Match>),
    /// File was a known negative (cache hit) — skipped without I/O.
    Cached,
    /// File could not be verified due to an I/O error; the file
    /// may or may not contain matches.
    Failed(crate::error::Error),
}

impl VerificationResult {
    /// Project into `Option<Vec<Match>>` for use with `Iterator::filter_map`.
    ///
    /// **Scar tissue:** Both `Cached` and `Failed` collapse to `None`.
    /// This overloads the `Option<Vec<Match>>` semantic space — callers
    /// using `filter_map` cannot distinguish "file known to be irrelevant"
    /// from "file could not be read (I/O error)" using the return value
    /// alone. The `files_failed_verify` counter in [`QueryStats`] carries
    /// the I/O-error signal. Consumers MUST inspect
    /// [`QueryStats::files_failed_verify`] to detect partial results.
    ///
    /// Restoring type-level distinction would require restructuring the
    /// parallel `filter_map` verification chain — a non-trivial refactor
    /// of the hot verification path.
    ///
    /// `Matches` becomes `Some`, `Cached` becomes `None` (file skipped
    /// cheaply), and `Failed` logs a warning, increments the
    /// `files_failed_verify` counter in `accum`, and returns `None`.
    fn into_option(self, accum: &QueryStatsAccum, file_info: &FileInfo) -> Option<Vec<Match>> {
        match self {
            Self::Matches(m) => Some(m),
            Self::Cached => None,
            Self::Failed(e) => {
                tracing::warn!("ix: cannot verify file {}: {e}", file_info.path.display());
                accum.files_failed_verify.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }
}

/// Tunable knobs that control query execution behaviour.
#[derive(Debug, Default, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct QueryOptions {
    /// Only report match counts (not line content).
    pub count_only: bool,
    /// Only list file paths containing matches.
    pub files_only: bool,
    /// Maximum number of results to return (0 = unlimited).
    pub max_results: usize,
    /// File extensions to restrict the search to.
    pub type_filter: Vec<String>,
    /// Number of context lines to show before and after each match.
    pub context_lines: usize,
    /// Transparently decompress archives (e.g. `.gz`) when scanning.
    pub decompress: bool,
    /// Dot-matches-newline mode for regex matching.
    pub multiline: bool,
    /// Search inside archive files (zip, tar.gz).
    pub archive: bool,
    /// Search binary files as if they were text.
    pub binary: bool,
    /// Match only at word boundaries.
    pub word_boundary: bool,
    /// Per-chunk size in bytes for large-file chunked streaming.
    /// 0 means use the streaming module's default (`16 MiB`).
    pub chunk_size_bytes: usize,
    /// Overlap between adjacent chunks in bytes.
    /// 0 means use the streaming module's default (`1 MiB`).
    pub chunk_overlap_bytes: usize,
}

/// Query executor that searches through an open index and verifies
/// candidate files against the original regex.
///
/// Optional caching layers reduce redundant work across queries:
/// - [`PostingCache`] avoids re-decoding compressed posting lists
/// - [`NegCache`] skips re-verification of known non-matching files
/// - [`RegexPool`] caches compiled regex objects across queries
pub struct Executor<'a> {
    index: &'a Reader,
    delta: Option<DeltaReader>,
    delta_path: Option<std::path::PathBuf>,
    posting_cache: Arc<PostingCache>,
    neg_cache: Arc<NegCache>,
    regex_pool: Arc<RegexPool>,
    neg_query_fingerprint: u64,
}

impl<'a> Executor<'a> {
    /// Create an executor backed by the given index reader with default caches.
    #[must_use]
    pub fn new(index: &'a Reader) -> Self {
        Self {
            index,
            delta: None,
            delta_path: None,
            posting_cache: Arc::new(PostingCache::default()),
            neg_cache: Arc::new(NegCache::new(65_536)),
            regex_pool: Arc::new(RegexPool::new(256)),
            neg_query_fingerprint: 0,
        }
    }

    /// Create an executor that shares caches with another executor.
    ///
    /// Use this in daemon mode to reuse posting list, negative, and regex
    /// caches across queries without re-decoding or re-verifying.
    #[must_use]
    pub fn new_with_caches(
        index: &'a Reader,
        posting_cache: Arc<PostingCache>,
        neg_cache: Arc<NegCache>,
        regex_pool: Arc<RegexPool>,
        delta_path: Option<std::path::PathBuf>,
    ) -> Self {
        let delta = delta_path
            .as_ref()
            .and_then(|p| crate::reader::DeltaReader::open(p).ok());
        Self {
            index,
            delta,
            delta_path,
            posting_cache,
            neg_cache,
            regex_pool,
            neg_query_fingerprint: 0,
        }
    }

    /// Returns a reference to the posting list cache.
    #[must_use]
    pub fn posting_cache(&self) -> &PostingCache {
        &self.posting_cache
    }

    /// Returns a reference to the negative result cache.
    #[must_use]
    pub fn neg_cache(&self) -> &NegCache {
        &self.neg_cache
    }

    /// Returns a reference to the regex compilation pool.
    #[must_use]
    pub fn regex_pool(&self) -> &RegexPool {
        &self.regex_pool
    }

    /// Unified file info getter bridging main shard and delta index.
    fn get_file_info(&self, fid: u32) -> Option<crate::reader::FileInfo> {
        if fid >= self.index.header.file_count {
            let delta = self.delta.as_ref()?;
            let info = delta.id_to_fileinfo.get(&fid)?;
            Some(crate::reader::FileInfo {
                file_id: fid,
                path: info.path.clone(),
                status: crate::format::FileStatus::Fresh,
                mtime_ns: info.mtime,
                size_bytes: info.size,
                content_hash: info.hash,
            })
        } else {
            self.index.get_file(fid).ok()
        }
    }

    /// Set the path for delta file lookup. Delta is loaded lazily on first search.
    pub fn set_delta_path(&mut self, path: std::path::PathBuf) {
        self.delta_path = Some(path);
    }

    /// Ensure delta is loaded from the configured path.
    fn ensure_delta(&mut self) {
        if self.delta.is_none()
            && let Some(ref path) = self.delta_path
            && let Ok(dr) = DeltaReader::open(path)
        {
            self.delta = Some(dr);
        }
    }

    /// Check if a `file_id` is tombstoned.
    fn is_tombstoned(&self, file_id: u32) -> bool {
        self.delta
            .as_ref()
            .is_some_and(|d| d.tombstones.contains(&file_id))
    }

    /// Merge delta postings into a candidate set.
    fn merge_delta_candidates(
        &self,
        candidates: &mut std::collections::HashSet<u32>,
        trigram: crate::trigram::Trigram,
    ) {
        if let Some(ref delta) = self.delta
            && let Some(entries) = delta.postings.get(&trigram)
        {
            for entry in entries {
                if !self.is_tombstoned(entry.file_id) {
                    candidates.insert(entry.file_id);
                }
            }
        }
    }

    /// Execute a query plan against the index.
    ///
    /// # Errors
    ///
    /// Returns an error if I/O fails when reading index sections, if posted
    /// data is corrupted, or if file content cannot be read during verification.
    pub fn execute(
        &mut self,
        plan: &QueryPlan,
        options: &QueryOptions,
    ) -> Result<(Vec<Match>, QueryStats)> {
        self.neg_query_fingerprint = crate::neg_cache::query_fingerprint(plan.pattern_str());
        self.ensure_delta();
        match plan {
            QueryPlan::Literal {
                pattern,
                trigrams,
                regex,
            } => self.execute_literal(pattern, trigrams, regex, options),
            QueryPlan::RegexWithLiterals {
                regex,
                required_trigram_sets,
            } => self.execute_regex_indexed(regex, required_trigram_sets, options),
            QueryPlan::CaseInsensitive {
                regex,
                trigram_groups,
            } => self.execute_case_insensitive(regex, trigram_groups, options),
            QueryPlan::FullScan { regex } => Ok(self.execute_full_scan(regex, options)),
        }
    }

    /// Execute a query plan progressively, sending results per batch.
    ///
    /// Collects all matches by calling [`Executor::execute`], then sends
    /// them as a single [`ProgressiveBatch`] through the channel. The
    /// caller receives results as they arrive rather than waiting for the
    /// entire query to complete.
    ///
    /// # Errors
    ///
    /// Delegates to [`Executor::execute`]; see that method for error conditions.
    pub fn execute_progressive(
        &mut self,
        plan: &QueryPlan,
        options: &QueryOptions,
        sender: Sender<ProgressiveBatch>,
    ) -> Result<QueryStats> {
        let (matches, stats) = self.execute(plan, options)?;
        let batch = ProgressiveBatch {
            files_verified: stats.files_verified,
            bytes_read: stats.bytes_verified,
            file_matches: matches,
        };
        // Ignore send error (receiver dropped = caller no longer interested).
        if let Err(e) = sender.send(batch) {
            tracing::debug!("progressive receiver closed: {e}");
        }
        // Drop sender explicitly to signal completion to the receiver.
        drop(sender);
        Ok(stats)
    }

    /// Decode a posting list with caching. Returns a cache hit if available,
    /// otherwise decodes from the mmap and inserts into the cache.
    fn decode_postings_cached(
        &self,
        tri: Trigram,
        info: &crate::reader::TrigramInfo,
        stats: &mut QueryStats,
    ) -> Result<crate::posting::PostingList> {
        if let Some(cached) = self.posting_cache.get(tri) {
            stats.posting_cache_hits += 1;
            return Ok(cached);
        }
        stats.posting_cache_misses += 1;
        let list = self.index.decode_postings(info)?;
        self.posting_cache.insert(tri, list.clone());
        stats.posting_lists_decoded += 1;
        Ok(list)
    }

    #[allow(clippy::as_conversions)] // match counts: len()→u32 fits within range
    #[allow(clippy::indexing_slicing)] // infos sorted+checked: .get(0) always valid
    fn execute_literal(
        &self,
        _pattern: &[u8],
        trigrams: &[Trigram],
        regex: &Regex,
        options: &QueryOptions,
    ) -> Result<(Vec<Match>, QueryStats)> {
        let mut stats = QueryStats::default();

        let mut infos = Vec::new();
        for &tri in trigrams {
            stats.trigrams_queried += 1;
            if let Some(info) = self.index.get_trigram(tri)? {
                infos.push((tri, info));
            } else {
                return Ok((vec![], stats));
            }
        }

        // Sort by doc_frequency (rarest first)
        infos.sort_by_key(|(_, info)| info.doc_frequency);

        tracing::debug!(
            "literal search: {} trigrams, rarities: {:?}",
            infos.len(),
            infos
                .iter()
                .map(|(t, i)| (format!("0x{t:06x}"), i.doc_frequency))
                .collect::<Vec<_>>()
        );

        // ── Step 1: Decode rarest posting list ──
        let (rarest_tri, rarest_info) = &infos[0];
        let postings = self.decode_postings_cached(*rarest_tri, rarest_info, &mut stats)?;

        let mut candidates: HashSet<u32> = postings.entries.iter().map(|e| e.file_id).collect();

        // Filter tombstoned file_ids and merge delta entries
        candidates.retain(|&fid| !self.is_tombstoned(fid));
        self.merge_delta_candidates(&mut candidates, *rarest_tri);
        tracing::debug!("step 1 (rarest): {} candidates", candidates.len());

        // ── Step 2: Intersect with next rarest lists if candidate set is large ──
        // Only decode up to 3 lists to avoid excessive I/O
        for (tri, info) in infos.iter().take(infos.len().min(3)).skip(1) {
            if candidates.len() < 100 {
                tracing::debug!(
                    "step 2: breaking early, {} candidates < 100",
                    candidates.len()
                );
                break;
            }

            let next_postings = self.decode_postings_cached(*tri, info, &mut stats)?;

            let mut next_set: HashSet<u32> =
                next_postings.entries.iter().map(|e| e.file_id).collect();
            if let Some(ref delta) = self.delta {
                if let Some(entries) = delta.postings.get(tri) {
                    for entry in entries {
                        if !self.is_tombstoned(entry.file_id) {
                            next_set.insert(entry.file_id);
                        }
                    }
                }
            }
            candidates.retain(|fid| next_set.contains(fid));
        }

        // ── Step 3: Filter remaining using Bloom filters ──
        // KNOWN LIMITATION: bloom_may_contain only consults the main shard's
        // bloom filters. Delta-only files pass this check unconditionally.
        for &(tri, _) in &infos[1..] {
            if candidates.is_empty() {
                break;
            }
            candidates.retain(|&fid| self.index.bloom_may_contain(fid, tri));
        }

        stats.candidate_files = candidates.len() as u32;

        let accum = QueryStatsAccum::new();
        let neg_fp = self.neg_query_fingerprint;

        let candidate_list: Vec<u32> = candidates.into_iter().collect();

        let mut all_matches: Vec<Match> = candidate_list
            .into_par_iter()
            .filter_map(|fid| {
                let file_info = self.get_file_info(fid)?;

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

                accum.files_verified.fetch_add(1, Ordering::Relaxed);
                accum
                    .bytes_verified
                    .fetch_add(file_info.size_bytes, Ordering::Relaxed);

                let file_matches = self
                    .verify_candidate(&file_info, regex, options, neg_fp, &accum)
                    .into_option(&accum, &file_info)?;
                accum
                    .matches_found
                    .fetch_add(file_matches.len() as u32, Ordering::Relaxed);
                Some(file_matches)
            })
            .flatten()
            .collect();

        accum.into_stats(stats.candidate_files, all_matches.len() as u32, &mut stats);

        let total_before_truncation = all_matches.len() as u32;
        if options.max_results > 0 && !options.files_only && all_matches.len() > options.max_results
        {
            all_matches.truncate(options.max_results);
        }

        stats.total_matches = all_matches.len() as u32;
        stats.total_available = total_before_truncation;

        Ok((all_matches, stats))
    }

    #[allow(clippy::as_conversions)] // match counts: len()→u32 fits within range
    #[allow(clippy::indexing_slicing)] // infos sorted+checked: .get(0) always valid
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
                if let Some(info) = self.index.get_trigram(tri)? {
                    infos.push((tri, info));
                } else {
                    return Ok((vec![], stats));
                }
            }

            infos.sort_by_key(|(_, info)| info.doc_frequency);

            // Intersection within fragment
            let (rarest_tri, rarest_info) = &infos[0];
            let postings = self.decode_postings_cached(*rarest_tri, rarest_info, &mut stats)?;
            let mut set_candidates: HashSet<u32> =
                postings.entries.iter().map(|e| e.file_id).collect();
            set_candidates.retain(|&fid| !self.is_tombstoned(fid));
            self.merge_delta_candidates(&mut set_candidates, *rarest_tri);

            // Intersect with up to 2 more lists if large
            for (tri, info) in infos.iter().take(infos.len().min(3)).skip(1) {
                if set_candidates.len() < 100 {
                    break;
                }
                let next_postings = self.decode_postings_cached(*tri, info, &mut stats)?;
                let mut next_set: HashSet<u32> =
                    next_postings.entries.iter().map(|e| e.file_id).collect();
                if let Some(ref delta) = self.delta {
                    if let Some(entries) = delta.postings.get(tri) {
                        for entry in entries {
                            if !self.is_tombstoned(entry.file_id) {
                                next_set.insert(entry.file_id);
                            }
                        }
                    }
                }
                set_candidates.retain(|fid| next_set.contains(fid));
            }

            // KNOWN LIMITATION: bloom_may_contain only consults the main
            // shard's bloom filters; delta-only files pass unconditionally.
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

        // Tombstone filtering
        final_candidates.retain(|&fid| !self.is_tombstoned(fid));

        stats.candidate_files = final_candidates.len() as u32;

        let accum = QueryStatsAccum::new();
        let neg_fp = self.neg_query_fingerprint;

        let candidate_list: Vec<u32> = final_candidates.into_iter().collect();

        let mut all_matches: Vec<Match> = candidate_list
            .into_par_iter()
            .filter_map(|fid| {
                let file_info = self.get_file_info(fid)?;

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

                accum.files_verified.fetch_add(1, Ordering::Relaxed);
                accum
                    .bytes_verified
                    .fetch_add(file_info.size_bytes, Ordering::Relaxed);

                let file_matches = self
                    .verify_candidate(&file_info, regex, options, neg_fp, &accum)
                    .into_option(&accum, &file_info)?;
                accum
                    .matches_found
                    .fetch_add(file_matches.len() as u32, Ordering::Relaxed);
                Some(file_matches)
            })
            .flatten()
            .collect();

        accum.into_stats(stats.candidate_files, all_matches.len() as u32, &mut stats);

        let total_before_truncation = all_matches.len() as u32;
        if options.max_results > 0 && !options.files_only && all_matches.len() > options.max_results
        {
            all_matches.truncate(options.max_results);
        }

        stats.total_matches = all_matches.len() as u32;
        stats.total_available = total_before_truncation;
        Ok((all_matches, stats))
    }

    #[allow(clippy::as_conversions)] // match counts: len()→u32 fits within range
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
                if let Some(info) = self.index.get_trigram(tri)?
                    && let Ok(postings) = self.decode_postings_cached(tri, &info, &mut stats)
                {
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
        #[allow(clippy::option_if_let_else)]
        let mut final_candidates = if let Some(mut base) = group_candidates.pop() {
            for set in group_candidates {
                base.retain(|fid| set.contains(fid));
            }
            base
        } else {
            // No trigrams found at all — fall back to all files
            let all: HashSet<u32> = (0..self.index.header.file_count).collect();
            all
        };

        // Tombstone filtering + delta merge
        final_candidates.retain(|&fid| !self.is_tombstoned(fid));
        if let Some(ref delta) = self.delta {
            final_candidates.extend(
                delta
                    .id_to_fileinfo
                    .keys()
                    .copied()
                    .filter(|fid| !self.is_tombstoned(*fid)),
            );
        }

        stats.candidate_files = final_candidates.len() as u32;

        let accum = QueryStatsAccum::new();
        let neg_fp = self.neg_query_fingerprint;

        let candidate_list: Vec<u32> = final_candidates.into_iter().collect();

        let mut all_matches: Vec<Match> = candidate_list
            .into_par_iter()
            .filter_map(|fid| {
                let file_info = self.get_file_info(fid)?;

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

                accum.files_verified.fetch_add(1, Ordering::Relaxed);
                accum
                    .bytes_verified
                    .fetch_add(file_info.size_bytes, Ordering::Relaxed);

                let file_matches = self
                    .verify_candidate(&file_info, regex, options, neg_fp, &accum)
                    .into_option(&accum, &file_info)?;
                accum
                    .matches_found
                    .fetch_add(file_matches.len() as u32, Ordering::Relaxed);
                Some(file_matches)
            })
            .flatten()
            .collect();

        accum.into_stats(stats.candidate_files, all_matches.len() as u32, &mut stats);

        let total_before_truncation = all_matches.len() as u32;
        if options.max_results > 0 && !options.files_only && all_matches.len() > options.max_results
        {
            all_matches.truncate(options.max_results);
        }

        stats.total_matches = all_matches.len() as u32;
        stats.total_available = total_before_truncation;
        Ok((all_matches, stats))
    }

    #[allow(clippy::as_conversions)] // line count fits within range
    fn execute_full_scan(&self, regex: &Regex, options: &QueryOptions) -> (Vec<Match>, QueryStats) {
        let mut candidates: Vec<u32> = (0..self.index.header.file_count)
            .filter(|fid| !self.is_tombstoned(*fid))
            .collect();
        if let Some(ref delta) = self.delta {
            candidates.extend(
                delta
                    .id_to_fileinfo
                    .keys()
                    .copied()
                    .filter(|fid| !self.is_tombstoned(*fid)),
            );
        }
        let stats_candidate_files = candidates.len() as u32;

        let accum = QueryStatsAccum::new();
        let neg_fp = self.neg_query_fingerprint;

        let mut all_matches: Vec<Match> = candidates
            .into_par_iter()
            .filter_map(|fid| {
                let file_info = self.get_file_info(fid)?;

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

                accum.files_verified.fetch_add(1, Ordering::Relaxed);
                accum
                    .bytes_verified
                    .fetch_add(file_info.size_bytes, Ordering::Relaxed);

                let file_matches = self
                    .verify_candidate(&file_info, regex, options, neg_fp, &accum)
                    .into_option(&accum, &file_info)?;
                accum
                    .matches_found
                    .fetch_add(file_matches.len() as u32, Ordering::Relaxed);
                Some(file_matches)
            })
            .flatten()
            .collect();

        let total_before_truncation = all_matches.len() as u32;
        if options.max_results > 0 && !options.files_only && all_matches.len() > options.max_results
        {
            all_matches.truncate(options.max_results);
        }

        let mut stats = QueryStats {
            candidate_files: stats_candidate_files,
            total_matches: all_matches.len() as u32,
            total_available: total_before_truncation,
            ..Default::default()
        };
        accum.into_stats(stats_candidate_files, all_matches.len() as u32, &mut stats);
        (all_matches, stats)
    }

    /// Verify a candidate file, consulting the negative-result cache first.
    ///
    /// Returns [`VerificationResult::Cached`] when `(query_fingerprint,
    /// content_hash)` is a known negative (skipping file I/O).
    /// Returns [`VerificationResult::Matches`] with the matches found
    /// (recording zero-match results as negatives for future queries).
    /// Returns [`VerificationResult::Failed`] when the file cannot be
    /// read — the caller should log the error and increment the
    /// failure counter via [`VerificationResult::into_option`].
    fn verify_candidate(
        &self,
        file_info: &FileInfo,
        regex: &Regex,
        options: &QueryOptions,
        neg_fp: u64,
        stats: &QueryStatsAccum,
    ) -> VerificationResult {
        if self
            .neg_cache
            .is_known_negative(neg_fp, file_info.content_hash)
        {
            stats.neg_cache_hits.fetch_add(1, Ordering::Relaxed);
            return VerificationResult::Cached;
        }
        stats.neg_cache_misses.fetch_add(1, Ordering::Relaxed);

        match Self::verify_file(file_info, regex, options) {
            Ok(matches) => {
                if matches.is_empty() {
                    self.neg_cache
                        .record_negative(neg_fp, file_info.content_hash);
                }
                VerificationResult::Matches(matches)
            }
            Err(e) => VerificationResult::Failed(e),
        }
    }

    fn verify_file(info: &FileInfo, regex: &Regex, options: &QueryOptions) -> Result<Vec<Match>> {
        let owned_opts;
        let effective_options: &QueryOptions = if options.files_only && options.max_results == 0 {
            owned_opts = QueryOptions {
                max_results: 1,
                ..options.clone()
            };
            &owned_opts
        } else {
            options
        };

        let mut file = File::open(&info.path)?;

        // Decompression path: needs full mmap for maybe_decompress.
        // Binary check runs AFTER decompression so compressed files
        // (which always look binary) are not falsely rejected.
        if options.decompress {
            // SAFETY: The file is opened read-only and held for the
            // duration of this function. The mmap is used only within
            // streaming module calls which treat it as an immutable
            // byte slice. No concurrent modification to the underlying
            // file is expected during reading.
            let mmap = unsafe { memmap2::Mmap::map(&file)? };
            if let Some(reader) = maybe_decompress(&info.path, &mmap)? {
                let mut stats = crate::streaming::StreamStats::default();
                let result = streaming::stream_file(
                    reader,
                    info.path.as_ref(),
                    regex,
                    effective_options,
                    true,
                    &mut stats,
                );
                return result;
            }
            // Not compressed — mmap already exists, do binary check
            // then stream from the same mmap.
            if !effective_options.binary && is_binary(&mmap[..8192.min(mmap.len())]) {
                return Ok(vec![]);
            }
            return streaming::stream_file_chunked(
                &mmap[..],
                info.path.as_ref(),
                regex,
                effective_options,
            );
        }

        // Non-decompress path: check binary with a small read before
        // mmap to avoid wasting address space on large binary files.
        if !effective_options.binary {
            use std::io::Read;
            let mut header = [0u8; 8192];
            let n = (&mut file).take(8192).read(&mut header)?;
            if is_binary(&header[..n]) {
                return Ok(vec![]);
            }
        }

        // SAFETY: See the SAFETY comment above; same invariants apply.
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        streaming::stream_file_chunked(&mmap[..], info.path.as_ref(), regex, effective_options)
    }
}
