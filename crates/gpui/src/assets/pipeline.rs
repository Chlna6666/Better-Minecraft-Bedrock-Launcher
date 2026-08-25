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
    /// Release all unused resident image state while keeping the bounded byte cache.
    Aggressive,
}

/// Application-wide image pipeline resource limits and diagnostics thresholds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImagePipelineConfig {
    /// Controls animated image playback and GPU frame residency.
    pub animated: AnimatedImageConfig,
    /// Approximate maximum decoded image bytes retained by bounded caches by default.
    pub max_resident_image_bytes: usize,
    /// Maximum compressed image bytes retained by the shared in-memory cache.
    pub max_compressed_bytes: usize,
    /// Maximum decoded animation bytes queued across all image streams.
    pub animation_prefetch_byte_limit: usize,
    /// Maximum bytes retained by the reusable decoded bitmap pool.
    pub bitmap_pool_bytes: usize,
    /// Maximum capacity of one reusable bitmap buffer.
    pub bitmap_pool_max_buffer_bytes: usize,
    /// Maximum aggregate GPU atlas bytes allocated for image tiles.
    pub max_atlas_bytes: usize,
    /// Maximum number of GPU atlas textures allocated for image tiles.
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
        Self {
            animated: AnimatedImageConfig::default(),
            max_resident_image_bytes: 128 * 1024 * 1024,
            max_compressed_bytes: 64 * 1024 * 1024,
            animation_prefetch_byte_limit: 96 * 1024 * 1024,
            bitmap_pool_bytes: 64 * 1024 * 1024,
            bitmap_pool_max_buffer_bytes: 16 * 1024 * 1024,
            max_atlas_bytes: 256 * 1024 * 1024,
            max_atlas_textures: 32,
            bounds_policy: ImageBoundsPolicy::Explicit,
            trim_memory_on_hidden: false,
            slow_image_threshold: std::time::Duration::from_millis(16),
            slow_upload_bytes: 8 * 1024 * 1024,
            slow_upload_threshold: std::time::Duration::from_millis(4),
        }
    }
}
