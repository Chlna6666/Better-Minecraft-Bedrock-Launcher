use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use gpui::{
    PerformanceMetricsSnapshot, Pixels, SharedString, TestAppContext, VisualTestContext,
    benchmark::LayoutBenchmark, performance_metrics_snapshot, px,
};

const NODE_COUNTS: [usize; 3] = [128, 1_024, 4_096];
const TEXT_LINE_COUNTS: [usize; 3] = [256, 2_048, 8_192];
const TEXT_SCAN_LINE_COUNT: usize = 16_384;
const TEXT_HOT_LINE_COUNT: usize = 1_024;

#[derive(Clone, Copy, Debug)]
struct TextCacheSample {
    requests: usize,
    hits: usize,
    reuses: usize,
    misses: usize,
    hit_rate_percent: f32,
    reuse_rate_percent: f32,
    cache_rate_percent: f32,
}

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

    text_layout_cache(criterion);
}

fn text_layout_cache(criterion: &mut Criterion) {
    let mut test_context = TestAppContext::single();
    let mut context = test_context.add_empty_window();

    let mut stable = criterion.benchmark_group("text_cache/stable_ide_hotset");
    stable.sample_size(20);
    for line_count in TEXT_LINE_COUNTS {
        let lines = make_code_lines(line_count, 0);
        warm_text_cache(&mut context, &lines, None, 2);
        stable.throughput(Throughput::Elements(line_count as u64));
        stable.bench_with_input(
            BenchmarkId::from_parameter(line_count),
            &line_count,
            |bencher, _| {
                bencher.iter(|| {
                    let sample = shape_text_frame(&mut context, black_box(&lines), None);
                    black_box((
                        sample.requests,
                        sample.hits,
                        sample.reuses,
                        sample.misses,
                        sample.cache_rate_percent,
                    ));
                });
            },
        );
    }
    stable.finish();

    let mut duplicate = criterion.benchmark_group("text_cache/same_frame_duplicates");
    duplicate.sample_size(20);
    for line_count in [256, 1_024, 4_096] {
        let unique_lines = make_code_lines(line_count, 10_000);
        let duplicated_lines = duplicate_lines(&unique_lines);
        duplicate.throughput(Throughput::Elements(duplicated_lines.len() as u64));
        duplicate.bench_with_input(
            BenchmarkId::from_parameter(duplicated_lines.len()),
            &duplicated_lines.len(),
            |bencher, _| {
                bencher.iter(|| {
                    let sample = shape_text_frame(&mut context, black_box(&duplicated_lines), None);
                    black_box((
                        sample.hit_rate_percent,
                        sample.reuse_rate_percent,
                        sample.cache_rate_percent,
                    ));
                });
            },
        );
    }
    duplicate.finish();

    let mut scan = criterion.benchmark_group("text_cache/scan_pollution_recovery");
    scan.sample_size(10);
    let hot_lines = make_code_lines(TEXT_HOT_LINE_COUNT, 100_000);
    let scan_chunks = (0..4)
        .map(|chunk| make_code_lines(TEXT_SCAN_LINE_COUNT, 200_000 + chunk * TEXT_SCAN_LINE_COUNT))
        .collect::<Vec<_>>();
    warm_text_cache(&mut context, &hot_lines, None, 3);
    let mut scan_index = 0usize;
    scan.throughput(Throughput::Elements(
        (TEXT_SCAN_LINE_COUNT + TEXT_HOT_LINE_COUNT) as u64,
    ));
    scan.bench_function("hotset_after_one_shot_scan", |bencher| {
        bencher.iter(|| {
            let scan_lines = &scan_chunks[scan_index % scan_chunks.len()];
            scan_index = scan_index.wrapping_add(1);
            let scan_sample = shape_text_frame(&mut context, black_box(scan_lines), None);
            let hot_sample = shape_text_frame(&mut context, black_box(&hot_lines), None);
            black_box((
                scan_sample.cache_rate_percent,
                hot_sample.cache_rate_percent,
                hot_sample.reuse_rate_percent,
            ));
        });
    });
    scan.finish();

    let mut wrapped = criterion.benchmark_group("text_cache/wrapped_ide_hotset");
    wrapped.sample_size(20);
    for line_count in [256, 1_024, 4_096] {
        let lines = make_wrapped_lines(line_count, 300_000);
        warm_text_cache(&mut context, &lines, Some(px(420.0)), 2);
        wrapped.throughput(Throughput::Elements(line_count as u64));
        wrapped.bench_with_input(
            BenchmarkId::from_parameter(line_count),
            &line_count,
            |bencher, _| {
                bencher.iter(|| {
                    let sample = shape_text_frame(&mut context, black_box(&lines), Some(px(420.0)));
                    black_box((
                        sample.requests,
                        sample.reuses,
                        sample.cache_rate_percent,
                    ));
                });
            },
        );
    }
    wrapped.finish();
}

