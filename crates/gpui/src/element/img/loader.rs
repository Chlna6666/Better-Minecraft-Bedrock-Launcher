use super::element::*;
use super::error::ImageCacheError;
use super::source::*;
use crate::{
    AnimatedFrame, App, Asset, AssetLocation, Bounds, EncodedImage, ImageRenderRecord,
    ImageRenderSize, ObjectFit, Pixels, RenderImage, SMOOTH_SVG_SCALE_FACTOR, SharedString, Size,
    SvgSize, Window, hash, record_image_asset_retained,
    record_image_processing_metrics_with_threshold, swap_rgba_pa_to_bgra,
};
use anyhow::{Context as _, Result};
use futures::{AsyncReadExt, Future};
use image::{Frame, ImageBuffer};
use parking_lot::Mutex;
use smallvec::SmallVec;
use std::{
    any::TypeId,
    borrow::Cow,
    collections::HashMap,
    fs,
    sync::{Arc, OnceLock},
    time::Instant,
};

struct CompressedCacheEntry {
    bytes: Arc<[u8]>,
    last_used: u64,
}

struct CompressedCache {
    entries: HashMap<u64, CompressedCacheEntry>,
    next_use: u64,
    retained_bytes: usize,
    max_bytes: usize,
}

impl CompressedCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            next_use: 0,
            retained_bytes: 0,
            max_bytes: 64 * 1024 * 1024,
        }
    }

    fn touch(&mut self) -> u64 {
        let use_order = self.next_use;
        self.next_use = self.next_use.wrapping_add(1);
        use_order
    }

    fn get(&mut self, key: u64) -> Option<Arc<[u8]>> {
        let use_order = self.touch();
        let entry = self.entries.get_mut(&key)?;
        entry.last_used = use_order;
        Some(entry.bytes.clone())
    }

    fn insert(&mut self, key: u64, bytes: Arc<[u8]>) {
        if bytes.len() > self.max_bytes {
            return;
        }
        let byte_len = bytes.len();
        let last_used = self.touch();
        if let Some(previous) = self
            .entries
            .insert(key, CompressedCacheEntry { bytes, last_used })
        {
            self.retained_bytes = self.retained_bytes.saturating_sub(previous.bytes.len());
        }
        self.retained_bytes = self.retained_bytes.saturating_add(byte_len);
        self.trim(self.max_bytes);
    }

    /// Removes the least recently used entry; eviction is a low-frequency path, so the linear
    /// scan here is the cheap trade for O(1) `get`/`insert`.
    fn evict_least_recently_used(&mut self) -> bool {
        let Some(oldest) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| *key)
        else {
            return false;
        };
        if let Some(previous) = self.entries.remove(&oldest) {
            self.retained_bytes = self.retained_bytes.saturating_sub(previous.bytes.len());
        }
        true
    }

    fn trim(&mut self, max_bytes: usize) {
        while self.retained_bytes > max_bytes {
            if !self.evict_least_recently_used() {
                break;
            }
        }
    }
}

static COMPRESSED_CACHE: OnceLock<Mutex<CompressedCache>> = OnceLock::new();

fn compressed_cache() -> &'static Mutex<CompressedCache> {
    COMPRESSED_CACHE.get_or_init(|| Mutex::new(CompressedCache::new()))
}

pub(crate) fn configure_compressed_cache(max_bytes: usize) {
    let mut cache = compressed_cache().lock();
    cache.max_bytes = max_bytes;
    cache.trim(max_bytes);
}

pub(crate) fn compressed_cache_snapshot() -> (usize, usize) {
    let cache = compressed_cache().lock();
    (cache.entries.len(), cache.retained_bytes)
}

pub(crate) fn trim_compressed_cache(max_bytes: usize) {
    compressed_cache().lock().trim(max_bytes);
}

/// AssetLocation image request for a concrete device-pixel output size.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ImageRenderRequest {
    resource: AssetLocation,
    size: ImageRenderSize,
    scale_factor_bits: u32,
    object_fit: ObjectFit,
    diagnostic_label: SharedString,
}

