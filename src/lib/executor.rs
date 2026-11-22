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
use llmosafe::ResourceGuard;
use rayon::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
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
    /// Number of candidate files skipped because memory pressure reached backpressure threshold.
    #[serde(default)]
    pub files_skipped_pressure: u64,
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
    files_skipped_pressure: AtomicU64,
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
            files_skipped_pressure: AtomicU64::new(0),
            bytes_verified: AtomicU64::new(0),
            matches_found: AtomicU32::new(0),
            neg_cache_hits: AtomicU64::new(0),
            neg_cache_misses: AtomicU64::new(0),
        }
    }

    const fn into_stats(self, candidate_files: u32, total_matches: u32, stats: &mut QueryStats) {
        stats.files_verified = self.files_verified.into_inner();
        stats.files_failed_verify = self.files_failed_verify.into_inner();
        stats.files_skipped_pressure = self.files_skipped_pressure.into_inner();
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
// ARCHITECTURAL_ESCALATION: QueryOptions is a public exported struct with 7
// boolean fields constructed literally across tests and library consumers;
// grouping or hiding fields breaks public API compatibility
// (`clippy.toml`: `avoid-breaking-exported-api = true`).
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
/// - [`ResourceGuard`] provides backpressure under memory pressure
pub struct Executor<'a> {
    index: &'a Reader,
    delta: Option<DeltaReader>,
    delta_path: Option<std::path::PathBuf>,
    posting_cache: Arc<PostingCache>,
    neg_cache: Arc<NegCache>,
    regex_pool: Arc<RegexPool>,
    resource_guard: Option<Arc<ResourceGuard>>,
    resource_guard_ceiling: Option<usize>,
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
            resource_guard: None,
            resource_guard_ceiling: None,
            neg_query_fingerprint: 0,
        }
    }

    /// Create an executor that shares caches with another executor.
    ///
    /// Use this in daemon mode to reuse posting list, negative, and regex
    /// caches across queries without re-decoding or re-verifying.
    #[must_use]
    pub const fn new_with_caches(
        index: &'a Reader,
        posting_cache: Arc<PostingCache>,
        neg_cache: Arc<NegCache>,
        regex_pool: Arc<RegexPool>,
        delta_path: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            index,
            delta: None,
            delta_path,
            posting_cache,
            neg_cache,
            regex_pool,
            resource_guard: None,
            resource_guard_ceiling: None,
            neg_query_fingerprint: 0,
        }
    }

    /// Create an executor with a `ResourceGuard` for backpressure under memory pressure.
    ///
    /// The guard is checked during parallel verification to apply backpressure
    /// when system memory pressure exceeds 90%.
    #[must_use]
    pub fn with_resource_guard(mut self, guard: Arc<ResourceGuard>) -> Self {
        self.resource_guard = Some(guard);
        self
    }

    /// Attach a `ResourceGuard` paired with its configured ceiling in bytes so
    /// live RSS (`VmRSS`) can be preferred over monotonic peak `ru_maxrss`.
    #[must_use]
    pub fn with_resource_guard_and_ceiling(
        mut self,
        guard: Arc<ResourceGuard>,
        ceiling_bytes: usize,
    ) -> Self {
        self.resource_guard = Some(guard);
        self.resource_guard_ceiling = Some(ceiling_bytes);
        self
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
    ///
    /// Propagates structural index corruption (`SectionOutOfBounds`,
    /// `StringPoolOutOfBounds`, `FileIdOutOfBounds`) instead of silently
    /// dropping unreadable file-table entries.
    fn get_file_info(&self, fid: u32) -> Result<crate::reader::FileInfo> {
        if fid >= self.index.header.file_count {
            let delta = self
                .delta
                .as_ref()
                .ok_or(crate::error::Error::FileIdOutOfBounds(fid))?;
            let info = delta
                .id_to_fileinfo
                .get(&fid)
                .ok_or(crate::error::Error::FileIdOutOfBounds(fid))?;
            Ok(crate::reader::FileInfo {
                file_id: fid,
                path: info.path.clone(),
                status: crate::format::FileStatus::Fresh,
                mtime_ns: info.mtime,
                size_bytes: info.size,
                content_hash: info.hash,
            })
        } else {
            self.index.get_file(fid)
        }
    }

    /// Set the path for delta file lookup. Delta is loaded lazily on first search.
    pub fn set_delta_path(&mut self, path: std::path::PathBuf) {
        self.delta_path = Some(path);
    }

    /// Ensure delta is loaded from the configured path.
    ///
    /// Missing delta files (`ErrorKind::NotFound`) resolve to an empty
    /// [`DeltaReader`], while unreadable or corrupted delta files propagate
    /// their error to the caller.
    fn ensure_delta(&mut self) -> Result<()> {
        if self.delta.is_none()
            && let Some(ref path) = self.delta_path
        {
            self.delta = Some(DeltaReader::open(path)?);
        }
        Ok(())
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
    /// Returns an error if `options.archive` is requested on an indexed query,
    /// if I/O fails when reading index sections, if posted data is corrupted, or
    /// if file content cannot be read during verification.
    pub fn execute(
        &mut self,
        plan: &QueryPlan,
        options: &QueryOptions,
    ) -> Result<(Vec<Match>, QueryStats)> {
        if options.archive {
            return Err(crate::error::Error::Config(
                "--archive is not supported on indexed searches; use raw Scanner".into(),
            ));
        }
        if self
            .posting_cache
            .ensure_shard(self.index.shard_fingerprint())
        {
            self.neg_cache.clear();
        }
        self.neg_query_fingerprint = crate::neg_cache::query_fingerprint_with_options(
            plan.pattern_str(),
            options.binary,
            options.decompress,
            options.multiline,
            options.word_boundary,
        );
        self.ensure_delta()?;
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
            QueryPlan::FullScan { regex } => self.execute_full_scan(regex, options),
        }
    }

    /// Execute a query plan progressively, streaming [`ProgressiveBatch`] chunks
    /// through `sender` as candidate verification batches finish.
    ///
    /// # Errors
    ///
    /// Returns an error if `options.archive` is requested on an indexed query,
    /// if the delta index is corrupted, or if index sections fail validation.
    pub fn execute_progressive(
        &mut self,
        plan: &QueryPlan,
        options: &QueryOptions,
        sender: Sender<ProgressiveBatch>,
    ) -> Result<QueryStats> {
        const PROGRESSIVE_BATCH_SIZE: usize = 64;

        if options.archive {
            return Err(crate::error::Error::Config(
                "--archive is not supported on indexed searches; use raw Scanner".into(),
            ));
        }
        if self
            .posting_cache
            .ensure_shard(self.index.shard_fingerprint())
        {
            self.neg_cache.clear();
        }
        self.neg_query_fingerprint = crate::neg_cache::query_fingerprint_with_options(
            plan.pattern_str(),
            options.binary,
            options.decompress,
            options.multiline,
            options.word_boundary,
        );
        self.ensure_delta()?;

        let mut stats = QueryStats::default();
        let (regex, candidates_opt) = self.resolve_plan_candidates(plan, &mut stats)?;
        let Some(candidate_list) = candidates_opt else {
            let _ = sender.send(ProgressiveBatch {
                files_verified: 0,
                bytes_read: 0,
                file_matches: Vec::new(),
            });
            drop(sender);
            return Ok(stats);
        };

        let candidate_count = u32::try_from(candidate_list.len()).unwrap_or(u32::MAX);
        let accum = QueryStatsAccum::new();
        let mut seen_paths = HashSet::new();
        let mut total_sent: usize = 0;
        let mut total_available: usize = 0;
        let mut prev_files_verified: u32 = 0;
        let mut prev_bytes_verified: u64 = 0;
        let mut sent_any = false;

        for chunk in candidate_list.chunks(PROGRESSIVE_BATCH_SIZE) {
            if options.max_results > 0 && total_sent >= options.max_results {
                break;
            }
            let mut batch_matches = self.verify_candidates_parallel(
                chunk.to_vec(),
                regex,
                options,
                self.neg_query_fingerprint,
                &accum,
            )?;
            if options.files_only {
                batch_matches.retain(|m| seen_paths.insert(m.file_path.clone()));
            }
            total_available = total_available.saturating_add(batch_matches.len());
            if options.max_results > 0 {
                let remaining = options.max_results.saturating_sub(total_sent);
                if batch_matches.len() > remaining {
                    batch_matches.truncate(remaining);
                }
            }
            total_sent = total_sent.saturating_add(batch_matches.len());

            let cur_files = accum.files_verified.load(Ordering::Relaxed);
            let cur_bytes = accum.bytes_verified.load(Ordering::Relaxed);
            let delta_files = cur_files.saturating_sub(prev_files_verified);
            let delta_bytes = cur_bytes.saturating_sub(prev_bytes_verified);
            prev_files_verified = cur_files;
            prev_bytes_verified = cur_bytes;

            let batch = ProgressiveBatch {
                files_verified: delta_files,
                bytes_read: delta_bytes,
                file_matches: batch_matches,
            };
            sent_any = true;
            if let Err(e) = sender.send(batch) {
                tracing::debug!("progressive receiver closed: {e}");
                break;
            }
        }

        if !sent_any {
            let _ = sender.send(ProgressiveBatch {
                files_verified: 0,
                bytes_read: 0,
                file_matches: Vec::new(),
            });
        }
        drop(sender);

        let final_total = u32::try_from(total_sent).unwrap_or(u32::MAX);
        accum.into_stats(candidate_count, final_total, &mut stats);
        stats.total_available = u32::try_from(total_available).unwrap_or(u32::MAX);
        Ok(stats)
    }

    fn resolve_plan_candidates<'p>(
        &self,
        plan: &'p QueryPlan,
        stats: &mut QueryStats,
    ) -> Result<(&'p Regex, Option<Vec<u32>>)> {
        match plan {
            QueryPlan::Literal {
                trigrams, regex, ..
            } => {
                let mut infos = Vec::new();
                for &tri in trigrams {
                    stats.trigrams_queried += 1;
                    if let Some(info) = self.index.get_trigram(tri)? {
                        infos.push((tri, info));
                    } else if self
                        .delta
                        .as_ref()
                        .is_some_and(|d| d.postings.contains_key(&tri))
                    {
                        return Ok((regex, Some(self.all_live_candidates())));
                    } else {
                        return Ok((regex, None));
                    }
                }
                infos.sort_by_key(|(_, info)| info.doc_frequency);
                let candidates = self.intersect_trigram_infos(&infos, stats)?;
                Ok((regex, Some(candidates.into_iter().collect())))
            }
            QueryPlan::RegexWithLiterals {
                regex,
                required_trigram_sets,
            } => {
                let mut fragment_candidates = Vec::new();
                for trigram_set in required_trigram_sets {
                    let mut infos = Vec::new();
                    for &tri in trigram_set {
                        stats.trigrams_queried += 1;
                        if let Some(info) = self.index.get_trigram(tri)? {
                            infos.push((tri, info));
                        } else if self
                            .delta
                            .as_ref()
                            .is_some_and(|d| d.postings.contains_key(&tri))
                        {
                            return Ok((regex, Some(self.all_live_candidates())));
                        } else {
                            return Ok((regex, None));
                        }
                    }
                    infos.sort_by_key(|(_, info)| info.doc_frequency);
                    fragment_candidates.push(self.intersect_trigram_infos(&infos, stats)?);
                }
                let Some(mut final_candidates) = fragment_candidates.pop() else {
                    return Ok((regex, None));
                };
                for set in fragment_candidates {
                    final_candidates.retain(|fid: &u32| set.contains(fid));
                }
                final_candidates.retain(|&fid| !self.is_tombstoned(fid));
                Ok((regex, Some(final_candidates.into_iter().collect())))
            }
            QueryPlan::CaseInsensitive {
                regex,
                trigram_groups,
            } => {
                let mut group_candidates = Vec::new();
                for group in trigram_groups {
                    let mut union_set: HashSet<u32> = HashSet::new();
                    for &tri in group {
                        stats.trigrams_queried += 1;
                        if let Some(info) = self.index.get_trigram(tri)? {
                            let postings = self.decode_postings_cached(tri, &info, stats)?;
                            for entry in &postings.entries {
                                union_set.insert(entry.file_id);
                            }
                        }
                    }
                    if !union_set.is_empty() {
                        group_candidates.push(union_set);
                    }
                }
                let mut final_candidates = group_candidates.pop().map_or_else(
                    || (0..self.index.header.file_count).collect(),
                    |mut base| {
                        for set in group_candidates {
                            base.retain(|fid| set.contains(fid));
                        }
                        base
                    },
                );
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
                Ok((regex, Some(final_candidates.into_iter().collect())))
            }
            QueryPlan::FullScan { regex } => Ok((regex, Some(self.all_live_candidates()))),
        }
    }

    fn all_live_candidates(&self) -> Vec<u32> {
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
        candidates
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

    /// Intersect sorted trigram descriptors into a filtered candidate set.
    fn intersect_trigram_infos(
        &self,
        infos: &[(Trigram, crate::reader::TrigramInfo)],
        stats: &mut QueryStats,
    ) -> Result<HashSet<u32>> {
        let Some((rarest_tri, rarest_info)) = infos.first() else {
            return Ok(HashSet::new());
        };
        let postings = self.decode_postings_cached(*rarest_tri, rarest_info, stats)?;

        let mut candidates: HashSet<u32> = postings.entries.iter().map(|e| e.file_id).collect();
        candidates.retain(|&fid| !self.is_tombstoned(fid));
        self.merge_delta_candidates(&mut candidates, *rarest_tri);

        for (tri, info) in infos.iter().take(infos.len().min(3)).skip(1) {
            if candidates.len() < 100 {
                break;
            }
            let next_postings = self.decode_postings_cached(*tri, info, stats)?;
            let mut next_set: HashSet<u32> =
                next_postings.entries.iter().map(|e| e.file_id).collect();
            if let Some(ref delta) = self.delta
                && let Some(entries) = delta.postings.get(tri)
            {
                for entry in entries {
                    if !self.is_tombstoned(entry.file_id) {
                        next_set.insert(entry.file_id);
                    }
                }
            }
            candidates.retain(|fid| next_set.contains(fid));
        }

        // KNOWN LIMITATION: bloom_may_contain only consults the main shard's
        // bloom filters. Delta-only files pass unconditionally.
        for &(tri, _) in infos.iter().skip(1) {
            if candidates.is_empty() {
                break;
            }
            candidates.retain(|&fid| {
                fid >= self.index.header.file_count || self.index.bloom_may_contain(fid, tri)
            });
        }

        Ok(candidates)
    }

    fn finalize_execution(
        candidate_files: u32,
        mut matches: Vec<Match>,
        options: &QueryOptions,
        accum: QueryStatsAccum,
        stats: &mut QueryStats,
    ) -> (Vec<Match>, QueryStats) {
        if options.files_only {
            let mut seen_paths = HashSet::new();
            matches.retain(|m| seen_paths.insert(m.file_path.clone()));
        }
        let total_before_truncation = u32::try_from(matches.len()).unwrap_or(u32::MAX);
        if options.max_results > 0 && matches.len() > options.max_results {
            matches.truncate(options.max_results);
        }
        let final_total = u32::try_from(matches.len()).unwrap_or(u32::MAX);
        let mut final_stats = std::mem::take(stats);
        accum.into_stats(candidate_files, final_total, &mut final_stats);
        final_stats.total_available = total_before_truncation;
        (matches, final_stats)
    }

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
            } else if self
                .delta
                .as_ref()
                .is_some_and(|d| d.postings.contains_key(&tri))
            {
                // Trigram absent from the base shard but present in the delta —
                // fall back to a delta- and tombstone-aware content scan.
                return self.execute_full_scan(regex, options);
            } else {
                return Ok((vec![], stats));
            }
        }

        infos.sort_by_key(|(_, info)| info.doc_frequency);

        let candidates = self.intersect_trigram_infos(&infos, &mut stats)?;
        let candidate_count = u32::try_from(candidates.len()).unwrap_or(u32::MAX);

        let accum = QueryStatsAccum::new();
        let candidate_list: Vec<u32> = candidates.into_iter().collect();
        let all_matches = self.verify_candidates_parallel(
            candidate_list,
            regex,
            options,
            self.neg_query_fingerprint,
            &accum,
        )?;

        Ok(Self::finalize_execution(
            candidate_count,
            all_matches,
            options,
            accum,
            &mut stats,
        ))
    }

    fn execute_regex_indexed(
        &self,
        regex: &Regex,
        required_trigram_sets: &[Vec<Trigram>],
        options: &QueryOptions,
    ) -> Result<(Vec<Match>, QueryStats)> {
        let mut stats = QueryStats::default();

        let mut fragment_candidates = Vec::new();
        for trigram_set in required_trigram_sets {
            let mut infos = Vec::new();
            for &tri in trigram_set {
                stats.trigrams_queried += 1;
                if let Some(info) = self.index.get_trigram(tri)? {
                    infos.push((tri, info));
                } else if self
                    .delta
                    .as_ref()
                    .is_some_and(|d| d.postings.contains_key(&tri))
                {
                    return self.execute_full_scan(regex, options);
                } else {
                    return Ok((vec![], stats));
                }
            }

            infos.sort_by_key(|(_, info)| info.doc_frequency);
            fragment_candidates.push(self.intersect_trigram_infos(&infos, &mut stats)?);
        }

        let Some(mut final_candidates) = fragment_candidates.pop() else {
            return Ok((vec![], stats));
        };
        for set in fragment_candidates {
            final_candidates.retain(|fid: &u32| set.contains(fid));
        }

        final_candidates.retain(|&fid| !self.is_tombstoned(fid));

        let candidate_count = u32::try_from(final_candidates.len()).unwrap_or(u32::MAX);
        let accum = QueryStatsAccum::new();
        let candidate_list: Vec<u32> = final_candidates.into_iter().collect();
        let all_matches = self.verify_candidates_parallel(
            candidate_list,
            regex,
            options,
            self.neg_query_fingerprint,
            &accum,
        )?;

        Ok(Self::finalize_execution(
            candidate_count,
            all_matches,
            options,
            accum,
            &mut stats,
        ))
    }

    fn execute_case_insensitive(
        &self,
        regex: &Regex,
        trigram_groups: &[Vec<Trigram>],
        options: &QueryOptions,
    ) -> Result<(Vec<Match>, QueryStats)> {
        let mut stats = QueryStats::default();

        // For each position group: UNION posting lists of all variants found.
        let mut group_candidates = Vec::new();
        for group in trigram_groups {
            let mut union_set: HashSet<u32> = HashSet::new();
            for &tri in group {
                stats.trigrams_queried += 1;
                if let Some(info) = self.index.get_trigram(tri)? {
                    let postings = self.decode_postings_cached(tri, &info, &mut stats)?;
                    for entry in &postings.entries {
                        union_set.insert(entry.file_id);
                    }
                }
            }
            if !union_set.is_empty() {
                group_candidates.push(union_set);
            }
        }

        // Intersect across position groups (fallback to all files if no trigrams matched).
        let mut final_candidates = group_candidates.pop().map_or_else(
            || (0..self.index.header.file_count).collect(),
            |mut base| {
                for set in group_candidates {
                    base.retain(|fid| set.contains(fid));
                }
                base
            },
        );

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

        let candidate_count = u32::try_from(final_candidates.len()).unwrap_or(u32::MAX);
        let accum = QueryStatsAccum::new();
        let candidate_list: Vec<u32> = final_candidates.into_iter().collect();
        let all_matches = self.verify_candidates_parallel(
            candidate_list,
            regex,
            options,
            self.neg_query_fingerprint,
            &accum,
        )?;

        Ok(Self::finalize_execution(
            candidate_count,
            all_matches,
            options,
            accum,
            &mut stats,
        ))
    }

    fn execute_full_scan(
        &self,
        regex: &Regex,
        options: &QueryOptions,
    ) -> Result<(Vec<Match>, QueryStats)> {
        let candidates = self.all_live_candidates();
        let candidate_count = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
        let accum = QueryStatsAccum::new();
        let all_matches = self.verify_candidates_parallel(
            candidates,
            regex,
            options,
            self.neg_query_fingerprint,
            &accum,
        )?;
        let mut stats = QueryStats::default();
        Ok(Self::finalize_execution(
            candidate_count,
            all_matches,
            options,
            accum,
            &mut stats,
        ))
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

    fn measure_guard_pressure(guard: &ResourceGuard, ceiling: Option<usize>) -> u8 {
        match ceiling {
            Some(c) => crate::cache_policy::live_pressure_or_guard(guard, c),
            None => guard.pressure(),
        }
    }

    /// Parallel verification of candidate files with backpressure and early termination.
    ///
    /// Extracts the common verification pattern used by all four execute methods.
    /// Applies:
    /// - Type filter (file extension)
    /// - Early termination when `max_results` reached
    /// - Throttled `ResourceGuard` backpressure when memory pressure >= 90%
    /// - Negative cache lookup/recording
    /// - File verification via `verify_candidate`
    fn verify_candidates_parallel(
        &self,
        candidates: Vec<u32>,
        regex: &Regex,
        options: &QueryOptions,
        neg_fp: u64,
        accum: &QueryStatsAccum,
    ) -> Result<Vec<Match>> {
        const PRESSURE_SAMPLE_STRIDE: usize = 64;

        let max_results = options.max_results;
        let max_results_u32 = u32::try_from(max_results).unwrap_or(u32::MAX);
        let type_filter = options.type_filter.clone();
        let resource_guard = self.resource_guard.clone();
        let guard_ceiling = self.resource_guard_ceiling;
        let initial_pressure = resource_guard.as_ref().map_or(0, |guard| {
            Self::measure_guard_pressure(guard, guard_ceiling)
        });
        let cached_pressure = AtomicU8::new(initial_pressure);
        let sample_counter = AtomicUsize::new(0);

        let per_file: Result<Vec<Option<Vec<Match>>>> = candidates
            .into_par_iter()
            .map(|fid| -> Result<Option<Vec<Match>>> {
                // Early termination: check if max_results already reached.
                if max_results > 0 && accum.matches_found.load(Ordering::Acquire) >= max_results_u32
                {
                    return Ok(None);
                }

                // Backpressure: sample ResourceGuard once at batch start and every
                // PRESSURE_SAMPLE_STRIDE candidates to avoid per-file getrusage syscalls.
                if let Some(ref guard) = resource_guard {
                    let idx = sample_counter.fetch_add(1, Ordering::Relaxed);
                    let pressure = if idx > 0 && idx % PRESSURE_SAMPLE_STRIDE == 0 {
                        let measured = Self::measure_guard_pressure(guard, guard_ceiling);
                        cached_pressure.store(measured, Ordering::Relaxed);
                        measured
                    } else {
                        cached_pressure.load(Ordering::Relaxed)
                    };
                    if pressure >= 90 {
                        accum.files_skipped_pressure.fetch_add(1, Ordering::Relaxed);
                        return Ok(None);
                    }
                }

                let file_info = self.get_file_info(fid)?;

                // Type filter (file extension)
                if !type_filter.is_empty() {
                    let ext = file_info
                        .path
                        .extension()
                        .and_then(|e: &std::ffi::OsStr| e.to_str())
                        .unwrap_or("");
                    if !type_filter.iter().any(|e: &String| e == ext) {
                        return Ok(None);
                    }
                }

                accum.files_verified.fetch_add(1, Ordering::Relaxed);
                accum
                    .bytes_verified
                    .fetch_add(file_info.size_bytes, Ordering::Relaxed);

                let Some(file_matches) = self
                    .verify_candidate(&file_info, regex, options, neg_fp, accum)
                    .into_option(accum, &file_info)
                else {
                    return Ok(None);
                };
                let added = u32::try_from(file_matches.len()).unwrap_or(u32::MAX);
                accum.matches_found.fetch_add(added, Ordering::Relaxed);
                Ok(Some(file_matches))
            })
            .collect();

        Ok(per_file?.into_iter().flatten().flatten().collect())
    }

    fn verify_file(info: &FileInfo, regex: &Regex, options: &QueryOptions) -> Result<Vec<Match>> {
        let owned_opts;
        let effective_options: &QueryOptions = if options.files_only {
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
                return streaming::stream_file(
                    reader,
                    info.path.as_ref(),
                    regex,
                    effective_options,
                    true,
                    &mut stats,
                );
            }
            // Not compressed — mmap already exists, do binary check
            // then stream from the same mmap.
            let header_slice = mmap.get(..8192.min(mmap.len())).unwrap_or_default();
            if !effective_options.binary && is_binary(header_slice) {
                return Ok(vec![]);
            }
            return streaming::stream_file_chunked(
                mmap.as_ref(),
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
            let header_slice = header.get(..n).unwrap_or_default();
            if is_binary(header_slice) {
                return Ok(vec![]);
            }
        }

        // SAFETY: See the SAFETY comment above; same invariants apply.
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        streaming::stream_file_chunked(mmap.as_ref(), info.path.as_ref(), regex, effective_options)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use tempfile::tempdir;

    use crate::builder::Builder;
    use crate::executor::{Executor, QueryOptions};
    use crate::planner::Planner;
    use crate::reader::Reader;
    use llmosafe::ResourceGuard;

    /// Helper: build an index with multiple files for backpressure testing.
    fn build_test_index(root: &std::path::Path) -> Reader {
        // Create multiple files to ensure there's work to skip under pressure
        for i in 0..10 {
            let content = format!("file {i}\nsearch term {i}\nanother line {i}\n");
            fs::write(root.join(format!("file_{i}.txt")), content).unwrap();
        }
        let mut builder = Builder::new(root).unwrap();
        builder.build().unwrap();
        let index_path = root.join(".ix").join("shard.ix");
        Reader::open(&index_path).unwrap()
    }

    #[test]
    fn backpressure_skips_files_at_high_pressure() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let reader = build_test_index(root);

        // Create a ResourceGuard with for_testing at 95% pressure (above 90% threshold)
        let guard = ResourceGuard::for_testing(1024 * 1024, 0, 95);
        let mut executor = Executor::new(&reader).with_resource_guard(Arc::new(guard));

        let plan =
            Planner::plan_with_options("search term", crate::planner::QueryOptions::default())
                .unwrap();
        let (_matches, stats) = executor.execute(&plan, &QueryOptions::default()).unwrap();

        // Under high pressure (95%), the backpressure filter should skip files
        // We expect fewer files verified than the total number of files (10)
        // Note: exact count depends on parallelism and timing, but should be < 10
        assert!(
            stats.files_verified < 10,
            "High pressure (95%) should cause some files to be skipped, but verified {}",
            stats.files_verified
        );

        // Under very high pressure, it's possible that all files are skipped
        // before any matches are found. This is acceptable behavior — the
        // backpressure mechanism is working as designed to prevent OOM.
        // Verify that files_verified < 10 and files_skipped_pressure > 0.
        assert!(
            stats.files_skipped_pressure > 0,
            "High pressure should increment files_skipped_pressure, got 0"
        );
    }

    #[test]
    fn neg_cache_distinguishes_binary_and_options() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let data_path = root.join("data.txt");
        // Index plain text containing "needle" first so the trigram index records `data.txt`.
        fs::write(&data_path, b"needle placeholder text\n").unwrap();
        let mut builder = Builder::new(root).unwrap();
        builder.build().unwrap();
        // Before a stale index rebuild, replace contents with binary bytes (>30% NUL)
        // plus "needle" while `shard.ix` keeps the prior `content_hash` for `data.txt`.
        fs::write(&data_path, b"\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0needle\n").unwrap();

        let reader = Reader::open(&root.join(".ix/shard.ix")).unwrap();
        let mut executor = Executor::new(&reader);

        let plan =
            Planner::plan_with_options("needle", crate::planner::QueryOptions::default()).unwrap();

        // Query 1: binary=false -> 0 matches, records negative entry.
        let (m1, s1) = executor.execute(&plan, &QueryOptions::default()).unwrap();
        assert!(m1.is_empty());
        assert_eq!(s1.neg_cache_misses, 1);

        // Query 2: binary=true with same NegCache -> must NOT be poisoned by binary=false.
        let (m2, s2) = executor
            .execute(
                &plan,
                &QueryOptions {
                    binary: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(m2.len(), 1);
        assert_eq!(s2.neg_cache_hits, 0);
        assert_eq!(s2.neg_cache_misses, 1);

        // Query 3: repeat binary=false -> hits negative cache recorded in Query 1.
        let (m3, s3) = executor.execute(&plan, &QueryOptions::default()).unwrap();
        assert!(m3.is_empty());
        assert_eq!(s3.neg_cache_hits, 1);

        let base =
            crate::neg_cache::query_fingerprint_with_options("needle", false, false, false, false);
        assert_ne!(
            base,
            crate::neg_cache::query_fingerprint_with_options("needle", false, true, false, false)
        );
        assert_ne!(
            base,
            crate::neg_cache::query_fingerprint_with_options("needle", false, false, true, false)
        );
        assert_ne!(
            base,
            crate::neg_cache::query_fingerprint_with_options("needle", false, false, false, true)
        );
    }

    #[test]
    fn files_only_with_max_results_caps_distinct_files() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.txt"), "needle 1\nneedle 2\nneedle 3\n").unwrap();
        fs::write(root.join("b.txt"), "needle 4\nneedle 5\n").unwrap();
        fs::write(root.join("c.txt"), "needle 6\n").unwrap();
        let mut builder = Builder::new(root).unwrap();
        builder.build().unwrap();
        let reader = Reader::open(&root.join(".ix/shard.ix")).unwrap();
        let mut executor = Executor::new(&reader);

        let plan =
            Planner::plan_with_options("needle", crate::planner::QueryOptions::default()).unwrap();
        let (matches, stats) = executor
            .execute(
                &plan,
                &QueryOptions {
                    files_only: true,
                    max_results: 2,
                    ..Default::default()
                },
            )
            .unwrap();
        let distinct: std::collections::HashSet<_> =
            matches.iter().map(|m| m.file_path.clone()).collect();
        assert_eq!(distinct.len(), 2);
        assert_eq!(matches.len(), 2);
        assert_eq!(stats.total_matches, 2);
    }

    #[test]
    fn execute_rejects_archive_option_on_indexed_executor() {
        let dir = tempdir().unwrap();
        let reader = build_test_index(dir.path());
        let mut executor = Executor::new(&reader);
        let plan =
            Planner::plan_with_options("search term", crate::planner::QueryOptions::default())
                .unwrap();
        let err = executor
            .execute(
                &plan,
                &QueryOptions {
                    archive: true,
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("--archive"));
    }

    #[test]
    fn literal_and_regex_delta_new_file_passes_bloom() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("base.txt"), "existing anchor phrase\n").unwrap();
        let mut builder = Builder::new(root).unwrap();
        builder.build().unwrap();

        let delta_file = root.join("added.txt");
        fs::write(&delta_file, "existing anchor phrase delta\n").unwrap();
        builder.update(&[delta_file]).unwrap();

        let reader = Reader::open(&root.join(".ix/shard.ix")).unwrap();
        let mut executor = Executor::new(&reader);
        executor.set_delta_path(root.join(".ix/shard.ix.delta"));

        let lit_plan = Planner::plan_with_options(
            "existing anchor phrase",
            crate::planner::QueryOptions::default(),
        )
        .unwrap();
        let (lit_matches, _) = executor
            .execute(&lit_plan, &QueryOptions::default())
            .unwrap();
        assert_eq!(lit_matches.len(), 2);
    }

    #[test]
    fn backpressure_allows_files_at_low_pressure() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let reader = build_test_index(root);

        // Create a ResourceGuard with for_testing at 25% pressure (below 90% threshold)
        let guard = ResourceGuard::for_testing(1024 * 1024, 0, 25);
        let mut executor = Executor::new(&reader).with_resource_guard(Arc::new(guard));

        let plan =
            Planner::plan_with_options("search term", crate::planner::QueryOptions::default())
                .unwrap();
        let (matches, stats) = executor.execute(&plan, &QueryOptions::default()).unwrap();

        // Under low pressure (25%), all files should be processed
        assert_eq!(
            stats.files_verified, 10,
            "Low pressure (25%) should allow all files to be processed"
        );

        // All 10 files should match (each contains "search term N")
        assert_eq!(
            matches.len(),
            10,
            "All 10 files should produce matches at low pressure"
        );
    }

    #[test]
    fn no_resource_guard_processes_all_files() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let reader = build_test_index(root);

        // Create executor without ResourceGuard
        let mut executor = Executor::new(&reader);

        let plan =
            Planner::plan_with_options("search term", crate::planner::QueryOptions::default())
                .unwrap();
        let (matches, stats) = executor.execute(&plan, &QueryOptions::default()).unwrap();

        // Without backpressure, all files should be processed
        assert_eq!(
            stats.files_verified, 10,
            "Without ResourceGuard, all files should be processed"
        );
        assert_eq!(matches.len(), 10, "All 10 files should produce matches");
    }
}