fn warm_text_cache(
    context: &mut VisualTestContext,
    lines: &[String],
    wrap_width: Option<Pixels>,
    frames: usize,
) {
    for _ in 0..frames {
        black_box(shape_text_frame(context, lines, wrap_width));
    }
}

fn shape_text_frame(
    context: &mut VisualTestContext,
    lines: &[String],
    wrap_width: Option<Pixels>,
) -> TextCacheSample {
    context.update(|window, cx| {
        let font_size = px(14.0);
        for line in lines {
            let run = window.text_style().to_run(line.len());
            let wrapped = window
                .text_system()
                .shape_text(
                    SharedString::new(line.as_str()),
                    font_size,
                    &[run],
                    wrap_width,
                    None,
                )
                .expect("text cache benchmark should shape synthetic text");
            black_box(wrapped);
        }
        let _ = window.draw(cx);
    });

    text_cache_sample_from_snapshot(performance_metrics_snapshot())
}

fn text_cache_sample_from_snapshot(snapshot: PerformanceMetricsSnapshot) -> TextCacheSample {
    let requests = snapshot
        .text_layout_hits
        .saturating_add(snapshot.text_layout_reuses)
        .saturating_add(snapshot.text_layout_misses);
    let percent = |count: usize| {
        if requests == 0 {
            100.0
        } else {
            (count as f32 * 100.0) / requests as f32
        }
    };
    let cache_hits = snapshot
        .text_layout_hits
        .saturating_add(snapshot.text_layout_reuses);

    TextCacheSample {
        requests,
        hits: snapshot.text_layout_hits,
        reuses: snapshot.text_layout_reuses,
        misses: snapshot.text_layout_misses,
        hit_rate_percent: percent(snapshot.text_layout_hits),
        reuse_rate_percent: percent(snapshot.text_layout_reuses),
        cache_rate_percent: percent(cache_hits),
    }
}

fn make_code_lines(line_count: usize, seed: usize) -> Vec<String> {
    (0..line_count)
        .map(|line| {
            let id = seed + line;
            format!(
                "fn cache_case_{id:06}() -> usize {{ let value = {id}; value.rotate_left({}); }} // 文本缓存 العربية עברית ไทย देवनागरी",
                id % 31,
            )
        })
        .collect()
}

fn make_wrapped_lines(line_count: usize, seed: usize) -> Vec<String> {
    (0..line_count)
        .map(|line| {
            let id = seed + line;
            format!(
                "cache paragraph {id:06}: retained layout should preserve shaped glyph runs across editor scrollback, diff panels, logs, 中文 fallback, العربية shaping, עברית bidi, ไทย wrapping, and देवनागरी clusters without fixed line-count eviction."
            )
        })
        .collect()
}

fn duplicate_lines(lines: &[String]) -> Vec<String> {
    let mut duplicated = Vec::with_capacity(lines.len().saturating_mul(2));
    for line in lines {
        duplicated.push(line.clone());
        duplicated.push(line.clone());
    }
    duplicated
}

criterion_group!(gpui_layouts, layouts);
criterion_main!(gpui_layouts);
