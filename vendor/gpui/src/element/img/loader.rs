use super::element::*;
use super::error::ImageCacheError;
use super::source::*;
use crate::{
    AnimatedFrame, App, Asset, Bounds, ImageDecodeRecord, ImageDecodeTarget, ObjectFit, Pixels,
    RenderImage, Resource, SMOOTH_SVG_SCALE_FACTOR, SharedString, Size, SvgSize, Window,
    decode_image_bytes, decode_image_bytes_to_target, decode_image_path_to_target,
    fitted_target_size, hash, record_image_asset_retained,
    record_image_decode_metrics_with_threshold, swap_rgba_pa_to_bgra,
};
use anyhow::{Context as _, Result};
use futures::{AsyncReadExt, Future};
use image::{Frame, ImageBuffer};
use smallvec::SmallVec;
use std::{borrow::Cow, fs, sync::Arc, time::Instant};

/// Resource image source plus the device-pixel decode target for bounds-aware loading.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TargetSizeImageSource {
    resource: Resource,
    target: ImageDecodeTarget,
    scale_factor_bits: u32,
    object_fit: ObjectFit,
    diagnostic_label: SharedString,
}

impl TargetSizeImageSource {
    pub(crate) fn new(
        resource: Resource,
        target: ImageDecodeTarget,
        scale_factor: f32,
        object_fit: ObjectFit,
    ) -> Self {
        let scale_factor = normalize_decode_scale_factor(scale_factor);
        Self {
            diagnostic_label: resource_diagnostic_label(&resource),
            resource,
            target,
            scale_factor_bits: scale_factor.to_bits(),
            object_fit,
        }
    }

    /// Returns the image resource backing this target-size source.
    pub fn resource(&self) -> &Resource {
        &self.resource
    }
}

/// Resource image source used to cache compressed bytes before size-specific decoding.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CompressedImageSource {
    resource: Resource,
}

impl CompressedImageSource {
    pub(crate) fn new(resource: Resource) -> Self {
        Self { resource }
    }
}

fn resource_diagnostic_label(resource: &Resource) -> SharedString {
    match resource {
        Resource::Path(path) => path.to_string_lossy().into_owned().into(),
        Resource::Uri(uri) => uri.to_string().into(),
        Resource::Embedded(path) => path.clone(),
    }
}

/// Asset loader for compressed image bytes reused across multiple size-specific decodes.
#[derive(Clone)]
pub enum CompressedImageAssetLoader {}

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
            let source_bytes =
                load_image_resource_data(source.resource, client, asset_source).await?;
            Ok(source_bytes.into_compressed_image_bytes())
        }
    }
}

pub(crate) fn target_size_for_decode(
    logical_size: Size<Pixels>,
    scale_factor: f32,
) -> Option<ImageDecodeTarget> {
    let scale_factor = normalize_decode_scale_factor(scale_factor);
    let size = logical_size.to_device_pixels(scale_factor);
    let width = u32::try_from(size.width.0.max(0)).ok()?;
    let height = u32::try_from(size.height.0.max(0)).ok()?;
    let overscan = decode_overscan_factor(width, height);
    ImageDecodeTarget::new(
        bucket_decode_dimension(((width as f32) * overscan).ceil() as u32),
        bucket_decode_dimension(((height as f32) * overscan).ceil() as u32),
    )
}

fn normalize_decode_scale_factor(scale_factor: f32) -> f32 {
    const SCALE_FACTOR_BUCKETS_PER_UNIT: f32 = 1024.0;

    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return 1.0;
    }

    (scale_factor * SCALE_FACTOR_BUCKETS_PER_UNIT).round() / SCALE_FACTOR_BUCKETS_PER_UNIT
}

pub(super) fn target_size_for_bounds(
    bounds: Bounds<Pixels>,
    window: &Window,
) -> Option<ImageDecodeTarget> {
    target_size_for_decode(bounds.size, window.scale_factor())
}