impl ImageRenderRequest {
    pub(crate) fn new(
        resource: AssetLocation,
        size: ImageRenderSize,
        scale_factor: f32,
        object_fit: ObjectFit,
    ) -> Self {
        let scale_factor = normalize_scale_factor(scale_factor);
        Self {
            diagnostic_label: resource_diagnostic_label(&resource),
            resource,
            size,
            scale_factor_bits: scale_factor.to_bits(),
            object_fit,
        }
    }

    /// Returns the asset location backing this image request.
    pub fn resource(&self) -> &AssetLocation {
        &self.resource
    }
}

/// AssetLocation image source used to cache compressed bytes before size-specific decoding.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CompressedImageSource {
    resource: AssetLocation,
}

impl CompressedImageSource {
    pub(crate) fn new(resource: AssetLocation) -> Self {
        Self { resource }
    }
}

fn resource_diagnostic_label(resource: &AssetLocation) -> SharedString {
    match resource {
        AssetLocation::Path(path) => path.to_string_lossy().into_owned().into(),
        AssetLocation::Uri(uri) => uri.to_string().into(),
        AssetLocation::Embedded(path) => path.clone(),
    }
}

/// Asset loader for compressed image bytes reused across multiple size-specific decodes.
#[derive(Clone)]
pub(crate) enum CompressedImageAssetLoader {}

impl Asset for CompressedImageAssetLoader {
    type Source = CompressedImageSource;
    type Output = Result<CompressedImageBytes, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        let asset_source = cx.asset_source().clone();
        async move {
            let cache_key = hash(&source);
            if let Some(bytes) = compressed_cache().lock().get(cache_key) {
                return Ok(CompressedImageBytes::Shared(bytes));
            }
            let source_bytes =
                load_image_resource_data(source.resource, client, asset_source).await?;
            let compressed = source_bytes.into_compressed_image_bytes();
            if let CompressedImageBytes::Shared(bytes) = &compressed {
                compressed_cache().lock().insert(cache_key, bytes.clone());
            }
            Ok(compressed)
        }
    }
}

pub(crate) fn image_size_for_bounds(
    logical_size: Size<Pixels>,
    scale_factor: f32,
) -> Option<ImageRenderSize> {
    let scale_factor = normalize_scale_factor(scale_factor);
    let size = logical_size.to_device_pixels(scale_factor);
    let width = u32::try_from(size.width.0.max(0)).ok()?;
    let height = u32::try_from(size.height.0.max(0)).ok()?;
    let overscan = sampling_overscan_factor(width, height);
    ImageRenderSize::new(
        bucket_image_dimension(((width as f32) * overscan).ceil() as u32),
        bucket_image_dimension(((height as f32) * overscan).ceil() as u32),
    )
}

fn normalize_scale_factor(scale_factor: f32) -> f32 {
    const SCALE_FACTOR_BUCKETS_PER_UNIT: f32 = 1024.0;

    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return 1.0;
    }

    (scale_factor * SCALE_FACTOR_BUCKETS_PER_UNIT).round() / SCALE_FACTOR_BUCKETS_PER_UNIT
}

pub(super) fn image_size_for_window(
    bounds: Bounds<Pixels>,
    window: &Window,
) -> Option<ImageRenderSize> {
    image_size_for_bounds(bounds.size, window.scale_factor())
}

fn sampling_overscan_factor(width: u32, height: u32) -> f32 {
    let max_dimension = width.max(height);
    if max_dimension <= 128 {
        1.0
    } else if max_dimension <= 512 {
        1.25
    } else if max_dimension <= 1024 {
        1.35
    } else {
        1.2
    }
}

fn bucket_image_dimension(value: u32) -> u32 {
    const BUCKET: u32 = 16;
    value.max(1).div_ceil(BUCKET) * BUCKET
}

/// An image loader for the GPUI asset system
#[derive(Clone)]
pub enum ImageAssetLoader {}

