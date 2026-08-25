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
/// animation prefetch, or GPU atlas residency. Image lifetime is controlled by ownership and
/// explicit trimming instead. The bitmap pool budget only bounds *idle reusable* buffers, so it
/// can reduce allocator churn and fragmentation without rejecting an active image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImagePipelineConfig {
    /// Controls animated image playback and GPU frame residency.
    pub animated: AnimatedImageConfig,
    /// Legacy decoded-image byte budget field. GPUI normalizes this to `usize::MAX` at runtime.
    pub max_resident_image_bytes: usize,
    /// Legacy compressed source byte budget field. GPUI normalizes this to `usize::MAX` at runtime.
    pub max_compressed_bytes: usize,
    /// Legacy animation-prefetch byte budget. GPUI uses frame-count backpressure instead.
    pub animation_prefetch_byte_limit: usize,
    /// Maximum bytes retained only by the reusable *free* decoded bitmap pool.
    ///
    /// This does not limit active image allocations: buffers larger than the retained pool are
    /// still allocated normally and are simply released when no longer reusable.
    pub bitmap_pool_bytes: usize,
    /// Maximum capacity eligible for bitmap-pool reuse. GPUI normalizes this to `usize::MAX` so
    /// full-size image buffers can participate in reuse; `bitmap_pool_bytes` still bounds idle
    /// retained memory.
    pub bitmap_pool_max_buffer_bytes: usize,
    /// Legacy aggregate GPU atlas byte budget. GPUI normalizes this to `usize::MAX` at runtime.
    pub max_atlas_bytes: usize,
    /// Legacy GPU atlas texture-count budget. GPUI normalizes this to `usize::MAX` at runtime.
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

impl ImagePipelineConfig {
    /// Converts byte-budget based image policy into GPUI's lifecycle-managed runtime policy.
    ///
    /// Count-based controls such as animation prefetch frames and GPU frame slots are preserved:
    /// they bound work in flight and latency rather than rejecting an image because of its byte
    /// size. The bitmap free-list retention budget is also preserved because it only controls idle
    /// allocator reuse memory.
    pub(crate) fn lifecycle_managed(mut self) -> Self {
        self.animated.prefetch_byte_limit = usize::MAX;
        self.animated.max_resident_bytes = usize::MAX;
        self.max_resident_image_bytes = usize::MAX;
        self.max_compressed_bytes = usize::MAX;
        self.animation_prefetch_byte_limit = usize::MAX;
        self.bitmap_pool_max_buffer_bytes = usize::MAX;
        self.max_atlas_bytes = usize::MAX;
        self.max_atlas_textures = usize::MAX;
        self
    }
}

impl Default for ImagePipelineConfig {
    fn default() -> Self {
        Self {
            animated: AnimatedImageConfig::default(),
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
        .lifecycle_managed()
    }
}

#[cfg(test)]
mod tests {
    use super::ImagePipelineConfig;

    #[test]
    fn runtime_policy_does_not_reject_images_by_byte_budget() {
        let mut config = ImagePipelineConfig::default();
        config.animated.prefetch_byte_limit = 1;
        config.animated.max_resident_bytes = 1;
        config.max_resident_image_bytes = 1;
        config.max_compressed_bytes = 1;
        config.animation_prefetch_byte_limit = 1;
        config.bitmap_pool_max_buffer_bytes = 1;
        config.max_atlas_bytes = 1;
        config.max_atlas_textures = 1;
        config.bitmap_pool_bytes = 4096;

        let runtime = config.lifecycle_managed();

        assert_eq!(runtime.animated.prefetch_byte_limit, usize::MAX);
        assert_eq!(runtime.animated.max_resident_bytes, usize::MAX);
        assert_eq!(runtime.max_resident_image_bytes, usize::MAX);
        assert_eq!(runtime.max_compressed_bytes, usize::MAX);
        assert_eq!(runtime.animation_prefetch_byte_limit, usize::MAX);
        assert_eq!(runtime.bitmap_pool_max_buffer_bytes, usize::MAX);
        assert_eq!(runtime.max_atlas_bytes, usize::MAX);
        assert_eq!(runtime.max_atlas_textures, usize::MAX);
        assert_eq!(runtime.bitmap_pool_bytes, 4096);
    }
}