fn decode_overscan_factor(width: u32, height: u32) -> f32 {
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

fn bucket_decode_dimension(value: u32) -> u32 {
    const BUCKET: u32 = 16;
    value.max(1).div_ceil(BUCKET) * BUCKET
}

/// An image loader for the GPUI asset system
#[derive(Clone)]
pub enum ImageAssetLoader {}

impl Asset for ImageAssetLoader {
    type Source = Resource;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        // TODO: Can we make SVGs always rescale?
        // let scale_factor = cx.scale_factor();
        let svg_renderer = cx.svg_renderer();
        let asset_source = cx.asset_source().clone();
        let pipeline_config = cx.image_pipeline_config();
        let image_config = pipeline_config.animated;
        let slow_decode_threshold = pipeline_config.slow_decode_threshold;
        let background_executor = cx.background_executor().clone();
        async move {
            let source_bytes =
                load_image_resource_data(source.clone(), client, asset_source).await?;
            let bytes = source_bytes.as_bytes();
            let compressed_len = source_bytes.len();

            let decode_started = Instant::now();
            let mut data = if let Ok(format) = image::guess_format(&bytes) {
                decode_image_bytes(
                    bytes,
                    format,
                    image_config,
                    Some(background_executor.clone()),
                )?
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

            let decode_duration = decode_started.elapsed();
            data = data.with_pipeline_metadata(compressed_len, decode_duration);
            record_image_decode_metrics_with_threshold(
                compressed_len,
                data.decoded_byte_len(),
                data.frame_count(),
                decode_duration,
                slow_decode_threshold,
            );
            if decode_duration >= slow_decode_threshold {
                log::debug!(
                    "slow image decode: source={source:?} compressed_bytes={} decoded_bytes={} frames={} decode_ms={:.3}",
                    compressed_len,
                    data.decoded_byte_len(),
                    data.frame_count(),
                    decode_duration.as_secs_f64() * 1000.0
                );
            }

            Ok(Arc::new(data))
        }
    }
}

/// Asset loader for resource images decoded to an element's current paint bounds.
#[derive(Clone)]
pub enum TargetSizeImageAssetLoader {}

impl Asset for TargetSizeImageAssetLoader {
    type Source = TargetSizeImageSource;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let svg_renderer = cx.svg_renderer();
        let pipeline_config = cx.image_pipeline_config();
        let image_config = pipeline_config.animated;
        let slow_decode_threshold = pipeline_config.slow_decode_threshold;
        let decode_source = cx
            .cached_asset_task::<CompressedImgResourceLoader>(&CompressedImageSource {
                resource: source.resource.clone(),
            })
            .map_or_else(
                || TargetSizeImageDecodeSource::Resource(source.resource.clone()),
                TargetSizeImageDecodeSource::PreloadedBytes,
            );
        let client = cx.http_client();
        let asset_source = cx.asset_source().clone();
        async move {
            let decode_started = Instant::now();
            let scale_factor = f32::from_bits(source.scale_factor_bits);
            let (mut data, metadata, compressed_len) = decode_target_size_image_from_source(
                decode_source,
                client,
                asset_source,
                svg_renderer,
                image_config,
                source.target,
                source.object_fit,
            )
            .await?;

            let decode_duration = decode_started.elapsed();
            data = data
                .with_scale_factor(scale_factor)
                .with_pipeline_metadata(compressed_len, decode_duration);
            record_image_decode_metrics_with_threshold(
                compressed_len,
                data.decoded_byte_len(),
                data.frame_count(),
                decode_duration,
                slow_decode_threshold,
            );

            let image = Arc::new(data);
            record_image_asset_retained(
                hash(&source),
                ImageDecodeRecord {
                    source: source.diagnostic_label.to_string(),
                    original_width: metadata.original_width,
                    original_height: metadata.original_height,
                    target_width: metadata.target.width,
                    target_height: metadata.target.height,
                    retained_decoded_bytes: image.decoded_byte_len(),
                    decode_mode: metadata.decode_mode.to_string(),
                },
            );

            Ok(image)
        }
    }
}

