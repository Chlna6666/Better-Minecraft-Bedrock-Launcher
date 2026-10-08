use super::element::*;
use super::error::ImageCacheError;
use crate::{
    AnyImageCache, App, Asset, AssetLocation, AssetLogger, ClipboardImage, CompressedImageBytes,
    RenderImage, SharedString, SharedUri, Window, hash,
};
use anyhow::Result;
use futures::Future;
use std::{
    any::TypeId,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

pub(super) enum ResourceImageBytes {
    Static(&'static [u8]),
    Owned(Vec<u8>),
}

impl ResourceImageBytes {
    pub(super) fn into_compressed_image_bytes(self) -> CompressedImageBytes {
        match self {
            Self::Static(bytes) => CompressedImageBytes::Static(bytes),
            Self::Owned(bytes) => CompressedImageBytes::from(bytes),
        }
    }
}

pub(super) enum SizedImageInput {
    PreloadedBytes(CompressedImagePreload),
}

/// A source of image content.
#[derive(Clone)]
pub enum ImageSource {
    /// The image content will be loaded from some resource location
    Asset(AssetLocation),
    /// A decoded image ready for rendering.
    RenderImage(Arc<RenderImage>),
    /// A clipboard image that GPUI can decode and cache.
    Clipboard(Arc<ClipboardImage>),
    /// Encoded image bytes from memory
    Encoded(EncodedImageBytes),
    /// A custom loading function to use
    Loader(Arc<dyn Fn(&mut Window, &mut App) -> Option<Result<Arc<RenderImage>, ImageCacheError>>>),
}

fn is_uri(uri: &str) -> bool {
    http_client::Uri::from_str(uri).is_ok()
}

impl From<SharedUri> for ImageSource {
    fn from(value: SharedUri) -> Self {
        Self::Asset(AssetLocation::Uri(value))
    }
}

impl<'a> From<&'a str> for ImageSource {
    fn from(s: &'a str) -> Self {
        if Path::new(s).is_absolute() {
            Self::Asset(PathBuf::from(s).into())
        } else if is_uri(s) {
            Self::Asset(AssetLocation::Uri(s.to_string().into()))
        } else {
            Self::Asset(AssetLocation::Embedded(s.to_string().into()))
        }
    }
}

impl From<String> for ImageSource {
    fn from(s: String) -> Self {
        if Path::new(&s).is_absolute() {
            Self::Asset(PathBuf::from(s).into())
        } else if is_uri(&s) {
            Self::Asset(AssetLocation::Uri(s.into()))
        } else {
            Self::Asset(AssetLocation::Embedded(s.into()))
        }
    }
}

impl From<SharedString> for ImageSource {
    fn from(s: SharedString) -> Self {
        s.as_ref().into()
    }
}

impl From<&Path> for ImageSource {
    fn from(value: &Path) -> Self {
        Self::Asset(value.to_path_buf().into())
    }
}

impl From<Arc<Path>> for ImageSource {
    fn from(value: Arc<Path>) -> Self {
        Self::Asset(value.into())
    }
}

impl From<PathBuf> for ImageSource {
    fn from(value: PathBuf) -> Self {
        Self::Asset(value.into())
    }
}

impl From<Arc<RenderImage>> for ImageSource {
    fn from(value: Arc<RenderImage>) -> Self {
        Self::RenderImage(value)
    }
}

impl From<Arc<ClipboardImage>> for ImageSource {
    fn from(value: Arc<ClipboardImage>) -> Self {
        Self::Clipboard(value)
    }
}

impl From<EncodedImageBytes> for ImageSource {
    fn from(value: EncodedImageBytes) -> Self {
        Self::Encoded(value)
    }
}

impl<F> From<F> for ImageSource
where
    F: Fn(&mut Window, &mut App) -> Option<Result<Arc<RenderImage>, ImageCacheError>> + 'static,
{
    fn from(value: F) -> Self {
        Self::Loader(Arc::new(value))
    }
}

impl ImageSource {
    pub(crate) fn use_render_image(
        &self,
        cache: Option<AnyImageCache>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        match self {
            ImageSource::Asset(resource) => {
                if let Some(cache) = cache {
                    cache.load(resource, window, cx)
                } else {
                    window.use_asset::<ResourceImageLoader>(resource, cx)
                }
            }
            ImageSource::Loader(loading_fn) => loading_fn(window, cx),
            ImageSource::RenderImage(render_image) => Some(Ok(render_image.to_owned())),
            ImageSource::Clipboard(clipboard_image) => {
                window.use_asset::<AssetLogger<ClipboardImageLoader>>(clipboard_image, cx)
            }
            ImageSource::Encoded(encoded_image) => {
                window.use_asset::<AssetLogger<EncodedImageLoader>>(encoded_image, cx)
            }
        }
    }

    pub(crate) fn render_image(
        &self,
        cache: Option<AnyImageCache>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        match self {
            ImageSource::Asset(resource) => {
                if let Some(cache) = cache {
                    cache.load(resource, window, cx)
                } else {
                    window.asset::<ResourceImageLoader>(resource, cx)
                }
            }
            ImageSource::Loader(loading_fn) => loading_fn(window, cx),
            ImageSource::RenderImage(render_image) => Some(Ok(render_image.to_owned())),
            ImageSource::Clipboard(clipboard_image) => {
                window.asset::<AssetLogger<ClipboardImageLoader>>(clipboard_image, cx)
            }
            ImageSource::Encoded(encoded_image) => {
                window.asset::<AssetLogger<EncodedImageLoader>>(encoded_image, cx)
            }
        }
    }

    /// Remove this image source from the asset system
    pub fn remove_asset(&self, cx: &mut App) {
        match self {
            ImageSource::Asset(resource) => {
                if let Some(preload) = cx.take_asset::<ResourceImageLoader>(resource)
                    && let Some(Ok(image)) = preload.get()
                {
                    cx.drop_image(image, None);
                }
            }
            ImageSource::Loader(_) | ImageSource::RenderImage(_) => {}
            ImageSource::Clipboard(clipboard_image) => {
                if let Some(preload) =
                    cx.take_asset::<AssetLogger<ClipboardImageLoader>>(clipboard_image)
                    && let Some(Ok(image)) = preload.get()
                {
                    cx.drop_image(image, None);
                }
            }
            ImageSource::Encoded(encoded_image) => {
                if let Some(preload) =
                    cx.take_asset::<AssetLogger<EncodedImageLoader>>(encoded_image)
                    && let Some(Ok(image)) = preload.get()
                {
                    cx.drop_image(image, None);
                }
            }
        }
    }
}

/// Encoded image bytes that can be loaded through GPUI's image asset system.
#[derive(Clone, Debug)]
pub struct EncodedImageBytes {
    format: crate::ImageFormat,
    bytes: CompressedImageBytes,
}

impl PartialEq for EncodedImageBytes {
    fn eq(&self, other: &Self) -> bool {
        self.format == other.format && self.bytes.as_bytes() == other.bytes.as_bytes()
    }
}

impl Eq for EncodedImageBytes {}

impl std::hash::Hash for EncodedImageBytes {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&self.format, state);
        std::hash::Hash::hash(self.bytes.as_bytes(), state);
    }
}

