use crate::assets::AnimatedFrame;
use crate::platform::{
    AtlasPixelEncodingBenchmarkCore, AtlasUploadBenchmarkCore, FrameUploadBenchmarkCore,
    MeshPackingBenchmarkCore, PathPackingBenchmarkCore,
};
use crate::{
    AnimatedImageConfig, AvailableSpace, Bounds, ContentMask, EncodedImage, LayoutId,
    PaintBackdropBlur, Path, Pixels, Quad, RenderImage, ScaledPixels, Scene, Style,
    TaffyLayoutEngine, VisualTestContext, acquire_bitmap_buffer_capacity,
    configure_global_bitmap_pool, global_bitmap_pool, point, px, release_bitmap_buffer, size,
    trim_global_bitmap_pool_to,
};
use image::{Frame, ImageFormat, Rgba, RgbaImage};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Controls the global bitmap pool for isolated Criterion measurements.
pub struct BitmapPoolBenchmark;

impl BitmapPoolBenchmark {
    /// Applies benchmark-local pool limits and clears retained buffers.
    pub fn new(byte_limit: usize, _max_buffer_bytes: usize) -> Self {
        configure_global_bitmap_pool(byte_limit);
        trim_global_bitmap_pool_to(0);
        Self
    }

    /// Acquires and releases one buffer for every requested capacity.
    pub fn cycle(&mut self, capacities: &[usize]) -> (usize, usize) {
        let buffers = capacities
            .iter()
            .map(|capacity| acquire_bitmap_buffer_capacity(*capacity))
            .collect::<Vec<_>>();
        for buffer in buffers {
            release_bitmap_buffer(buffer);
        }
        let snapshot = global_bitmap_pool().snapshot();
        (snapshot.retained_bytes, snapshot.free_buffers)
    }
}

/// Owns retained layout state for Criterion measurements.
pub struct LayoutBenchmark {
    engine: TaffyLayoutEngine,
}

impl LayoutBenchmark {
    /// Creates an empty layout benchmark state.
    pub fn new() -> Self {
        Self {
            engine: TaffyLayoutEngine::new(),
        }
    }

    /// Starts another frame while preserving retained-layout history.
    pub fn next_frame(&mut self) {
        self.engine.clear();
    }

    /// Builds and computes a flat tree with the requested number of leaf nodes.
    pub fn flat_tree(
        &mut self,
        node_count: usize,
        context: &mut VisualTestContext,
    ) -> Bounds<Pixels> {
        let root = self.request_flat_tree(node_count);
        context.update(|window, cx| {
            self.engine.compute_layout(
                root,
                size(
                    AvailableSpace::Definite(px(1_920.0)),
                    AvailableSpace::Definite(px(1_080.0)),
                ),
                window,
                cx,
            );
            self.engine.layout_bounds(root, window.scale_factor())
        })
    }

    fn request_flat_tree(&mut self, node_count: usize) -> LayoutId {
        let children = (0..node_count)
            .map(|_| {
                self.engine
                    .request_layout(Style::default(), px(16.0), 1.0, &[])
            })
            .collect::<Vec<_>>();
        self.engine
            .request_layout(Style::default(), px(16.0), 1.0, &children)
    }
}

impl Default for LayoutBenchmark {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of one retained Nova frame-upload encoding iteration.
#[derive(Clone, Copy, Debug)]
pub struct FrameUploadBenchmarkSample {
    pub encoded_primitives: usize,
    pub encoded_batches: usize,
    pub uploaded_bytes: usize,
    pub retained_byte_capacity: usize,
    pub backdrop_blur_configs: usize,
}

/// Retains Nova upload buffers while repeatedly encoding a deterministic scene.
pub struct FrameUploadBenchmark {
    core: FrameUploadBenchmarkCore,
}

/// A prepared set of pending atlas writes for measuring CPU-side upload batching.
pub struct AtlasUploadBenchmark {
    core: AtlasUploadBenchmarkCore,
}

/// A reusable CPU-side atlas pixel conversion workload.
pub struct AtlasPixelEncodingBenchmark {
    core: AtlasPixelEncodingBenchmarkCore,
}

/// Owns a path used to measure bulk device-space vertex transformation.
pub struct PathTransformBenchmark {
    path: Path<Pixels>,
    requested_vertex_count: usize,
}

/// Owns a Nova scene used to measure path rasterization packing on cache misses.
pub struct PathPackingBenchmark {
    core: PathPackingBenchmarkCore,
}

/// Owns a CPU-only mesh conversion workload for packed vertex and index buffers.
pub struct MeshPackingBenchmark {
    core: MeshPackingBenchmarkCore,
}

fn benchmark_path(requested_vertex_count: usize) -> Path<Pixels> {
    let mut path = Path::new(point(px(0.0), px(0.0)));
    let triangle_count = requested_vertex_count.saturating_add(2) / 3;
    for triangle_index in 0..triangle_count {
        let base = triangle_index as f32 * 3.0;
        path.push_triangle(
            (
                point(px(base), px(base * 0.5)),
                point(px(base + 1.0), px(base * 0.5 + 1.0)),
                point(px(base + 2.0), px(base * 0.5)),
            ),
            (point(0.0, 0.0), point(0.5, 1.0), point(1.0, 0.0)),
        );
    }
    path.content_mask = ContentMask::new(path.bounds);
    path
}

impl PathTransformBenchmark {
    /// Creates a path with enough complete triangles to cover the requested vertex count.
    pub fn new(requested_vertex_count: usize) -> Self {
        Self {
            path: benchmark_path(requested_vertex_count),
            requested_vertex_count,
        }
    }

