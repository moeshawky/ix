use criterion::{Criterion, black_box, criterion_group, criterion_main};
use ix::builder::Builder;
use ix::executor::Executor;
use ix::planner::Planner;
use ix::reader::Reader;
use std::fs;
use tempfile::tempdir;

fn bench_search_with_context(c: &mut Criterion) {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create 10 files of 1MB each
    for i in 0..10 {
        let mut content = String::new();
        for j in 0..10000 {
            if j == 5000 {
                content.push_str("hello world\n");
            } else {
                content.push_str(&format!("this is a normal line {}\n", j));
            }
        }
        fs::write(root.join(format!("file_{}.txt", i)), content).unwrap();
    }

    let mut builder = Builder::new(root).unwrap();
    builder.build().unwrap();

    let index_path = root.join(".ix/shard.ix");
    let reader = Reader::open(&index_path).unwrap();
    let mut executor = Executor::new(&reader);
    let plan = Planner::plan("hello", false);
    let options = ix::executor::QueryOptions {
        context_lines: 3,
        ..Default::default()
    };

    c.bench_function("search_with_context_lines", |b| {
        b.iter(|| {
            executor.execute(black_box(&plan), &options).unwrap();
        })
    });
}

criterion_group!(benches, bench_search_with_context);
criterion_main!(benches);
