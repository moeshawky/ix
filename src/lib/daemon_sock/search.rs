use std::path::Path;
use std::sync::Arc;

use crate::neg_cache::NegCache;
use crate::posting_cache::PostingCache;
use crate::regex_pool::RegexPool;

use super::types::{SearchQuery, SearchResults};

/// Bundled cache handles for daemon-mode search execution.
#[derive(Clone)]
pub(crate) struct SearchCaches {
    pub posting_cache: Arc<PostingCache>,
    pub neg_cache: Arc<NegCache>,
    pub regex_pool: Arc<RegexPool>,
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
) -> std::result::Result<crate::executor::QueryStats, Box<dyn std::error::Error + Send + Sync>> {
    execute_search_progressive_inner(root, query, sender, None)
}

pub(crate) fn execute_search_inner(
    root: &Path,
    query: &SearchQuery,
    caches: Option<&SearchCaches>,
) -> std::result::Result<SearchResults, Box<dyn std::error::Error + Send + Sync>> {
    use crate::executor::{Executor, QueryOptions};
    use crate::planner::Planner;
    use crate::reader::Reader;

    let index_dir = root.join(".ix");
    let index_path = index_dir.join("shard.ix");
    if !index_path.exists() {
        return Err("index not found".into());
    }

    let reader = Reader::open(&index_path)?;
    let delta_path = index_dir.join("shard.ix.delta");

    let mut executor = if let Some(c) = caches {
        Executor::new_with_caches(
            &reader,
            Arc::clone(&c.posting_cache),
            Arc::clone(&c.neg_cache),
            Arc::clone(&c.regex_pool),
            Some(delta_path),
        )
    } else {
        let mut e = Executor::new(&reader);
        e.set_delta_path(delta_path);
        e
    };

    let plan = Planner::plan_with_pool(
        &query.pattern,
        crate::planner::QueryOptions {
            is_regex: query.is_regex,
            ignore_case: query.ignore_case,
            multiline: query.multiline,
            word_boundary: query.word_boundary,
        },
        executor.regex_pool(),
    )?;

    let options = QueryOptions {
        count_only: false,
        files_only: false,
        max_results: query.max_results,
        type_filter: query.file_types.clone(),
        context_lines: query.context_lines,
        decompress: query.decompress,

        multiline: query.multiline,
        archive: query.archive,
        binary: query.binary,
        word_boundary: query.word_boundary,
        chunk_size_bytes: query.chunk_size_bytes,
        chunk_overlap_bytes: query.chunk_overlap_bytes,
    };

    let (matches, mut stats) = executor.execute(&plan, &options)?;
    let filtered_matches: Vec<_> = if let Some(ref search_path) = query.search_path {
        let filtered: Vec<_> = matches
            .into_iter()
            .filter(|m| {
                let abs_path = if m.file_path.is_absolute() {
                    m.file_path.clone()
                } else {
                    root.join(&m.file_path)
                };
                abs_path.starts_with(search_path)
            })
            .collect();
        stats.total_matches = filtered.len() as u32;
        filtered
    } else {
        matches
    };
    let error = if stats.files_failed_verify > 0 {
        Some(format!(
            "{} file(s) could not be verified (I/O error) \u{2014} results may be incomplete",
            stats.files_failed_verify
        ))
    } else {
        None
    };
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
) -> std::result::Result<crate::executor::QueryStats, Box<dyn std::error::Error + Send + Sync>> {
    use crate::executor::{Executor, ProgressiveBatch, QueryOptions};
    use crate::planner::Planner;
    use crate::reader::Reader;

    let index_dir = root.join(".ix");
    let index_path = index_dir.join("shard.ix");
    if !index_path.exists() {
        return Err("index not found".into());
    }

    let reader = Reader::open(&index_path)?;
    let delta_path = index_dir.join("shard.ix.delta");

    let mut executor = if let Some(c) = caches {
        Executor::new_with_caches(
            &reader,
            Arc::clone(&c.posting_cache),
            Arc::clone(&c.neg_cache),
            Arc::clone(&c.regex_pool),
            Some(delta_path),
        )
    } else {
        let mut e = Executor::new(&reader);
        e.set_delta_path(delta_path);
        e
    };

    let plan = Planner::plan_with_pool(
        &query.pattern,
        crate::planner::QueryOptions {
            is_regex: query.is_regex,
            ignore_case: query.ignore_case,
            multiline: query.multiline,
            word_boundary: query.word_boundary,
        },
        executor.regex_pool(),
    )?;

    let options = QueryOptions {
        count_only: false,
        files_only: false,
        max_results: query.max_results,
        type_filter: query.file_types.clone(),
        context_lines: query.context_lines,
        decompress: query.decompress,

        multiline: query.multiline,
        archive: query.archive,
        binary: query.binary,
        word_boundary: query.word_boundary,
        chunk_size_bytes: query.chunk_size_bytes,
        chunk_overlap_bytes: query.chunk_overlap_bytes,
    };

    let (prog_sender, prog_receiver) = std::sync::mpsc::channel::<ProgressiveBatch>();
    let stats = executor.execute_progressive(&plan, &options, prog_sender)?;

    let mut batch_num = 0u32;
    while let Ok(batch) = prog_receiver.recv() {
        let filtered_matches: Vec<_> = if let Some(ref search_path) = query.search_path {
            batch
                .file_matches
                .into_iter()
                .filter(|m| {
                    let abs_path = if m.file_path.is_absolute() {
                        m.file_path.clone()
                    } else {
                        root.join(&m.file_path)
                    };
                    abs_path.starts_with(search_path)
                })
                .collect()
        } else {
            batch.file_matches
        };
        let is_last = batch_num == 0;
        let error = if stats.files_failed_verify > 0 {
            Some(format!(
                "{} file(s) could not be verified (I/O error) \u{2014} results may be incomplete",
                stats.files_failed_verify
            ))
        } else {
            None
        };
        if sender
            .send(SearchResults {
                id: query.id,
                matches: filtered_matches,
                stats: stats.clone(),
                error,
                done: is_last,
                batch: batch_num,
            })
            .is_err()
        {
            tracing::debug!("progressive search: receiver closed (client disconnected)");
        }
        batch_num += 1;
        if is_last {
            break;
        }
    }

    Ok(stats)
}
