use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use gpui::{TestAppContext, benchmark::LayoutBenchmark};

const NODE_COUNTS: [usize; 3] = [128, 1_024, 4_096];

fn layouts(criterion: &mut Criterion) {
    let mut test_context = TestAppContext::single();
    let context = test_context.add_empty_window();

    let mut cold = criterion.benchmark_group("layout/cold_flat_tree");
    cold.sample_size(20);
    for node_count in NODE_COUNTS {
        cold.throughput(Throughput::Elements(node_count as u64));
        cold.bench_with_input(
            BenchmarkId::from_parameter(node_count),
            &node_count,
            |bencher, &node_count| {
                bencher.iter(|| {
                    let mut layout = LayoutBenchmark::new();
                    black_box(layout.flat_tree(black_box(node_count), context));
                });
            },
        );
    }
    cold.finish();

    let mut retained = criterion.benchmark_group("layout/retained_flat_tree");
    retained.sample_size(20);
    for node_count in NODE_COUNTS {
        let mut layout = LayoutBenchmark::new();
        black_box(layout.flat_tree(node_count, context));
        retained.throughput(Throughput::Elements(node_count as u64));
        retained.bench_with_input(
            BenchmarkId::from_parameter(node_count),
            &node_count,
            |bencher, &node_count| {
                bencher.iter(|| {
                    layout.next_frame();
                    black_box(layout.flat_tree(black_box(node_count), context));
                });
            },
        );
    }
    retained.finish();
}

criterion_group!(gpui_layouts, layouts);
criterion_main!(gpui_layouts);
