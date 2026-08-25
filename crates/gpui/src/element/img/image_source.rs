use super::element::*;
use super::error::ImageCacheError;
use crate::{
    AnyImageCache, App, Asset, AssetLogger, Image, RenderImage, Resource, SharedString, SharedUri,
    Window, hash,
};
use anyhow::Result;
use futures::{Future, FutureExt};
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
            Self::Owned(bytes) => CompressedImageBytes::Shared(Arc::from(bytes)),
        }
    }
}

/// Compressed image bytes retained by GPUI for bounds-aware decode reuse.
#[derive(Clone)]
pub enum CompressedImageBytes {
    /// Statically embedded image bytes borrowed directly from the asset source.
    Static(&'static [u8]),
    /// Shared owned bytes retained for file or network-backed image resources.
    Shared(Arc<[u8]>),
}

impl CompressedImageBytes {
    /// Returns the compressed image bytes as a borrowed slice.
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Static(bytes) => bytes,
            Self::Shared(bytes) => bytes.as_ref(),
        }
    }

    /// Returns the number of compressed bytes retained by this value.
    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }
}

pub(super) enum TargetSizeImageDecodeSource {
    PreloadedBytes(CompressedImageLoadingTask),
}

/// A source of image content.
#[derive(Clone)]
pub enum ImageSource {
    /// The image content will be loaded from some resource location
    Resource(Resource),
    /// Cached image data
    Render(Arc<RenderImage>),
    /// Cached image data
    Image(Arc<Image>),
    /// Encoded image bytes from memory
    Bytes(EncodedImageBytes),
    /// A custom loading function to use
    Custom(Arc<dyn Fn(&mut Window, &mut App) -> Option<Result<Arc<RenderImage>, ImageCacheError>>>),
}

fn is_uri(uri: &str) -> bool {
    http_client::Uri::from_str(uri).is_ok()
}

impl From<SharedUri> for ImageSource {
    fn from(value: SharedUri) -> Self {
        Self::Resource(Resource::Uri(value))
    }
}

impl<'a> From<&'a str> for ImageSource {
    fn from(s: &'a str) -> Self {
        if Path::new(s).is_absolute() {
            Self::Resource(PathBuf::from(s).into())
        } else if is_uri(s) {
            Self::Resource(Resource::Uri(s.to_string().into()))
        } else {
            Self::Resource(Resource::Embedded(s.to_string().into()))
        }
    }
}

impl From<String> for ImageSource {
    fn from(s: String) -> Self {
        if Path::new(&s).is_absolute() {
            Self::Resource(PathBuf::from(s).into())
        } else if is_uri(&s) {
            Self::Resource(Resource::Uri(s.into()))
        } else {
            Self::Resource(Resource::Embedded(s.into()))
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
        Self::Resource(value.to_path_buf().into())
    }
}

impl From<Arc<Path>> for ImageSource {
    fn from(value: Arc<Path>) -> Self {
        Self::Resource(value.into())
    }
}

impl From<PathBuf> for ImageSource {
    fn from(value: PathBuf) -> Self {
        Self::Resource(value.into())
    }
}

impl From<Arc<RenderImage>> for ImageSource {
    fn from(value: Arc<RenderImage>) -> Self {
        Self::Render(value)
    }
}

impl From<Arc<Image>> for ImageSource {
    fn from(value: Arc<Image>) -> Self {
        Self::Image(value)
    }
}

impl From<EncodedImageBytes> for ImageSource {
    fn from(value: EncodedImageBytes) -> Self {
        Self::Bytes(value)
    }
}

impl<F> From<F> for ImageSource
where
    F: Fn(&mut Window, &mut App) -> Option<Result<Arc<RenderImage>, ImageCacheError>> + 'static,
{
    fn from(value: F) -> Self {
        Self::Custom(Arc::new(value))
    }
}

impl ImageSource {
    pub(crate) fn use_data(
        &self,
        cache: Option<AnyImageCache>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        match self {
            ImageSource::Resource(resource) => {
                if let Some(cache) = cache {
                    cache.load(resource, window, cx)
                } else {
                    window.use_asset::<ImgResourceLoader>(resource, cx)
                }
            }
            ImageSource::Custom(loading_fn) => loading_fn(window, cx),
            ImageSource::Render(data) => Some(Ok(data.to_owned())),
            ImageSource::Image(data) => window.use_asset::<AssetLogger<ImageDecoder>>(data, cx),
            ImageSource::Bytes(data) => {
                window.use_asset::<AssetLogger<EncodedImageDecoder>>(data, cx)
            }
        }
    }

