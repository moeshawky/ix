use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::executor::{Executor, Match, ProgressiveBatch, QueryOptions, QueryStats};
use crate::neg_cache::NegCache;
use crate::planner::{Planner, QueryPlan};
use crate::posting_cache::PostingCache;
use crate::reader::Reader;
use crate::regex_pool::RegexPool;

use super::types::{SearchQuery, SearchResults};

/// Bundled cache and safety-guard handles for daemon-mode search execution.
#[derive(Clone)]
pub(crate) struct SearchCaches {
    pub posting_cache: Arc<PostingCache>,
    pub neg_cache: Arc<NegCache>,
    pub regex_pool: Arc<RegexPool>,
    pub resource_guard: Option<Arc<llmosafe::ResourceGuard>>,
    pub resource_guard_ceiling: Option<usize>,
}

impl SearchCaches {
    /// Synchronize shared posting and negative caches against `reader`'s shard
    /// generation fingerprint so out-of-band shard swaps cannot poison queries.
    pub(crate) fn sync_to_reader(&self, reader: &Reader) {
        if self.posting_cache.ensure_shard(reader.shard_fingerprint()) {
            self.neg_cache.clear();
        }
    }
}

/// # Errors
///
/// Returns an error if the index cannot be opened or the search fails.
pub fn execute_search(
    root: &Path,
    query: &SearchQuery,
) -> std::result::Result<SearchResults, Box<dyn std::error::Error + Send + Sync>> {
    execute_search_inner(root, query, None)
}

/// Execute a search query in progressive mode, sending results via a channel.
///
/// # Errors
///
/// Returns an error if the index cannot be opened or the search fails.
pub fn execute_search_progressive(
    root: &Path,
    query: &SearchQuery,
    sender: &std::sync::mpsc::Sender<SearchResults>,
) -> std::result::Result<QueryStats, Box<dyn std::error::Error + Send + Sync>> {
    execute_search_progressive_inner(root, query, sender, None)
}

fn resolve_search_path(
    root: &Path,
    canonical_root: &Path,
    search_path: Option<&PathBuf>,
) -> Option<PathBuf> {
    search_path.map(|sp| {
        sp.canonicalize().unwrap_or_else(|_| {
            sp.strip_prefix(root)
                .map_or_else(|_| sp.clone(), |rel| canonical_root.join(rel))
        })
    })
}

fn resolve_match_abs_path(file_path: &Path, root: &Path, canonical_root: &Path) -> PathBuf {
    if file_path.is_absolute() {
        file_path.strip_prefix(root).map_or_else(
            |_| {
                file_path
                    .canonicalize()
                    .unwrap_or_else(|_| file_path.to_path_buf())
            },
            |rel| canonical_root.join(rel),
        )
    } else {
        canonical_root.join(file_path)
    }
}

fn filter_matches_by_path(
    matches: Vec<Match>,
    root: &Path,
    canonical_root: &Path,
    search_path: &Path,
) -> Vec<Match> {
    matches
        .into_iter()
        .filter(|m| {
            let abs_path = resolve_match_abs_path(&m.file_path, root, canonical_root);
            abs_path.starts_with(search_path)
        })
        .collect()
}

fn format_verify_error(stats: &QueryStats) -> Option<String> {
    match (stats.files_failed_verify, stats.files_skipped_pressure) {
        (0, 0) => None,
        (f, 0) => Some(format!(
            "{f} file(s) could not be verified (I/O error) \u{2014} results may be incomplete"
        )),
        (0, p) => Some(format!(
            "{p} file(s) skipped due to high memory pressure \u{2014} results may be incomplete"
        )),
        (f, p) => Some(format!(
            "{f} file(s) failed I/O verify and {p} skipped under memory pressure \u{2014} results may be incomplete"
        )),
    }
}

fn build_plan_and_options(
    query: &SearchQuery,
    regex_pool: &RegexPool,
    is_scoped: bool,
) -> std::result::Result<(QueryPlan, QueryOptions), Box<dyn std::error::Error + Send + Sync>> {
    let plan = Planner::plan_with_pool(
        &query.pattern,
        crate::planner::QueryOptions {
            is_regex: query.is_regex,
            ignore_case: query.ignore_case,
            multiline: query.multiline,
            word_boundary: query.word_boundary,
        },
        regex_pool,
    )?;

    let options = QueryOptions {
        count_only: query.count_only,
        files_only: query.files_only,
        // Defer max_results truncation until after search_path prefix filtering
        // when scoped to a strict subdirectory so out-of-scope matches do not
        // starve in-scope results.
        max_results: if is_scoped { 0 } else { query.max_results },
        type_filter: crate::file_types::expand(&query.file_types),
        context_lines: query.context_lines,
        decompress: query.decompress,
        multiline: query.multiline,
        archive: query.archive,
        binary: query.binary,
        word_boundary: query.word_boundary,
        chunk_size_bytes: query.chunk_size_bytes,
        chunk_overlap_bytes: query.chunk_overlap_bytes,
    };

    Ok((plan, options))
}