impl EncodedImageBytes {
    /// Creates an encoded source, retaining static, shared, or owned compressed storage.
    ///
    /// Owned vectors preserve their allocation and spare capacity. Clones share the payload;
    /// equality and asset-cache hashing compare format and contents regardless of storage type.
    /// Non-static borrowed input must first be copied into an owned vector or shared slice.
    pub fn new(format: crate::ImageFormat, bytes: impl Into<CompressedImageBytes>) -> Self {
        Self {
            format,
            bytes: bytes.into(),
        }
    }

    fn render(
        &self,
        renderer: crate::SvgRenderer,
        config: crate::AnimatedImageConfig,
    ) -> crate::Result<RenderImage> {
        Ok(match self.format {
            crate::ImageFormat::Svg => {
                let pixmap = renderer
                    .render_pixmap(self.bytes.as_ref(), crate::SvgSize::ScaleFactor(1.0))?;
                let mut buffer =
                    image::ImageBuffer::from_raw(pixmap.width(), pixmap.height(), pixmap.take())
                        .ok_or_else(|| anyhow::anyhow!("invalid SVG raster dimensions"))?;
                crate::swap_rgba_pa_to_bgra_buffer(buffer.as_mut());
                RenderImage::new(smallvec::SmallVec::from_elem(image::Frame::new(buffer), 1))
            }
            format => crate::EncodedImage::new(
                match format {
                    crate::ImageFormat::Png => image::ImageFormat::Png,
                    crate::ImageFormat::Jpeg => image::ImageFormat::Jpeg,
                    crate::ImageFormat::Webp => image::ImageFormat::WebP,
                    crate::ImageFormat::Gif => image::ImageFormat::Gif,
                    crate::ImageFormat::Bmp => image::ImageFormat::Bmp,
                    crate::ImageFormat::Tiff => image::ImageFormat::Tiff,
                    crate::ImageFormat::Svg => unreachable!("SVG was handled above"),
                },
                self.bytes.clone(),
            )
            .render(config)?,
        })
    }

    /// Hashes a cheap identity for this source: the format plus the byte buffer's address and
    /// length rather than its contents.
    ///
    /// This is stable across frames as long as the same source (or clones of it) is reused,
    /// which is how element ids are expected to behave; two different allocations holding
    /// identical bytes hash differently, which is acceptable for id derivation and avoids
    /// re-hashing potentially megabytes of compressed data every frame.
    pub(crate) fn hash_identity(&self, hasher: &mut impl std::hash::Hasher) {
        use std::hash::Hash;

        self.format.hash(hasher);
        (self.bytes.as_bytes().as_ptr() as usize).hash(hasher);
        self.bytes.len().hash(hasher);
    }
}

#[derive(Clone)]
pub(crate) enum ClipboardImageLoader {}

