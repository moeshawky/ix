use criterion::{black_box, criterion_group, criterion_main, Criterion};
use llmosafe::{sift_perceptions, calculate_halo_signal, Synapse, WorkingMemory};

fn bench_sift_perceptions(c: &mut Criterion) {
    let objective = "Safety-Critical AI";
    let observations_10 = vec!["Stable observation"; 10];
    let observations_100 = vec!["Stable observation"; 100];
    let observations_1000 = vec!["Stable observation"; 1000];

    let mut group = c.benchmark_group("sift_perceptions");
    group.bench_function("10_obs", |b| b.iter(|| sift_perceptions(black_box(&observations_10), black_box(objective))));
    group.bench_function("100_obs", |b| b.iter(|| sift_perceptions(black_box(&observations_100), black_box(objective))));
    group.bench_function("1000_obs", |b| b.iter(|| sift_perceptions(black_box(&observations_1000), black_box(objective))));
    group.finish();
}

fn bench_calculate_halo_signal(c: &mut Criterion) {
    let clean_text = "This is a clean sentence with no bias keywords.";
    let all_bias_text = "expert official popular trending limited exclusive now today love joy sophisticated advanced";
    let mixed_text = "The expert says this is a normal observation that is popular.";

    let mut group = c.benchmark_group("calculate_halo_signal");
    group.bench_function("clean", |b| b.iter(|| calculate_halo_signal(black_box(clean_text))));
    group.bench_function("all_bias", |b| b.iter(|| calculate_halo_signal(black_box(all_bias_text))));
    group.bench_function("mixed", |b| b.iter(|| calculate_halo_signal(black_box(mixed_text))));
    group.finish();
}

fn bench_synapse_validate(c: &mut Criterion) {
    let valid = Synapse::from_raw_u64(400);
    let biased = {
        let mut s = Synapse::new();
        s.set_has_bias(true);
        s
    };
    let unstable = Synapse::from_raw_u64(1500);

    let mut group = c.benchmark_group("synapse_validate");
    group.bench_function("valid", |b| b.iter(|| black_box(valid).validate()));
    group.bench_function("biased", |b| b.iter(|| black_box(biased).validate()));
    group.bench_function("unstable", |b| b.iter(|| black_box(unstable).validate()));
    group.finish();
}

fn bench_working_memory_update(c: &mut Criterion) {
    let mut memory = WorkingMemory::<1024>::new(1000);
    let synapse = Synapse::from_raw_u64(400);

    c.bench_function("working_memory_update_1000", |b| b.iter(|| {
        for _ in 0..1000 {
            let _ = memory.update(black_box(synapse));
        }
    }));
}

criterion_group!(benches, bench_sift_perceptions, bench_calculate_halo_signal, bench_synapse_validate, bench_working_memory_update);
criterion_main!(benches);
