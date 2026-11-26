use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use gpui::benchmark::{BitmapPoolBenchmark, BitmapPoolBenchmarkSample};

const OPERATION_COUNT: usize = 64;

fn memory(criterion: &mut Criterion) {
    let uniform = vec![64 * 1024; OPERATION_COUNT];
    let mixed = (0..OPERATION_COUNT)
        .map(|index| 1usize << (8 + index % 13))
        .collect::<Vec<_>>();
    let dense_large = (0..32)
        .map(|index| 1024 * 1024 + index * 4 * 1024 + 1)
        .collect::<Vec<_>>();
    let workloads = [
        ("uniform_64k", uniform),
        ("mixed_256b_to_1m", mixed),
        ("dense_1m_to_1_125m", dense_large),
    ];

    let mut group = criterion.benchmark_group("bitmap_pool/steady_state");
    group.sample_size(30);
    for (name, capacities) in &workloads {
        let mut pool = BitmapPoolBenchmark::new(64 * 1024 * 1024);
        let retained = pool.cycle(capacities);
        assert_eq!(retained.requested_bytes, capacities.iter().sum::<usize>());
        assert!(retained.acquired_capacity_bytes >= retained.requested_bytes);
        let warm = pool.cycle(capacities);
        assert_eq!(warm.staging_capacity_bytes, retained.staging_capacity_bytes);
        if *name == "dense_1m_to_1_125m" {
            assert!(warm.retained_bytes <= 40 * 1024 * 1024);
        }
        report(name, "warm", warm);
        group.throughput(Throughput::Elements(capacities.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(name),
            &capacities,
            |bencher, capacities| {
                bencher.iter(|| black_box(pool.cycle(black_box(capacities))));
            },
        );
    }
    group.finish();
    cold_and_trim(criterion, &workloads);
    working_set_recovery(criterion);
    working_set_switch(criterion);
}

fn cold_and_trim(criterion: &mut Criterion, workloads: &[(&str, Vec<usize>)]) {
    for operation in ["cold_after_trim", "trim_reclaim"] {
        let mut group = criterion.benchmark_group(format!("bitmap_pool/{operation}"));
        group.sample_size(30);
        for (name, capacities) in workloads {
            let mut pool = BitmapPoolBenchmark::new(64 * 1024 * 1024);
            pool.cycle(capacities);
            let empty = pool.trim(0);
            assert_eq!(empty.retained_bytes, 0);
            assert_eq!(empty.free_buffers, 0);
            assert_eq!(empty.staging_capacity_bytes, 0);
            report(name, "trimmed", empty);
            group.bench_with_input(
                BenchmarkId::from_parameter(name),
                capacities,
                |bencher, capacities| {
                    bencher.iter_batched(
                        || {
                            let mut pool = BitmapPoolBenchmark::new(64 * 1024 * 1024);
                            if operation == "trim_reclaim" {
                                pool.cycle(capacities);
                            }
                            pool
                        },
                        |mut pool| {
                            if operation == "cold_after_trim" {
                                black_box(pool.cycle(black_box(capacities)));
                            } else {
                                black_box(pool.trim(0));
                            }
                        },
                        BatchSize::PerIteration,
                    );
                },
            );
        }
        group.finish();
    }
}

fn working_set_recovery(criterion: &mut Criterion) {
    let large = vec![1024 * 1024; 32];
    let small = vec![256; OPERATION_COUNT];
    let mut pool = BitmapPoolBenchmark::new(64 * 1024 * 1024);
    report("large_to_small", "large", pool.cycle(&large));
    report("large_to_small", "small_before_trim", pool.cycle(&small));
    let empty = pool.trim(0);
    assert_eq!(empty.retained_bytes, 0);
    report("large_to_small", "trimmed", empty);
    report("large_to_small", "small_after_trim", pool.cycle(&small));
    let mut group = criterion.benchmark_group("bitmap_pool/working_set_recovery");
    group.sample_size(30);
    group.bench_function("large_small_trim_small", |bencher| {
        bencher.iter(|| {
            black_box(pool.cycle(black_box(&large)));
            black_box(pool.cycle(black_box(&small)));
            black_box(pool.trim(0));
            black_box(pool.cycle(black_box(&small)));
        });
    });
    group.finish();
}

fn working_set_switch(criterion: &mut Criterion) {
    let large = vec![1024 * 1024; 32];
    let mut group = criterion.benchmark_group("bitmap_pool/working_set_switch");
    group.sample_size(30);
    for (name, capacity, count) in [
        ("large_to_tiny", 256, 64),
        ("large_to_half", 512 * 1024, 32),
    ] {
        let small = vec![capacity; count];
        let mut pool = BitmapPoolBenchmark::new(64 * 1024 * 1024);
        pool.cycle(&large);
        report(name, "small_after_large", pool.cycle(&small));
        report(name, "small_warm", pool.cycle(&small));
        group.bench_function(name, |bencher| {
            bencher.iter(|| black_box(pool.cycle(black_box(&small))));
        });
        report(name, "return_to_large", pool.cycle(&large));
    }
    let tiny = vec![256; OPERATION_COUNT];
    let mut pool = BitmapPoolBenchmark::new(64 * 1024 * 1024);
    pool.cycle(&large);
    pool.cycle(&tiny);
    pool.cycle(&large);
    pool.cycle(&tiny);
    group.bench_function("large_tiny_alternating", |bencher| {
        bencher.iter(|| {
            black_box(pool.cycle(black_box(&large)));
            black_box(pool.cycle(black_box(&tiny)));
        });
    });
    group.finish();
}

fn report(case: &str, phase: &str, sample: BitmapPoolBenchmarkSample) {
    println!(
        "{}",
        serde_json::json!({
            "benchmark": "bitmap_pool_capacity", "case": case, "phase": phase,
            "requested_bytes": sample.requested_bytes,
            "acquired_capacity_bytes": sample.acquired_capacity_bytes,
            "retained_bytes": sample.retained_bytes, "free_buffers": sample.free_buffers,
            "staging_capacity_bytes": sample.staging_capacity_bytes,
        })
    );
}

criterion_group!(gpui_memory, memory);
criterion_main!(gpui_memory);
