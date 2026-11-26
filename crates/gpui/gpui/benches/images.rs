use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use gpui::benchmark::{AnimationQueueBenchmark, AnimationStreamBenchmark};
use gpui::image::{
    DynamicImage, ExtendedColorType, ImageEncoder as _, ImageFormat, Rgba, RgbaImage,
    codecs::webp::WebPEncoder,
};
use gpui::{AnimatedImageConfig, EncodedImage, ImageRenderSize, ObjectFit};
use std::{io::Cursor, sync::Arc, time::Duration};

const SOURCE_WIDTH: u32 = 1_920;
const SOURCE_HEIGHT: u32 = 1_080;
const TARGET_SIZES: [(u32, u32); 2] = [(320, 180), (1_280, 720)];
const ANIMATION_WIDTH: u32 = 320;
const ANIMATION_HEIGHT: u32 = 180;
const ANIMATION_FRAME_COUNT: u32 = 24;

fn resident_animation_config(max_resident_frames: usize) -> AnimatedImageConfig {
    let mut config = AnimatedImageConfig::default();
    config.max_resident_frames = max_resident_frames;
    config
}

fn streamed_animation_config() -> AnimatedImageConfig {
    let mut config = AnimatedImageConfig::default();
    config.prefetch_frames = 4;
    config.max_resident_frames = 1;
    config
}

fn fixture(format: ImageFormat) -> Arc<[u8]> {
    let pixels = RgbaImage::from_fn(SOURCE_WIDTH, SOURCE_HEIGHT, |x, y| {
        let mixed = x.wrapping_mul(1_664_525) ^ y.wrapping_mul(1_013_904_223);
        Rgba([
            mixed as u8,
            mixed.rotate_left(9) as u8,
            mixed.rotate_left(17) as u8,
            255,
        ])
    });
    let mut encoded = Cursor::new(Vec::new());
    let image = DynamicImage::ImageRgba8(pixels);
    let image = if format == ImageFormat::Jpeg {
        DynamicImage::ImageRgb8(image.into_rgb8())
    } else {
        image
    };
    image
        .write_to(&mut encoded, format)
        .expect("the deterministic benchmark image can be encoded");
    encoded.into_inner().into()
}

fn animated_webp_fixture() -> Arc<[u8]> {
    let mut webp = Vec::new();
    webp.extend_from_slice(b"WEBP");

    let mut canvas = vec![0x02, 0, 0, 0];
    push_u24(&mut canvas, ANIMATION_WIDTH - 1);
    push_u24(&mut canvas, ANIMATION_HEIGHT - 1);
    push_chunk(&mut webp, *b"VP8X", &canvas);
    push_chunk(&mut webp, *b"ANIM", &[0, 0, 0, 0, 0, 0]);

    for frame_index in 0..ANIMATION_FRAME_COUNT {
        let pixels = RgbaImage::from_fn(ANIMATION_WIDTH, ANIMATION_HEIGHT, |x, y| {
            let mixed = x.wrapping_mul(31) ^ y.wrapping_mul(131) ^ frame_index.wrapping_mul(1_013);
            Rgba([
                mixed as u8,
                mixed.rotate_left(7) as u8,
                mixed.rotate_left(15) as u8,
                255,
            ])
        });
        let mut encoded_frame = Vec::new();
        WebPEncoder::new_lossless(&mut encoded_frame)
            .write_image(
                pixels.as_raw(),
                ANIMATION_WIDTH,
                ANIMATION_HEIGHT,
                ExtendedColorType::Rgba8,
            )
            .expect("the deterministic animation frame can be encoded");
        push_animation_frame(&mut webp, &encoded_frame[12..]);
    }

    let mut riff = Vec::with_capacity(webp.len() + 8);
    riff.extend_from_slice(b"RIFF");
    riff.extend_from_slice(
        &u32::try_from(webp.len())
            .expect("the benchmark WebP length fits u32")
            .to_le_bytes(),
    );
    riff.extend_from_slice(&webp);
    riff.into()
}

fn push_animation_frame(output: &mut Vec<u8>, frame_chunks: &[u8]) {
    let mut frame = Vec::with_capacity(16 + frame_chunks.len());
    push_u24(&mut frame, 0);
    push_u24(&mut frame, 0);
    push_u24(&mut frame, ANIMATION_WIDTH - 1);
    push_u24(&mut frame, ANIMATION_HEIGHT - 1);
    push_u24(&mut frame, 16);
    frame.push(0);
    frame.extend_from_slice(frame_chunks);
    push_chunk(output, *b"ANMF", &frame);
}

fn push_chunk(output: &mut Vec<u8>, kind: [u8; 4], payload: &[u8]) {
    output.extend_from_slice(&kind);
    output.extend_from_slice(
        &u32::try_from(payload.len())
            .expect("the benchmark WebP chunk length fits u32")
            .to_le_bytes(),
    );
    output.extend_from_slice(payload);
    if payload.len() % 2 != 0 {
        output.push(0);
    }
}

