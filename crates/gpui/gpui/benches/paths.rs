use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use gpui::benchmark::{PathPackingBenchmark, PathTransformBenchmark};
use gpui::{PathBuilder, point, px};
use std::f32::consts::TAU;

const PATH_SEGMENT_COUNTS: [usize; 3] = [64, 512, 4_096];

fn tessellate_closed_path(segment_count: usize) {
    let mut path = PathBuilder::fill();
    path.move_to(point(px(512.0), px(128.0)));

    for segment in 1..segment_count {
        let angle = segment as f32 / segment_count as f32 * TAU;
        let radius = if segment % 2 == 0 { 384.0 } else { 192.0 };
        path.line_to(point(
            px(512.0 + angle.cos() * radius),
            px(512.0 + angle.sin() * radius),
        ));
    }

    path.close();
    black_box(
        path.build()
            .expect("the deterministic benchmark path is valid"),
    );
}

fn paths(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("path_tessellation");
    group.sample_size(40);

    for segment_count in PATH_SEGMENT_COUNTS {
        group.throughput(Throughput::Elements(segment_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(segment_count),
            &segment_count,
            |bencher, &segment_count| {
                bencher.iter(|| tessellate_closed_path(black_box(segment_count)));
            },
        );
    }

    group.finish();

    let mut transform = criterion.benchmark_group("path_transform");
    transform.sample_size(40);
    for requested_vertex_count in PATH_SEGMENT_COUNTS {
        transform.throughput(Throughput::Elements(requested_vertex_count as u64));
        transform.bench_with_input(
            BenchmarkId::new("scalar", requested_vertex_count),
            &requested_vertex_count,
            |bencher, &requested_vertex_count| {
                let path = PathTransformBenchmark::new(requested_vertex_count);
                bencher.iter(|| black_box(path.transform_scalar()));
            },
        );
        transform.bench_with_input(
            BenchmarkId::new("fearless_simd", requested_vertex_count),
            &requested_vertex_count,
            |bencher, &requested_vertex_count| {
                let path = PathTransformBenchmark::new(requested_vertex_count);
                bencher.iter(|| black_box(path.transform_simd()));
            },
        );
    }
    transform.finish();

    let mut packing = criterion.benchmark_group("path_packing_cache_miss");
    packing.sample_size(40);
    for requested_vertex_count in PATH_SEGMENT_COUNTS {
        let mut path = PathPackingBenchmark::new(requested_vertex_count);
        packing.throughput(Throughput::Elements(requested_vertex_count as u64));
        packing.bench_with_input(
            BenchmarkId::from_parameter(requested_vertex_count),
            &requested_vertex_count,
            |bencher, _| bencher.iter(|| black_box(path.encode_cache_miss())),
        );
    }
    packing.finish();
}

criterion_group!(gpui_paths, paths);
criterion_main!(gpui_paths);