impl Asset for ClipboardImageLoader {
    type Source = Arc<ClipboardImage>;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let renderer = cx.svg_renderer();
        let config = cx.image_pipeline_config().animated;
        async move {
            let mut image = source.to_render_image_with_config(renderer, config)?;
            // `ClipboardImage::hash` is derived from its content, so processing the same image after
            // an eviction can reuse the previous ImageId and its resident atlas tiles. The
            // decode returns a freshly created Arc, so `get_mut` normally succeeds; if it
            // ever does not, we conservatively keep the auto-assigned id.
            if let Some(image) = Arc::get_mut(&mut image) {
                image.id = crate::interned_render_image_id(
                    TypeId::of::<ClipboardImageLoader>(),
                    hash(&source),
                );
            }
            Ok(image)
        }
    }
}

#[derive(Clone)]
pub(crate) enum EncodedImageLoader {}

impl Asset for EncodedImageLoader {
    type Source = EncodedImageBytes;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let renderer = cx.svg_renderer();
        let config = cx.image_pipeline_config().animated;
        async move {
            let processing_started = std::time::Instant::now();
            let data = source.render(renderer, config)?;
            let processing_duration = processing_started.elapsed();
            crate::record_image_processing_metrics_with_threshold(
                source.bytes.len(),
                data.resident_byte_len(),
                data.frame_count(),
                processing_duration,
                crate::ImagePipelineConfig::default().slow_image_threshold,
            );
            let mut image =
                Arc::new(data.with_processing_metrics(source.bytes.len(), processing_duration));
            // The source hash covers the format and the encoded bytes, so a re-decode after
            // an eviction produces identical frames and can reuse the previous ImageId.
            if let Some(image) = Arc::get_mut(&mut image) {
                image.id = crate::interned_render_image_id(
                    TypeId::of::<EncodedImageLoader>(),
                    hash(&source),
                );
            }
            Ok(image)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_resource_preserves_compressed_allocation() {
        let bytes = vec![1, 2, 3, 4];
        let pointer = bytes.as_ptr();
        let compressed = ResourceImageBytes::Owned(bytes).into_compressed_image_bytes();
        assert_eq!(compressed.as_bytes().as_ptr(), pointer);
        assert!(matches!(compressed, CompressedImageBytes::Owned(_)));
    }

    #[test]
    fn inline_owned_sources_preserve_allocation_and_content_cache_keys() {
        static BYTES: &[u8] = &[1, 2, 3, 4];
        let bytes = BYTES.to_vec();
        let pointer = bytes.as_ptr();
        let owned = EncodedImageBytes::new(crate::ImageFormat::Png, bytes);
        assert_eq!(owned.bytes.as_bytes().as_ptr(), pointer);
        let cloned = owned.clone();
        assert_eq!(cloned.bytes.as_bytes().as_ptr(), pointer);
        for bytes in [
            CompressedImageBytes::Static(BYTES),
            CompressedImageBytes::Shared(Arc::from(BYTES)),
        ] {
            let other = EncodedImageBytes::new(crate::ImageFormat::Png, bytes);
            assert_eq!(owned, other);
            assert_eq!(hash(&owned), hash(&other));
        }
    }

    #[test]
    fn shared_encoded_svg_keeps_the_svg_decode_path() {
        let source = EncodedImageBytes::new(
            crate::ImageFormat::Svg,
            Arc::<[u8]>::from(br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="2" height="2" fill="red"/></svg>"#.as_slice()),
        );
        let image = source
            .render(
                crate::SvgRenderer::new(Arc::new(())),
                crate::AnimatedImageConfig::default(),
            )
            .expect("shared SVG should decode without the clipboard adapter");
        let frame = image.frame(0).expect("SVG should have a resident frame");
        assert_eq!(frame.bytes().len(), 16);
        assert_eq!(frame.bytes(), &[0, 0, 255, 255].repeat(4));
    }

    #[test]
    fn inline_svg_unpremultiplies_translucent_colors_in_place() {
        let bytes = br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="2" height="2" fill="red" opacity="0.5"/></svg>"#.to_vec();
        let source = EncodedImageBytes::new(crate::ImageFormat::Svg, bytes);
        let image = source
            .render(
                crate::SvgRenderer::new(Arc::new(())),
                crate::AnimatedImageConfig::default(),
            )
            .unwrap();
        // Match the resource SVG path's existing floating-point truncation exactly.
        assert_eq!(image.as_bytes(0).unwrap(), &[0, 0, 254, 128].repeat(4));
    }

    #[test]
    fn shared_encoded_png_keeps_the_raster_decode_path() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("test PNG should encode");
        let source = EncodedImageBytes::new(crate::ImageFormat::Png, bytes.into_inner());
        let image = source
            .render(
                crate::SvgRenderer::new(Arc::new(())),
                crate::AnimatedImageConfig::default(),
            )
            .expect("shared PNG should decode without the clipboard adapter");
        let frame = image.frame(0).expect("PNG should have a resident frame");
        assert_eq!(frame.bytes(), &[0, 0, 255, 255].repeat(4));
    }
}