fn push_u24(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes()[..3]);
}

fn images(criterion: &mut Criterion) {
    let fixtures = [
        (ImageFormat::Png, fixture(ImageFormat::Png)),
        (ImageFormat::WebP, fixture(ImageFormat::WebP)),
        (ImageFormat::Jpeg, fixture(ImageFormat::Jpeg)),
        (ImageFormat::Bmp, fixture(ImageFormat::Bmp)),
        (ImageFormat::Gif, fixture(ImageFormat::Gif)),
    ];
    let source_pixels = u64::from(SOURCE_WIDTH) * u64::from(SOURCE_HEIGHT);

    let mut full_size = criterion.benchmark_group("image_render/full_size");
    full_size.sample_size(20);
    full_size.throughput(Throughput::Elements(source_pixels));
    for (format, bytes) in &fixtures {
        full_size.bench_with_input(
            BenchmarkId::from_parameter(format.extensions_str()[0]),
            bytes,
            |bencher, bytes| {
                bencher.iter_batched(
                    || EncodedImage::new(*format, Arc::clone(bytes)),
                    |source| {
                        black_box(
                            source
                                .render(AnimatedImageConfig::default())
                                .expect("the benchmark fixture is valid"),
                        );
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
    full_size.finish();

    ownership(criterion);
    batch_png(criterion, &fixtures[0].1);

    let mut resized = criterion.benchmark_group("image_render/contain");
    resized.sample_size(20);
    for (format, bytes) in &fixtures {
        for (width, height) in TARGET_SIZES {
            let target = ImageRenderSize::new(width, height).expect("target size is non-zero");
            resized.throughput(Throughput::Elements(u64::from(width) * u64::from(height)));
            resized.bench_with_input(
                BenchmarkId::new(format.extensions_str()[0], format!("{width}x{height}")),
                bytes,
                |bencher, bytes| {
                    bencher.iter_batched(
                        || EncodedImage::new(*format, Arc::clone(bytes)),
                        |source| {
                            black_box(
                                source
                                    .render_sized(
                                        target,
                                        ObjectFit::Contain,
                                        AnimatedImageConfig::default(),
                                    )
                                    .expect("the benchmark fixture is valid"),
                            );
                        },
                        BatchSize::SmallInput,
                    );
                },
            );
        }
    }
    resized.finish();

    let animated_bytes = animated_webp_fixture();
    let mut animated = criterion.benchmark_group("image_render/animated_webp");
    animated.sample_size(10);
    animated.throughput(Throughput::Elements(
        u64::from(ANIMATION_WIDTH) * u64::from(ANIMATION_HEIGHT) * u64::from(ANIMATION_FRAME_COUNT),
    ));
    animated.bench_function("resident_24_frames_320x180", |bencher| {
        bencher.iter_batched(
            || EncodedImage::new(ImageFormat::WebP, Arc::clone(&animated_bytes)),
            |source| {
                black_box(
                    source
                        .render(resident_animation_config(ANIMATION_FRAME_COUNT as usize))
                        .expect("the benchmark animation is valid"),
                );
            },
            BatchSize::SmallInput,
        );
    });
    animated.bench_function("streamed_24_frames_320x180", |bencher| {
        bencher.iter_batched(
            || EncodedImage::new(ImageFormat::WebP, Arc::clone(&animated_bytes)),
            |source| {
                let mut stream = AnimationStreamBenchmark::new(source, streamed_animation_config())
                    .expect("the benchmark animation is valid");
                black_box(stream.consume(usize::try_from(ANIMATION_FRAME_COUNT - 1).unwrap()));
            },
            BatchSize::SmallInput,
        );
    });
    animated.bench_function("streamed_loop_restart_320x180", |bencher| {
        bencher.iter_batched(
            || EncodedImage::new(ImageFormat::WebP, Arc::clone(&animated_bytes)),
            |source| {
                let mut stream = AnimationStreamBenchmark::new(source, streamed_animation_config())
                    .expect("the benchmark animation is valid");
                black_box(stream.consume(ANIMATION_FRAME_COUNT as usize));
            },
            BatchSize::SmallInput,
        );
    });
    animated.bench_function("streamed_8_streams_24_frames_320x180", |bencher| {
        bencher.iter_batched(
            || EncodedImage::new(ImageFormat::WebP, Arc::clone(&animated_bytes)),
            |source| {
                let config = streamed_animation_config();
                let mut streams = (0..8)
                    .map(|_| AnimationStreamBenchmark::new(source.clone(), config))
                    .collect::<gpui::Result<Vec<_>>>()
                    .expect("the benchmark animations are valid");
                for stream in &mut streams {
                    black_box(
                        stream.consume(
                            usize::try_from(ANIMATION_FRAME_COUNT - 1)
                                .expect("the benchmark frame count fits usize"),
                        ),
                    );
                }
            },
            BatchSize::SmallInput,
        );
    });
    animated.finish();

    let mut queue = criterion.benchmark_group("image_render/animation_queue");
    queue.sample_size(20);
    queue.bench_function("stale_12_frames", |bencher| {
        bencher.iter_custom(|iterations| {
            const BATCH_SIZE: u64 = 256;
            let mut remaining = iterations;
            let mut measured = Duration::ZERO;
            while remaining != 0 {
                let batch_size = remaining.min(BATCH_SIZE);
                let mut queues = (0..batch_size)
                    .map(|_| AnimationQueueBenchmark::new(12))
                    .collect::<Vec<_>>();
                let started_at = std::time::Instant::now();
                for queue in &mut queues {
                    black_box(queue.drain_stale());
                }
                measured = measured.saturating_add(started_at.elapsed());
                remaining -= batch_size;
            }
            measured
        });
    });
    queue.finish();

    let truncated = Arc::<[u8]>::from(&animated_bytes[..20]);
    let mut failures = criterion.benchmark_group("image_render/failure");
    failures.sample_size(20);
    failures.throughput(Throughput::Bytes(truncated.len() as u64));
    failures.bench_function("truncated_animated_webp", |bencher| {
        bencher.iter_batched(
            || EncodedImage::new(ImageFormat::WebP, Arc::clone(&truncated)),
            |source| {
                black_box(
                    source
                        .render(AnimatedImageConfig::default())
                        .expect_err("the truncated animation must be rejected"),
                );
            },
            BatchSize::SmallInput,
        );
    });
    failures.finish();
}

fn batch_png(criterion: &mut Criterion, bytes: &Arc<[u8]>) {
    const IMAGE_COUNT: usize = 8;
    let executor = gpui::background_executor();
    let mut group = criterion.benchmark_group("image_render/png_batch_8");
    group.sample_size(10);
    group.throughput(Throughput::Elements(
        u64::from(SOURCE_WIDTH) * u64::from(SOURCE_HEIGHT) * IMAGE_COUNT as u64,
    ));
    for concurrency in [0, 1, 2, 4] {
        let label = if concurrency == 0 {
            "caller_serial".to_owned()
        } else {
            format!("background_{concurrency}")
        };
        group.bench_function(label, |bencher| {
            bencher.iter(|| {
                if concurrency == 0 {
                    for _ in 0..IMAGE_COUNT {
                        black_box(
                            EncodedImage::new(ImageFormat::Png, Arc::clone(bytes))
                                .render(AnimatedImageConfig::default())
                                .expect("the benchmark fixture is valid"),
                        );
                    }
                } else {
                    // Bound decode concurrency on the existing platform executor. Each worker
                    // releases its decoded image before beginning the next; no new pool is created.
                    let tasks = (0..concurrency).map(|_| {
                        let bytes = Arc::clone(bytes);
                        executor.spawn(async move {
                            for _ in 0..IMAGE_COUNT / concurrency {
                                black_box(
                                    EncodedImage::new(ImageFormat::Png, Arc::clone(&bytes))
                                        .render(AnimatedImageConfig::default())
                                        .expect("the benchmark fixture is valid"),
                                );
                            }
                        })
                    });
                    executor.block(futures::future::join_all(tasks));
                }
            });
        });
    }
    group.finish();
}

fn ownership(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("image_render/ownership");
    group.sample_size(20);
    for (width, height) in [(1, 1), (64, 64), (SOURCE_WIDTH, SOURCE_HEIGHT)] {
        let pixels = vec![37; width as usize * height as usize * 4];
        group.throughput(Throughput::Bytes(pixels.len() as u64));
        for shared_copy in [true, false] {
            let label = if shared_copy {
                "arc_slice_copy"
            } else {
                "owned_pixels"
            };
            group.bench_function(
                BenchmarkId::new(label, format!("{width}x{height}")),
                |bencher| {
                    bencher.iter_batched(
                        || pixels.clone(),
                        |pixels| {
                            let image = if shared_copy {
                                gpui::RenderImage::from_raw_pixel_bytes(
                                    width,
                                    height,
                                    gpui::ImagePixelFormat::Rgba8,
                                    Arc::<[u8]>::from(pixels),
                                )
                            } else {
                                gpui::RenderImage::from_raw_pixels(
                                    width,
                                    height,
                                    gpui::ImagePixelFormat::Rgba8,
                                    pixels,
                                )
                            }
                            .expect("valid benchmark pixels");
                            black_box(image);
                        },
                        BatchSize::PerIteration,
                    );
                },
            );
        }
    }
    group.finish();
}

criterion_group!(gpui_images, images);
criterion_main!(gpui_images);
