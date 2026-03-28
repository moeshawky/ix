use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::fs;
use tempfile::tempdir;
use ix::trigram::Extractor;
use ix::posting::{PostingList, PostingEntry};
use ix::builder::Builder;
use ix::reader::Reader;
use ix::executor::Executor;
use ix::planner::Planner;

fn bench_trigram_extraction(c: &mut Criterion) {
    let mut extractor = Extractor::new();
    let data = vec![b'a'; 1024 * 1024]; // 1MB of 'a's

    c.bench_function("trigram_extraction_1mb", |b| {
        b.iter(|| {
            extractor.extract_with_offsets(black_box(&data));
        })
    });
}

fn bench_posting_decode(c: &mut Criterion) {
    let mut entries = Vec::new();
    for i in 0..1000 {
        entries.push(PostingEntry {
            file_id: i,
            offsets: vec![1, 10, 100, 1000],
        });
    }
    let list = PostingList { entries };
    let encoded = list.encode();

    c.bench_function("posting_decode_1000_files", |b| {
        b.iter(|| {
            PostingList::decode(black_box(&encoded)).unwrap();
        })
    });
}

fn bench_search(c: &mut Criterion) {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create 100 files of 10KB each
    for i in 0..100 {
        let content = "hello world\n".repeat(800); // ~10KB
        fs::write(root.join(format!("file_{}.txt", i)), content).unwrap();
    }

    let mut builder = Builder::new(root);
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let executor = Executor::new(&reader);
    let plan = Planner::plan("hello", false);

    c.bench_function("search_100_files_1mb_total", |b| {
        b.iter(|| {
            executor.execute(black_box(&plan)).unwrap();
        })
    });
}

criterion_group!(benches, bench_trigram_extraction, bench_posting_decode, bench_search);
criterion_main!(benches);
