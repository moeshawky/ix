//! C5: Resource Contention — Concurrent reader safety
//!
//! Multiple threads reading the same index must produce identical results.
//! The index is read-only after build — contention is on the cache and
//! memory-mapped file views, not the file itself.

use ix::builder::Builder;
use ix::executor::{Executor, QueryOptions};
use ix::planner::Planner;
use ix::reader::Reader;
use std::fs;
use std::sync::Arc;
use std::thread;
use tempfile::tempdir;

#[test]
fn test_c5_concurrent_readers_same_index() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    let file_names: Vec<String> = (0..20).map(|i| format!("file_{i:02}.txt")).collect();

    for (i, name) in file_names.iter().enumerate() {
        let content = match i % 5 {
            0 => "alpha needle gamma",
            1 => "beta needle delta",
            2 => "epsilon needle zeta",
            3 => "eta needle theta",
            _ => "iota needle kappa",
        };
        fs::write(root.join(name), content).unwrap();
    }

    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Arc::new(Reader::open(&index_path).unwrap());

    let queries = vec![
        ("alpha", false),
        ("beta", false),
        ("needle", false),
        ("theta", false),
    ];
    let query_count = queries.len();

    let mut handles = Vec::new();

    for thread_id in 0..8 {
        let reader = Arc::clone(&reader);
        let queries = queries.clone();
        let handle = thread::spawn(move || {
            let mut executor = Executor::new(&reader);
            let mut results = Vec::new();

            for &(pattern, is_regex) in &queries {
                let plan = Planner::plan(pattern, is_regex);
                let (matches, _) = executor
                    .execute(&plan, &QueryOptions::default())
                    .expect("C5: Query must not fail under concurrency");
                results.push((pattern, matches.len()));
            }

            (thread_id, results)
        });
        handles.push(handle);
    }

    // Collect results — all threads must agree on match counts
    let mut all_results: Vec<(usize, Vec<(&str, usize)>)> = Vec::new();
    for handle in handles {
        match handle.join() {
            Ok(result) => all_results.push(result),
            Err(e) => {
                if let Some(msg) = e.downcast_ref::<String>() {
                    panic!("C5: Thread panicked: {msg}");
                }
                panic!("C5: Thread panicked with unknown payload");
            }
        }
    }

    // Verify consensus across all threads
    let reference = &all_results[0].1;
    for (tid, results) in &all_results[1..] {
        for (q, (r_pattern, r_count)) in results.iter().zip(reference.iter()) {
            assert_eq!(q.0, *r_pattern, "C5: Thread {tid} query order mismatch");
            assert_eq!(
                q.1, *r_count,
                "C5: Thread {tid} found {} matches for '{}' but thread 0 found {}",
                q.1, q.0, r_count
            );
        }
    }

    assert_eq!(all_results.len(), 8, "C5: All 8 threads must complete");
    assert_eq!(
        all_results[0].1.len(),
        query_count,
        "C5: Each thread must process all queries"
    );
}

#[test]
fn test_c5_parallel_cache_does_not_corrupt_results() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    for i in 0..10 {
        fs::write(
            root.join(format!("f{i}.txt")),
            "common_term unique_term specific_match_found",
        )
        .unwrap();
    }

    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");

    // Run the same query 100 times in parallel — cache must not diverge
    let mut handles = Vec::new();
    for _ in 0..8 {
        let idx = index_path.clone();
        let handle = thread::spawn(move || {
            let reader = Reader::open(&idx).unwrap();
            let mut executor = Executor::new(&reader);
            let mut results = Vec::new();
            for i in 0..50 {
                let plan = Planner::plan("specific_match_found", false);
                let (matches, _) = executor
                    .execute(&plan, &QueryOptions::default())
                    .expect("C5: Repeated query must not fail");
                results.push((i, matches.len()));
            }
            results
        });
        handles.push(handle);
    }

    for handle in handles {
        let results = handle.join().unwrap();
        for (iteration, count) in &results {
            assert_eq!(
                *count, 10,
                "C5: Iteration {iteration} found {count} matches (expected 10)"
            );
        }
    }
}