fn open_reader_and_delta(
    root: &Path,
) -> std::result::Result<(Reader, PathBuf), Box<dyn std::error::Error + Send + Sync>> {
    let index_dir = root.join(".ix");
    let index_path = index_dir.join("shard.ix");
    if !index_path.exists() {
        return Err("index not found".into());
    }
    let reader = Reader::open(&index_path)?;
    Ok((reader, index_dir.join("shard.ix.delta")))
}

fn make_executor<'a>(
    reader: &'a Reader,
    delta_path: PathBuf,
    caches: Option<&SearchCaches>,
) -> Executor<'a> {
    if let Some(c) = caches {
        c.sync_to_reader(reader);
        let exec = Executor::new_with_caches(
            reader,
            Arc::clone(&c.posting_cache),
            Arc::clone(&c.neg_cache),
            Arc::clone(&c.regex_pool),
            Some(delta_path),
        );
        match (&c.resource_guard, c.resource_guard_ceiling) {
            (Some(guard), Some(ceiling)) => {
                exec.with_resource_guard_and_ceiling(Arc::clone(guard), ceiling)
            }
            (Some(guard), None) => exec.with_resource_guard(Arc::clone(guard)),
            (None, _) => exec,
        }
    } else {
        let mut e = Executor::new(reader);
        e.set_delta_path(delta_path);
        e
    }
}

pub(crate) fn execute_search_inner(
    root: &Path,
    query: &SearchQuery,
    caches: Option<&SearchCaches>,
) -> std::result::Result<SearchResults, Box<dyn std::error::Error + Send + Sync>> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let resolved_sp = resolve_search_path(root, &canonical_root, query.search_path.as_ref());
    let is_scoped = resolved_sp
        .as_ref()
        .is_some_and(|sp| sp != &canonical_root && sp != root);

    let (reader, delta_path) = open_reader_and_delta(root)?;
    let mut executor = make_executor(&reader, delta_path, caches);
    let (plan, options) = build_plan_and_options(query, executor.regex_pool(), is_scoped)?;

    let (matches, mut stats) = executor.execute(&plan, &options)?;
    let filtered_matches = if let Some(ref sp) = resolved_sp {
        let mut filtered = filter_matches_by_path(matches, root, &canonical_root, sp);
        if is_scoped {
            if query.files_only {
                let mut seen = std::collections::HashSet::new();
                filtered.retain(|m| seen.insert(m.file_path.clone()));
            }
            stats.total_available = u32::try_from(filtered.len()).unwrap_or(u32::MAX);
            if query.max_results > 0 && filtered.len() > query.max_results {
                filtered.truncate(query.max_results);
            }
        }
        stats.total_matches = u32::try_from(filtered.len()).unwrap_or(u32::MAX);
        filtered
    } else {
        matches
    };

    let error = format_verify_error(&stats);
    Ok(SearchResults {
        id: query.id,
        matches: filtered_matches,
        stats,
        error,
        done: true,
        batch: 0,
    })
}