impl Asset for ImageAssetLoader {
    type Source = AssetLocation;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let compressed_task = cx
            .fetch_asset::<CompressedImageLoader>(&CompressedImageSource::new(source.clone()))
            .0;
        let svg_renderer = cx.svg_renderer();
        let pipeline_config = cx.image_pipeline_config();
        let image_config = pipeline_config.animated;
        let slow_image_threshold = pipeline_config.slow_image_threshold;
        async move {
            let source_bytes = compressed_task.await?;
            let bytes = source_bytes.as_bytes();
            let compressed_len = source_bytes.len();

            let processing_started = Instant::now();
            let mut data = if let Ok(format) = image::guess_format(&bytes) {
                EncodedImage::new(format, Arc::<[u8]>::from(bytes)).render(image_config)?
            } else {
                let pixmap =
                    // TODO: Can we make svgs always rescale?
                    svg_renderer
                        .render_pixmap(bytes, SvgSize::ScaleFactor(SMOOTH_SVG_SCALE_FACTOR))?;

                let mut buffer =
                    ImageBuffer::from_raw(pixmap.width(), pixmap.height(), pixmap.take()).unwrap();

                for pixel in buffer.chunks_exact_mut(4) {
                    swap_rgba_pa_to_bgra(pixel);
                }

                let mut image = RenderImage::new(SmallVec::from_elem(Frame::new(buffer), 1));
                image.scale_factor = SMOOTH_SVG_SCALE_FACTOR;
                image
            };

            let processing_duration = processing_started.elapsed();
            data = data.with_processing_metrics(compressed_len, processing_duration);
            // Reuse the same ImageId across re-decodes of this resource so retained atlas
            // tiles keyed by the id are reused instead of leaking a new tile per decode.
            data.id =
                crate::interned_render_image_id(TypeId::of::<ImageAssetLoader>(), hash(&source));
            record_image_processing_metrics_with_threshold(
                compressed_len,
                data.resident_byte_len(),
                data.frame_count(),
                processing_duration,
                slow_image_threshold,
            );
            if processing_duration >= slow_image_threshold {
                log::debug!(
                    "slow image processing: source={source:?} compressed_bytes={} output_bytes={} frames={} processing_ms={:.3}",
                    compressed_len,
                    data.resident_byte_len(),
                    data.frame_count(),
                    processing_duration.as_secs_f64() * 1000.0
                );
            }

            Ok(Arc::new(data))
        }
    }
}

/// Asset loader for resource images decoded to an element's current paint bounds.
#[derive(Clone)]
pub enum SizedImageAssetLoader {}

impl Asset for SizedImageAssetLoader {
    type Source = ImageRenderRequest;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let svg_renderer = cx.svg_renderer();
        let pipeline_config = cx.image_pipeline_config();
        let image_config = pipeline_config.animated;
        let slow_image_threshold = pipeline_config.slow_image_threshold;
        let image_input = SizedImageInput::PreloadedBytes(
            cx.fetch_asset::<CompressedImageLoader>(&CompressedImageSource {
                resource: source.resource.clone(),
            })
            .0,
        );
        async move {
            let processing_started = Instant::now();
            let scale_factor = f32::from_bits(source.scale_factor_bits);
            let (mut data, metadata, compressed_len) = render_sized_input(
                image_input,
                svg_renderer,
                image_config,
                source.size,
                source.object_fit,
            )
            .await?;

            let processing_duration = processing_started.elapsed();
            data = data
                .with_scale_factor(scale_factor)
                .with_processing_metrics(compressed_len, processing_duration);
            // The source hash covers the resource, decode target, scale factor, and object
            // fit, so a matching key always yields pixel-identical frames and can safely
            // reuse the previous ImageId (and therefore its resident atlas tiles).
            data.id = crate::interned_render_image_id(
                TypeId::of::<SizedImageAssetLoader>(),
                hash(&source),
            );
            record_image_processing_metrics_with_threshold(
                compressed_len,
                data.resident_byte_len(),
                data.frame_count(),
                processing_duration,
                slow_image_threshold,
            );

            let image = Arc::new(data);
            record_image_asset_retained(
                hash(&source),
                ImageRenderRecord {
                    source: source.diagnostic_label.to_string(),
                    original_width: metadata.original_width,
                    original_height: metadata.original_height,
                    target_width: metadata.size.width,
                    target_height: metadata.size.height,
                    resident_bytes: image.resident_byte_len(),
                    render_path: metadata.render_path.to_string(),
                },
            );

            Ok(image)
        }
    }
}