    /// Transforms the path and returns the requested workload size.
    pub fn transform(&self) -> usize {
        let transformed = self.path.scale_and_transform_for_paint(
            1.25,
            0.875,
            point(ScaledPixels(4.0), ScaledPixels(-3.0)),
        );
        std::hint::black_box(transformed);
        self.requested_vertex_count
    }

    /// Transforms the same path with the scalar baseline.
    pub fn transform_scalar(&self) -> usize {
        let transformed = self.path.scale_and_transform_for_paint_scalar(
            1.25,
            0.875,
            point(ScaledPixels(4.0), ScaledPixels(-3.0)),
        );
        std::hint::black_box(transformed);
        self.requested_vertex_count
    }

    /// Transforms the same path with the runtime-selected Fearless SIMD path.
    pub fn transform_simd(&self) -> usize {
        let transformed = self.path.scale_and_transform_for_paint_simd(
            1.25,
            0.875,
            point(ScaledPixels(4.0), ScaledPixels(-3.0)),
        );
        std::hint::black_box(transformed);
        self.requested_vertex_count
    }
}

impl PathPackingBenchmark {
    /// Creates a path scene whose packed vertices are regenerated on every benchmark iteration.
    pub fn new(requested_vertex_count: usize) -> Self {
        let mut scene = Scene::default();
        scene.insert_primitive(benchmark_path(requested_vertex_count).scale(1.0));
        scene.finish();
        Self {
            core: PathPackingBenchmarkCore::new(scene),
        }
    }

    /// Clears the path cache before encoding, forcing the cache-miss packing path.
    pub fn encode_cache_miss(&mut self) -> usize {
        self.core.encode_cache_miss()
    }
}

impl MeshPackingBenchmark {
    /// Creates a mesh conversion workload with either 16-bit or 32-bit output indices.
    pub fn new(vertex_count: usize, uses_u16: bool) -> Self {
        Self {
            core: MeshPackingBenchmarkCore::new(vertex_count, uses_u16),
        }
    }

    /// Rebuilds the packed vertex and index buffers and returns their byte lengths.
    pub fn pack(&mut self) -> (usize, usize) {
        self.core.pack()
    }

    /// Rebuilds the same mesh using the scalar index-packing baseline.
    pub fn pack_scalar(&mut self) -> (usize, usize) {
        self.core.pack_scalar()
    }

    /// Rebuilds the same mesh using the runtime-selected Fearless SIMD index packer.
    pub fn pack_simd(&mut self) -> (usize, usize) {
        self.core.pack_simd()
    }
}

impl AtlasPixelEncodingBenchmark {
    /// Creates an RGBA-to-BGRA atlas workload with edge padding.
    pub fn rgba(width: u32, height: u32, padding: u32) -> Self {
        Self {
            core: AtlasPixelEncodingBenchmarkCore::rgba(width, height, padding),
        }
    }

    /// Creates an already-BGRA atlas workload with edge padding.
    pub fn bgra(width: u32, height: u32, padding: u32) -> Self {
        Self {
            core: AtlasPixelEncodingBenchmarkCore::bgra(width, height, padding),
        }
    }

    /// Creates a coverage-mask-to-BGRA glyph workload with edge padding.
    pub fn monochrome(width: u32, height: u32, padding: u32) -> Self {
        Self {
            core: AtlasPixelEncodingBenchmarkCore::monochrome(width, height, padding),
        }
    }

    /// Creates a premultiplied subpixel glyph workload with edge padding.
    pub fn subpixel(width: u32, height: u32, padding: u32) -> Self {
        Self {
            core: AtlasPixelEncodingBenchmarkCore::subpixel(width, height, padding),
        }
    }

    /// Rewrites the retained destination buffer and returns its byte length.
    pub fn encode(&mut self) -> usize {
        self.core.encode()
    }

    /// Rewrites an RGBA destination with the scalar channel-swap baseline.
    pub fn encode_scalar(&mut self) -> usize {
        self.core.encode_scalar()
    }

    /// Rewrites an RGBA destination with the runtime-selected Fearless SIMD path.
    pub fn encode_simd(&mut self) -> usize {
        self.core.encode_simd()
    }
}

impl AtlasUploadBenchmark {
    /// Creates an atlas containing square dirty RGBA image tiles.
    pub fn rgba_tiles(upload_count: usize, tile_size: u32) -> Self {
        Self {
            core: AtlasUploadBenchmarkCore::rgba_tiles(upload_count, tile_size),
        }
    }