async fn decode_target_size_image_from_source(
    source: TargetSizeImageDecodeSource,
    client: Arc<dyn http_client::HttpClient>,
    asset_source: Arc<dyn crate::AssetSource>,
    svg_renderer: crate::SvgRenderer,
    image_config: crate::AnimatedImageConfig,
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<(RenderImage, crate::TargetImageDecodeMetadata, usize), ImageCacheError> {
    match source {
        TargetSizeImageDecodeSource::PreloadedBytes(compressed_task) => {
            let compressed_bytes = compressed_task.await?;
            let compressed_len = compressed_bytes.len();
            let (image, metadata) = decode_target_size_image_bytes(
                compressed_bytes.as_bytes(),
                &svg_renderer,
                image_config,
                target,
                object_fit,
            )?;
            Ok((image, metadata, compressed_len))
        }
        TargetSizeImageDecodeSource::Resource(Resource::Path(path)) => {
            match decode_image_path_to_target(path.as_ref(), image_config, target, object_fit) {
                Ok((image, metadata)) => {
                    let compressed_len = fs::metadata(path.as_ref())
                        .map(|metadata| usize::try_from(metadata.len()).unwrap_or(usize::MAX))
                        .unwrap_or(0);
                    Ok((image, metadata, compressed_len))
                }
                Err(path_decode_error) => {
                    let bytes = fs::read(path.as_ref()).map_err(ImageCacheError::from)?;
                    let (image, metadata) = decode_target_size_image_bytes(
                        &bytes,
                        &svg_renderer,
                        image_config,
                        target,
                        object_fit,
                    )
                    .map_err(|error| {
                        ImageCacheError::Asset(
                            format!(
                                "failed to decode target-size path image: {path_decode_error}; \
fallback decode failed: {error}"
                            )
                            .into(),
                        )
                    })?;
                    Ok((image, metadata, bytes.len()))
                }
            }
        }
        TargetSizeImageDecodeSource::Resource(resource) => {
            let source_bytes = load_image_resource_data(resource, client, asset_source).await?;
            let bytes = source_bytes.as_bytes();
            let compressed_len = source_bytes.len();
            let (image, metadata) = decode_target_size_image_bytes(
                bytes,
                &svg_renderer,
                image_config,
                target,
                object_fit,
            )?;
            Ok((image, metadata, compressed_len))
        }
    }
}

fn decode_target_size_image_bytes(
    bytes: &[u8],
    svg_renderer: &crate::SvgRenderer,
    image_config: crate::AnimatedImageConfig,
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<(RenderImage, crate::TargetImageDecodeMetadata)> {
    if let Ok(format) = image::guess_format(bytes) {
        return decode_image_bytes_to_target(bytes, format, image_config, target, object_fit);
    }

    let natural_size = svg_renderer.natural_size(bytes)?;
    let fitted_target = fitted_target_size(
        natural_size.map(|dimension| u32::from(dimension)),
        target,
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
        crate::TargetImageDecodeMetadata {
            original_width: u32::from(natural_size.width),
            original_height: u32::from(natural_size.height),
            target: fitted_target,
            decode_mode: "svg_target_raster",
        },
    ))
}

async fn load_image_resource_data(
    resource: Resource,
    client: Arc<dyn http_client::HttpClient>,
    asset_source: Arc<dyn crate::AssetSource>,
) -> Result<ResourceImageBytes, ImageCacheError> {
    Ok(match resource {
        Resource::Path(uri) => ResourceImageBytes::Owned(fs::read(uri.as_ref())?),
        Resource::Uri(uri) => {
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
        Resource::Embedded(path) => {
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
    fn decode_bounds_dimension_is_bucketed() {
        assert_eq!(bucket_decode_dimension(1), 16);
        assert_eq!(bucket_decode_dimension(38), 48);
        assert_eq!(bucket_decode_dimension(800), 800);
    }
}