    pub(crate) fn data(
        &self,
        cache: Option<AnyImageCache>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        match self {
            ImageSource::Resource(resource) => {
                if let Some(cache) = cache {
                    cache.load(resource, window, cx)
                } else {
                    window.asset::<ImgResourceLoader>(resource, cx)
                }
            }
            ImageSource::Custom(loading_fn) => loading_fn(window, cx),
            ImageSource::Render(data) => Some(Ok(data.to_owned())),
            ImageSource::Image(data) => window.asset::<AssetLogger<ImageDecoder>>(data, cx),
            ImageSource::Bytes(data) => window.asset::<AssetLogger<EncodedImageDecoder>>(data, cx),
        }
    }

    /// Remove this image source from the asset system
    pub fn remove_asset(&self, cx: &mut App) {
        match self {
            ImageSource::Resource(resource) => {
                if let Some(task) = cx.take_asset::<ImgResourceLoader>(resource)
                    && let Some(Ok(image)) = task.now_or_never()
                {
                    cx.drop_image(image, None);
                }
            }
            ImageSource::Custom(_) | ImageSource::Render(_) => {}
            ImageSource::Image(data) => {
                if let Some(task) = cx.take_asset::<AssetLogger<ImageDecoder>>(data)
                    && let Some(Ok(image)) = task.now_or_never()
                {
                    cx.drop_image(image, None);
                }
            }
            ImageSource::Bytes(data) => {
                if let Some(task) = cx.take_asset::<AssetLogger<EncodedImageDecoder>>(data)
                    && let Some(Ok(image)) = task.now_or_never()
                {
                    cx.drop_image(image, None);
                }
            }
        }
    }
}

/// Encoded image bytes that can be loaded through GPUI's image asset system.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EncodedImageBytes {
    format: crate::ImageFormat,
    bytes: Arc<[u8]>,
}

impl EncodedImageBytes {
    /// Creates an encoded image source from an image format and compressed bytes.
    pub fn new(format: crate::ImageFormat, bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            format,
            bytes: bytes.into(),
        }
    }

    /// Hashes a cheap identity for this source: the format plus the byte buffer's address and
    /// length rather than its contents.
    ///
    /// This is stable across frames as long as the same `Arc` (or clones of it) is reused,
    /// which is how element ids are expected to behave; two different allocations holding
    /// identical bytes hash differently, which is acceptable for id derivation and avoids
    /// re-hashing potentially megabytes of compressed data every frame.
    pub(crate) fn hash_identity(&self, hasher: &mut impl std::hash::Hasher) {
        use std::hash::Hash;

        self.format.hash(hasher);
        (Arc::as_ptr(&self.bytes) as *const u8 as usize).hash(hasher);
        self.bytes.len().hash(hasher);
    }
}

#[derive(Clone)]
pub(crate) enum ImageDecoder {}

impl Asset for ImageDecoder {
    type Source = Arc<Image>;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let renderer = cx.svg_renderer();
        let config = cx.image_pipeline_config().animated;
        let executor = cx.background_executor().clone();
        async move {
            let mut image = source.to_image_data_with_config(renderer, config, Some(executor))?;
            // `Image::hash` is derived from its content, so re-decoding the same image after
            // an eviction can reuse the previous ImageId and its resident atlas tiles. The
            // decode returns a freshly created Arc, so `get_mut` normally succeeds; if it
            // ever does not, we conservatively keep the auto-assigned id.
            if let Some(image) = Arc::get_mut(&mut image) {
                image.id =
                    crate::interned_render_image_id(TypeId::of::<ImageDecoder>(), hash(&source));
            }
            Ok(image)
        }
    }
}

#[derive(Clone)]
pub(crate) enum EncodedImageDecoder {}

impl Asset for EncodedImageDecoder {
    type Source = EncodedImageBytes;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let renderer = cx.svg_renderer();
        let config = cx.image_pipeline_config().animated;
        let executor = cx.background_executor().clone();
        async move {
            let decoded = Image::from_bytes(source.format, source.bytes.to_vec());
            let mut image = decoded.to_image_data_with_config(renderer, config, Some(executor))?;
            // The source hash covers the format and the encoded bytes, so a re-decode after
            // an eviction produces identical frames and can reuse the previous ImageId.
            if let Some(image) = Arc::get_mut(&mut image) {
                image.id = crate::interned_render_image_id(
                    TypeId::of::<EncodedImageDecoder>(),
                    hash(&source),
                );
            }
            Ok(image)
        }
    }
}
