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
    /// Release unused decoded bitmaps while keeping reusable source data.
    #[default]
    Moderate,
    /// Release all unused resident image state.
    Aggressive,
}

/// Application-wide image pipeline policy and diagnostics thresholds.
///
/// Active image memory is lifetime-managed. The public API exposes scheduling, visibility,
/// diagnostics, and idle-buffer reuse policy; fixed byte ceilings are intentionally absent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImagePipelineConfig {
    /// Controls animated image playback and frame residency.
    pub animated: AnimatedImageConfig,
    /// Maximum bytes retained only by the reusable *free* decoded bitmap pool.
    ///
    /// This does not limit active image allocations. It only bounds idle allocator reuse memory.
    pub bitmap_pool_bytes: usize,
    /// Controls whether bounds-aware decode is gated by visibility.
    pub bounds_policy: ImageBoundsPolicy,
    /// Trim image staging and bitmap pool when a window loses activation.
    pub trim_memory_on_hidden: bool,
    /// Log slow image processing when this duration is exceeded.
    pub slow_image_threshold: std::time::Duration,
    /// Log a slow atlas upload when this byte threshold is exceeded.
    ///
    /// This is diagnostics-only and never rejects an upload.
    pub slow_upload_bytes: usize,
    /// Log a slow atlas upload when this duration is exceeded.
    pub slow_upload_threshold: std::time::Duration,
}

impl Default for ImagePipelineConfig {
    fn default() -> Self {
        Self {
            animated: AnimatedImageConfig::default(),
            // This is intentionally a free-buffer retention budget, not an image memory limit.
            bitmap_pool_bytes: 64 * 1024 * 1024,
            bounds_policy: ImageBoundsPolicy::Explicit,
            trim_memory_on_hidden: false,
            slow_image_threshold: std::time::Duration::from_millis(16),
            slow_upload_bytes: 8 * 1024 * 1024,
            slow_upload_threshold: std::time::Duration::from_millis(4),
        }
    }
}
