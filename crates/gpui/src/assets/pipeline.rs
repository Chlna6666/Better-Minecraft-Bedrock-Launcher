use super::AnimatedImageConfig;

/// Controls when bounds-aware image decoding is allowed to start.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ImageBoundsPolicy {
    /// Decode when the element explicitly requests bounds-aware decoding.
    #[default]
    Explicit,
    /// Decode bounds-aware images only when their visual bounds intersect the viewport.
    Visible,
}

/// Requested severity for an application image-memory trim.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ImageMemoryTrimLevel {
    /// Release temporary staging allocations.
    Light,
    /// Release unused decoded bitmaps while keeping compressed bytes.
    #[default]
    Moderate,
    /// Release all unused resident image state while keeping reusable source data.
    Aggressive,
}

/// Application-wide image pipeline policy and diagnostics thresholds.
///
/// GPUI does not impose finite byte ceilings on active decoded images, compressed image sources,
/// animation prefetch, or GPU atlas residency by default. Image lifetime is controlled by resource
/// ownership and explicit trimming instead. The bitmap pool budget only bounds *idle reusable*
/// buffers, so it can reduce allocator churn and fragmentation without rejecting an active image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImagePipelineConfig {
    /// Controls animated image playback and GPU frame residency.
    pub animated: AnimatedImageConfig,
    /// Optional decoded-image cache budget. `usize::MAX` means lifecycle-managed/unbounded.
    pub max_resident_image_bytes: usize,
    /// Optional compressed source cache budget. `usize::MAX` means lifecycle-managed/unbounded.
    pub max_compressed_bytes: usize,
    /// Optional aggregate animation-prefetch byte budget. `usize::MAX` disables byte backpressure.
    pub animation_prefetch_byte_limit: usize,
    /// Maximum bytes retained only by the reusable *free* decoded bitmap pool.
    ///
    /// This does not limit active image allocations: buffers larger than the retained pool are
    /// still allocated normally and are simply released when no longer reusable.
    pub bitmap_pool_bytes: usize,
    /// Maximum capacity eligible for bitmap-pool reuse. `usize::MAX` allows full-size image
    /// buffers to participate in reuse; `bitmap_pool_bytes` still bounds idle retained memory.
    pub bitmap_pool_max_buffer_bytes: usize,
    /// Optional aggregate GPU atlas byte budget. `usize::MAX` means lifecycle-managed/unbounded.
    pub max_atlas_bytes: usize,
    /// Optional GPU atlas texture-count budget. `usize::MAX` means lifecycle-managed/unbounded.
    pub max_atlas_textures: usize,
    /// Controls whether bounds-aware decode is gated by visibility.
    pub bounds_policy: ImageBoundsPolicy,
    /// Trim image staging and bitmap pool when a window loses activation.
    pub trim_memory_on_hidden: bool,
    /// Log a slow image processing when this duration is exceeded.
    pub slow_image_threshold: std::time::Duration,
    /// Log a slow atlas upload when this byte threshold is exceeded.
    pub slow_upload_bytes: usize,
    /// Log a slow atlas upload when this duration is exceeded.
    pub slow_upload_threshold: std::time::Duration,
}

impl Default for ImagePipelineConfig {
    fn default() -> Self {
        let mut animated = AnimatedImageConfig::default();
        // Frame counts remain bounded for scheduling/streaming, but byte size is not used as a
        // validity limit. A large frame must not become unrenderable just because it crosses an
        // arbitrary MiB threshold.
        animated.prefetch_byte_limit = usize::MAX;
        animated.max_resident_bytes = usize::MAX;

        Self {
            animated,
            max_resident_image_bytes: usize::MAX,
            max_compressed_bytes: usize::MAX,
            animation_prefetch_byte_limit: usize::MAX,
            // This is intentionally a free-buffer retention budget, not an image memory limit.
            bitmap_pool_bytes: 64 * 1024 * 1024,
            bitmap_pool_max_buffer_bytes: usize::MAX,
            max_atlas_bytes: usize::MAX,
            max_atlas_textures: usize::MAX,
            bounds_policy: ImageBoundsPolicy::Explicit,
            trim_memory_on_hidden: false,
            slow_image_threshold: std::time::Duration::from_millis(16),
            slow_upload_bytes: 8 * 1024 * 1024,
            slow_upload_threshold: std::time::Duration::from_millis(4),
        }
    }
}
