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
/// Active image memory is lifetime-managed. The public API exposes only scheduling, visibility,
/// diagnostics, and idle-buffer reuse policy; byte ceilings are not configurable because they must
/// never make a valid image unrenderable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImagePipelineConfig {
    /// Controls animated image playback and GPU frame residency.
    pub animated: AnimatedImageConfig,
    /// Internal compatibility state for code paths that have not yet been converted away from byte
    /// accounting. Always normalized to an unbounded sentinel and deliberately not public.
    pub(crate) max_resident_image_bytes: usize,
    pub(crate) max_compressed_bytes: usize,
    pub(crate) animation_prefetch_byte_limit: usize,
    /// Maximum bytes retained only by the reusable *free* decoded bitmap pool.
    ///
    /// This does not limit active image allocations. It only bounds idle allocator reuse memory.
    pub bitmap_pool_bytes: usize,
    pub(crate) bitmap_pool_max_buffer_bytes: usize,
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
    /// Normalizes internal compatibility state to lifecycle-managed/unbounded values.
    pub(crate) fn lifecycle_managed(mut self) -> Self {
        self.animated.prefetch_byte_limit = usize::MAX;
        self.animated.max_resident_bytes = usize::MAX;
        self.max_resident_image_bytes = usize::MAX;
        self.max_compressed_bytes = usize::MAX;
        self.animation_prefetch_byte_limit = usize::MAX;
        self.bitmap_pool_max_buffer_bytes = usize::MAX;
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
    fn internal_budget_state_is_always_unbounded() {
        let runtime = ImagePipelineConfig::default().lifecycle_managed();

        assert_eq!(runtime.animated.prefetch_byte_limit, usize::MAX);
        assert_eq!(runtime.animated.max_resident_bytes, usize::MAX);
        assert_eq!(runtime.max_resident_image_bytes, usize::MAX);
        assert_eq!(runtime.max_compressed_bytes, usize::MAX);
        assert_eq!(runtime.animation_prefetch_byte_limit, usize::MAX);
        assert_eq!(runtime.bitmap_pool_max_buffer_bytes, usize::MAX);
    }
}