    /// Resolves and drains the prepared upload descriptors without issuing GPU work.
    pub fn upload(&self) -> (usize, usize) {
        self.core.upload()
    }
}

impl FrameUploadBenchmark {
    /// Creates a scene containing a flat run of quads.
    pub fn quads(primitive_count: usize) -> Self {
        let mut scene = Scene::default();
        for index in 0..primitive_count {
            let x = (index % 128) as f32 * 12.0;
            let y = (index / 128) as f32 * 12.0;
            let bounds = Bounds::new(
                point(ScaledPixels(x), ScaledPixels(y)),
                size(ScaledPixels(10.0), ScaledPixels(10.0)),
            );
            scene.insert_primitive(Quad {
                order: index as u32,
                bounds,
                content_mask: ContentMask {
                    bounds,
                    ..ContentMask::default()
                },
                ..Quad::default()
            });
        }
        scene.finish();
        Self::new(scene)
    }

    /// Creates overlapping blur groups to measure config merging and upload reuse.
    pub fn backdrop_blurs(primitive_count: usize) -> Self {
        let mut scene = Scene::default();
        for index in 0..primitive_count {
            let x = (index % 16) as f32 * 48.0;
            let y = (index / 16) as f32 * 32.0;
            let bounds = Bounds::new(
                point(ScaledPixels(x), ScaledPixels(y)),
                size(ScaledPixels(96.0), ScaledPixels(64.0)),
            );
            scene.insert_primitive(PaintBackdropBlur {
                order: index as u32,
                animation_id: None,
                bounds,
                content_mask: ContentMask {
                    bounds,
                    ..ContentMask::default()
                },
                corner_radii: Default::default(),
                radius: ScaledPixels(18.0),
                downsample: 1,
                levels: 1,
                recompute_overlap: false,
                saturation: 1.0,
                opacity: 1.0,
                tint: None,
            });
        }
        scene.finish();
        Self::new(scene)
    }

    fn new(scene: Scene) -> Self {
        Self {
            core: FrameUploadBenchmarkCore::new(scene),
        }
    }

    /// Encodes another frame while preserving upload-buffer capacities and caches.
    pub fn next_frame(&mut self) -> FrameUploadBenchmarkSample {
        let (
            encoded_primitives,
            encoded_batches,
            uploaded_bytes,
            retained_byte_capacity,
            backdrop_blur_configs,
        ) = self.core.next_frame();
        FrameUploadBenchmarkSample {
            encoded_primitives,
            encoded_batches,
            uploaded_bytes,
            retained_byte_capacity,
            backdrop_blur_configs,
        }
    }
}

/// Drives one bounded streaming animation without involving window presentation.
pub struct AnimationStreamBenchmark {
    image: RenderImage,
    sequence: usize,
}

impl AnimationStreamBenchmark {
    /// Starts the streaming path for an encoded animation.
    pub fn new(source: EncodedImage, config: AnimatedImageConfig) -> crate::Result<Self> {
        let image = source.render(config)?;
        assert_eq!(
            image.frame_count(),
            usize::MAX,
            "the benchmark configuration must select streaming storage"
        );
        Ok(Self { image, sequence: 0 })
    }

    /// Waits for the requested number of decoded frames and returns the final sequence number.
    pub fn consume(&mut self, frame_count: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(2);
        for _ in 0..frame_count {
            loop {
                if let Some(frame) = self.image.next_streaming_frame(self.sequence) {
                    self.sequence = frame.sequence();
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "streaming animation benchmark exceeded its two-second deadline"
                );
                std::thread::yield_now();
            }
        }
        self.sequence
    }
}

/// Owns a prefilled streaming queue for isolated stale-frame drain measurements.
pub struct AnimationQueueBenchmark {
    image: RenderImage,
}

impl AnimationQueueBenchmark {
    /// Creates a streaming image with the requested number of queued one-pixel frames.
    pub fn new(queued_frame_count: usize) -> Self {
        let source = EncodedImage::new(ImageFormat::Png, Arc::<[u8]>::from([]));
        let frames = (0..=queued_frame_count)
            .map(|sequence| {
                AnimatedFrame::from_bgra_frame(
                    sequence,
                    Frame::new(RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 255]))),
                )
            })
            .collect::<Vec<_>>();
        let first_frame = frames
            .first()
            .expect("the benchmark always creates an initial frame")
            .clone();
        let queued_frames = frames.into_iter().skip(1).collect::<Vec<_>>();
        let image = RenderImage::streaming(
            source,
            first_frame,
            queued_frames.into(),
            AnimatedImageConfig {
                prefetch_frames: queued_frame_count.max(2),
                max_resident_frames: 1,
                ..AnimatedImageConfig::default()
            },
        );
        Self { image }
    }

    /// Discards every queued frame and returns the released resident bytes.
    pub fn drain_stale(&mut self) -> usize {
        let before = self.image.resident_byte_len();
        let _ = self.image.next_streaming_frame(usize::MAX);
        before.saturating_sub(self.image.resident_byte_len())
    }
}