pub(crate) fn execute_search_progressive_inner(
    root: &Path,
    query: &SearchQuery,
    sender: &std::sync::mpsc::Sender<SearchResults>,
    caches: Option<&SearchCaches>,
) -> std::result::Result<QueryStats, Box<dyn std::error::Error + Send + Sync>> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let resolved_sp = resolve_search_path(root, &canonical_root, query.search_path.as_ref());
    let is_scoped = resolved_sp
        .as_ref()
        .is_some_and(|sp| sp != &canonical_root && sp != root);

    let (reader, delta_path) = open_reader_and_delta(root)?;
    let mut executor = make_executor(&reader, delta_path, caches);
    let (plan, options) = build_plan_and_options(query, executor.regex_pool(), is_scoped)?;

    let (prog_sender, prog_receiver) = std::sync::mpsc::channel::<ProgressiveBatch>();
    let mut stats = executor.execute_progressive(&plan, &options, prog_sender)?;

    let raw_batches: Vec<ProgressiveBatch> = prog_receiver.iter().collect();
    let mut filtered_batches: Vec<Vec<Match>> = if let Some(ref sp) = resolved_sp {
        raw_batches
            .into_iter()
            .map(|b| filter_matches_by_path(b.file_matches, root, &canonical_root, sp))
            .collect()
    } else {
        raw_batches.into_iter().map(|b| b.file_matches).collect()
    };

    if is_scoped {
        if query.files_only {
            let mut seen = std::collections::HashSet::new();
            for batch in &mut filtered_batches {
                batch.retain(|m| seen.insert(m.file_path.clone()));
            }
        }
        let total_avail: usize = filtered_batches.iter().map(Vec::len).sum();
        stats.total_available = u32::try_from(total_avail).unwrap_or(u32::MAX);
        let mut emitted = 0usize;
        if query.max_results > 0 {
            let mut remaining = query.max_results;
            for batch in &mut filtered_batches {
                if batch.len() > remaining {
                    batch.truncate(remaining);
                }
                remaining = remaining.saturating_sub(batch.len());
                emitted = emitted.saturating_add(batch.len());
            }
        } else {
            emitted = total_avail;
        }
        stats.total_matches = u32::try_from(emitted).unwrap_or(u32::MAX);
    }

    let error = format_verify_error(&stats);
    if filtered_batches.is_empty() {
        if sender
            .send(SearchResults {
                id: query.id,
                matches: Vec::new(),
                stats: stats.clone(),
                error,
                done: true,
                batch: 0,
            })
            .is_err()
        {
            tracing::debug!("progressive search: receiver closed (client disconnected)");
        }
        return Ok(stats);
    }

    let total_batches = filtered_batches.len();
    for (idx, matches) in filtered_batches.into_iter().enumerate() {
        let is_last = idx + 1 == total_batches;
        let batch_num = u32::try_from(idx).unwrap_or(u32::MAX);
        if sender
            .send(SearchResults {
                id: query.id,
                matches,
                stats: stats.clone(),
                error: if is_last { error.clone() } else { None },
                done: is_last,
                batch: batch_num,
            })
            .is_err()
        {
            tracing::debug!("progressive search: receiver closed (client disconnected)");
            break;
        }
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::Builder;
    use std::fs;

    fn base_query(pattern: &str, search_path: Option<PathBuf>, max_results: usize) -> SearchQuery {
        SearchQuery {
            id: 7,
            pattern: pattern.to_string(),
            is_regex: false,
            ignore_case: false,
            word_boundary: false,
            max_results,
            context_lines: 0,
            file_types: Vec::new(),
            decompress: false,
            multiline: false,
            archive: false,
            binary: false,
            search_path,
            progressive: false,
            chunk_size_bytes: 0,
            chunk_overlap_bytes: 0,
            count_only: false,
            files_only: false,
        }
    }

    #[test]
    fn scoped_search_applies_max_results_after_filtering() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("aaa")).expect("mkdir aaa");
        fs::create_dir_all(root.join("sub")).expect("mkdir sub");

        fs::write(root.join("aaa/one.rs"), "needle\nneedle\n").expect("write");
        fs::write(root.join("sub/two.rs"), "needle\n").expect("write");
        fs::write(root.join("sub/three.rs"), "needle\n").expect("write");

        let mut builder = Builder::new(root).expect("builder");
        builder.build().expect("build");

        let query = base_query("needle", Some(root.join("sub")), 1);
        let res = execute_search(root, &query).expect("execute_search");
        assert_eq!(res.matches.len(), 1);
        assert_eq!(res.stats.total_matches, 1);
        assert_eq!(res.stats.total_available, 2);
        assert!(res.matches[0].file_path.to_string_lossy().contains("sub"));
    }

    #[test]
    fn progressive_scoped_search_updates_stats_and_sets_done() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("other")).expect("mkdir other");
        fs::create_dir_all(root.join("target_dir")).expect("mkdir target_dir");

        fs::write(root.join("other/a.rs"), "needle\n").expect("write");
        fs::write(root.join("target_dir/b.rs"), "needle\n").expect("write");
        fs::write(root.join("target_dir/c.rs"), "needle\n").expect("write");

        let mut builder = Builder::new(root).expect("builder");
        builder.build().expect("build");

        let mut query = base_query("needle", Some(root.join("target_dir")), 1);
        query.progressive = true;
        let (tx, rx) = std::sync::mpsc::channel();
        let stats = execute_search_progressive(root, &query, &tx).expect("progressive");
        drop(tx);

        let batches: Vec<_> = rx.iter().collect();
        assert!(!batches.is_empty());
        assert!(batches.last().expect("last batch").done);
        let total_matches: usize = batches.iter().map(|b| b.matches.len()).sum();
        assert_eq!(total_matches, 1);
        assert_eq!(stats.total_matches, 1);
        assert_eq!(stats.total_available, 2);
    }
}