async fn render_sized_input(
    source: SizedImageInput,
    svg_renderer: crate::SvgRenderer,
    image_config: crate::AnimatedImageConfig,
    target: ImageRenderSize,
    object_fit: ObjectFit,
) -> Result<(RenderImage, crate::ImageRenderInfo, usize), ImageCacheError> {
    match source {
        SizedImageInput::PreloadedBytes(compressed_task) => {
            let compressed_bytes = compressed_task.await?;
            let compressed_len = compressed_bytes.len();
            let (image, metadata) = render_sized_bytes(
                compressed_bytes.as_bytes(),
                &svg_renderer,
                image_config,
                target,
                object_fit,
            )?;
            Ok((image, metadata, compressed_len))
        }
    }
}

fn render_sized_bytes(
    bytes: &[u8],
    svg_renderer: &crate::SvgRenderer,
    image_config: crate::AnimatedImageConfig,
    target: ImageRenderSize,
    object_fit: ObjectFit,
) -> Result<(RenderImage, crate::ImageRenderInfo)> {
    if let Ok(format) = image::guess_format(bytes) {
        return EncodedImage::new(format, Arc::<[u8]>::from(bytes)).render_sized(
            target,
            object_fit,
            image_config,
        );
    }

    let natural_size = svg_renderer.natural_size(bytes)?;
    let fitted_target = target.fit(
        natural_size.map(|dimension| u32::from(dimension)),
        object_fit,
    );
    let pixmap = svg_renderer.render_pixmap(bytes, SvgSize::Size(fitted_target.size()))?;
    let mut buffer = ImageBuffer::from_raw(pixmap.width(), pixmap.height(), pixmap.take())
        .ok_or_else(|| anyhow::anyhow!("invalid SVG raster dimensions"))?;

    for pixel in buffer.chunks_exact_mut(4) {
        swap_rgba_pa_to_bgra(pixel);
    }

    Ok((
        RenderImage::from_resident_frames(SmallVec::from_elem(
            AnimatedFrame::from_bgra_image(0, buffer),
            1,
        )),
        crate::ImageRenderInfo {
            original_width: u32::from(natural_size.width),
            original_height: u32::from(natural_size.height),
            size: fitted_target,
            render_path: "svg_target_raster",
        },
    ))
}

async fn load_image_resource_data(
    resource: AssetLocation,
    client: Arc<dyn http_client::HttpClient>,
    asset_source: Arc<dyn crate::AssetSource>,
) -> Result<ResourceImageBytes, ImageCacheError> {
    Ok(match resource {
        AssetLocation::Path(uri) => ResourceImageBytes::Owned(fs::read(uri.as_ref())?),
        AssetLocation::Uri(uri) => {
            let mut response = client
                .get(uri.as_ref(), ().into(), true)
                .await
                .with_context(|| format!("loading image asset from {uri:?}"))?;
            let mut body = Vec::new();
            response.body_mut().read_to_end(&mut body).await?;
            if !response.status().is_success() {
                let mut body = String::from_utf8_lossy(&body).into_owned();
                let first_line = body.lines().next().unwrap_or("").trim_end();
                body.truncate(first_line.len());
                return Err(ImageCacheError::BadStatus {
                    uri,
                    status: response.status(),
                    body,
                });
            }
            ResourceImageBytes::Owned(body)
        }
        AssetLocation::Embedded(path) => {
            let data = asset_source.load(&path).ok().flatten();
            if let Some(data) = data {
                match data {
                    Cow::Borrowed(bytes) => ResourceImageBytes::Static(bytes),
                    Cow::Owned(bytes) => ResourceImageBytes::Owned(bytes),
                }
            } else {
                return Err(ImageCacheError::Asset(
                    format!("Embedded resource not found: {path}").into(),
                ));
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_bounds_dimension_is_bucketed() {
        assert_eq!(bucket_image_dimension(1), 16);
        assert_eq!(bucket_image_dimension(38), 48);
        assert_eq!(bucket_image_dimension(800), 800);
    }

    #[test]
    fn compressed_cache_is_bounded_and_updates_lru_order() {
        let mut cache = CompressedCache::new();
        cache.max_bytes = 8;
        cache.insert(1, Arc::from(vec![1_u8; 4]));
        cache.insert(2, Arc::from(vec![2_u8; 4]));
        assert!(cache.get(1).is_some());

        cache.insert(3, Arc::from(vec![3_u8; 4]));

        assert!(cache.entries.contains_key(&1));
        assert!(!cache.entries.contains_key(&2));
        assert!(cache.entries.contains_key(&3));
        assert_eq!(cache.retained_bytes, 8);
    }
}
