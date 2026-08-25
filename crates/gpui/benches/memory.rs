use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use gpui::benchmark::BitmapPoolBenchmark;

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
    for (name, capacities) in workloads {
        let mut pool = BitmapPoolBenchmark::new(64 * 1024 * 1024, 8 * 1024 * 1024);
        let retained = pool.cycle(&capacities);
        if name == "dense_1m_to_1_125m" {
            assert!(retained.0 <= 40 * 1024 * 1024);
        }
        black_box(retained);
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
}

criterion_group!(gpui_memory, memory);
criterion_main!(gpui_memory);
