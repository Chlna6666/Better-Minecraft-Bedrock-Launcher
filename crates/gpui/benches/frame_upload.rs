use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use gpui::benchmark::{
    AtlasPixelEncodingBenchmark, AtlasUploadBenchmark, FrameUploadBenchmark, MeshPackingBenchmark,
};

const QUAD_COUNTS: [usize; 3] = [128, 1_024, 8_192];
const BLUR_COUNTS: [usize; 3] = [1, 16, 128];
const ATLAS_UPLOAD_COUNTS: [usize; 3] = [1, 8, 64];
const ATLAS_TILE_SIZES: [u32; 3] = [1, 16, 64];
const GLYPH_SIZES: [u32; 3] = [16, 32, 64];
const GLYPH_PADDING: [u32; 3] = [0, 1, 2];
const RGBA_PIXEL_WORKLOADS: [(usize, u32, u32); 6] = [
    (16, 4, 4),
    (32, 8, 4),
    (64, 8, 8),
    (256, 16, 16),
    (1_024, 32, 32),
    (65_536, 256, 256),
];
const MESH_VERTEX_COUNTS: [usize; 3] = [256, 1_024, 65_536];

fn frame_upload(criterion: &mut Criterion) {
    let mut quads = criterion.benchmark_group("frame_upload/scene_pack_time/quads");
    quads.sample_size(30);
    for primitive_count in QUAD_COUNTS {
        let mut frame = FrameUploadBenchmark::quads(primitive_count);
        black_box(frame.next_frame());
        quads.throughput(Throughput::Elements(primitive_count as u64));
        quads.bench_with_input(
            BenchmarkId::from_parameter(primitive_count),
            &primitive_count,
            |bencher, _| bencher.iter(|| black_box(frame.next_frame())),
        );
    }
    quads.finish();

    let mut blurs = criterion.benchmark_group("frame_upload/retained_backdrop_blurs");
    blurs.sample_size(30);
    for primitive_count in BLUR_COUNTS {
        let mut frame = FrameUploadBenchmark::backdrop_blurs(primitive_count);
        let warm = frame.next_frame();
        assert!(warm.retained_byte_capacity >= warm.uploaded_bytes);
        blurs.throughput(Throughput::Elements(primitive_count as u64));
        blurs.bench_with_input(
            BenchmarkId::from_parameter(primitive_count),
            &primitive_count,
            |bencher, _| bencher.iter(|| black_box(frame.next_frame())),
        );
    }
    blurs.finish();

    let mut atlas = criterion.benchmark_group("frame_upload/atlas_writes");
    atlas.sample_size(30);
    for tile_size in ATLAS_TILE_SIZES {
        for upload_count in ATLAS_UPLOAD_COUNTS {
            atlas.throughput(Throughput::Bytes(
                upload_count as u64 * u64::from(tile_size) * u64::from(tile_size) * 4,
            ));
            atlas.bench_with_input(
                BenchmarkId::new(format!("{tile_size}x{tile_size}"), upload_count),
                &(upload_count, tile_size),
                |bencher, &(upload_count, tile_size)| {
                    bencher.iter_batched(
                        || AtlasUploadBenchmark::rgba_tiles(upload_count, tile_size),
                        |upload| black_box(upload.upload()),
                        BatchSize::SmallInput,
                    );
                },
            );
        }
    }
    atlas.finish();

    let mut pixels = criterion.benchmark_group("frame_upload/atlas_pixels");
    pixels.sample_size(30);

    for &(pixel_count, width, height) in &RGBA_PIXEL_WORKLOADS {
        for padding in [0, 1, 2] {
            pixels.throughput(Throughput::Bytes((pixel_count * 4) as u64));
            let mut rgba_scalar = AtlasPixelEncodingBenchmark::rgba(width, height, padding);
            pixels.bench_function(
                format!("scalar/rgba_{pixel_count}_pixels_padding_{padding}"),
                |bencher| bencher.iter(|| black_box(rgba_scalar.encode_scalar())),
            );
            let mut rgba_simd = AtlasPixelEncodingBenchmark::rgba(width, height, padding);
            pixels.bench_function(
                format!("fearless_simd/rgba_{pixel_count}_pixels_padding_{padding}"),
                |bencher| bencher.iter(|| black_box(rgba_simd.encode_simd())),
            );
        }
    }

    pixels.throughput(Throughput::Bytes(320 * 180 * 4));
    let mut rgba_unpadded = AtlasPixelEncodingBenchmark::rgba(320, 180, 0);
    pixels.bench_function("rgba_320x180_padding_0", |bencher| {
        bencher.iter(|| black_box(rgba_unpadded.encode()));
    });
    let mut rgba = AtlasPixelEncodingBenchmark::rgba(320, 180, 1);
    pixels.bench_function("rgba_320x180_padding_1", |bencher| {
        bencher.iter(|| black_box(rgba.encode()));
    });
    let mut bgra_unpadded = AtlasPixelEncodingBenchmark::bgra(320, 180, 0);
    pixels.bench_function("bgra_320x180_padding_0", |bencher| {
        bencher.iter(|| black_box(bgra_unpadded.encode()));
    });
    let mut bgra = AtlasPixelEncodingBenchmark::bgra(320, 180, 1);
    pixels.bench_function("bgra_320x180_padding_1", |bencher| {
        bencher.iter(|| black_box(bgra.encode()));
    });
    pixels.finish();

    let mut monochrome = criterion.benchmark_group("frame_upload/atlas_monochrome");
    monochrome.sample_size(30);
    for size in GLYPH_SIZES {
        for padding in GLYPH_PADDING {
            monochrome.throughput(Throughput::Bytes(u64::from(size) * u64::from(size)));
            let mut glyph = AtlasPixelEncodingBenchmark::monochrome(size, size, padding);
            monochrome.bench_function(format!("{size}x{size}_padding_{padding}"), |bencher| {
                bencher.iter(|| black_box(glyph.encode()));
            });
        }
    }
    monochrome.finish();

    let mut subpixel = criterion.benchmark_group("frame_upload/atlas_subpixel");
    subpixel.sample_size(30);
    for size in GLYPH_SIZES {
        for padding in GLYPH_PADDING {
            subpixel.throughput(Throughput::Bytes(u64::from(size) * u64::from(size) * 4));
            let mut glyph = AtlasPixelEncodingBenchmark::subpixel(size, size, padding);
            subpixel.bench_function(format!("{size}x{size}_padding_{padding}"), |bencher| {
                bencher.iter(|| black_box(glyph.encode()));
            });
        }
    }
    subpixel.finish();

    let mut meshes = criterion.benchmark_group("mesh_packing");
    meshes.sample_size(30);
    for vertex_count in MESH_VERTEX_COUNTS {
        for uses_u16 in [true, false] {
            let index_format = if uses_u16 { "u16" } else { "u32" };
            meshes.throughput(Throughput::Elements(vertex_count as u64));
            meshes.bench_with_input(
                BenchmarkId::new(format!("scalar/{index_format}"), vertex_count),
                &vertex_count,
                |bencher, _| {
                    let mut mesh = MeshPackingBenchmark::new(vertex_count, uses_u16);
                    bencher.iter(|| black_box(mesh.pack_scalar()));
                },
            );
            meshes.bench_with_input(
                BenchmarkId::new(format!("fearless_simd/{index_format}"), vertex_count),
                &vertex_count,
                |bencher, _| {
                    let mut mesh = MeshPackingBenchmark::new(vertex_count, uses_u16);
                    bencher.iter(|| black_box(mesh.pack_simd()));
                },
            );
        }
    }
    meshes.finish();
}

criterion_group!(gpui_frame_upload, frame_upload);
criterion_main!(gpui_frame_upload);
