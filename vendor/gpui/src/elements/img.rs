use crate::{
    AnimatedFrame, AnyElement, AnyImageCache, App, Asset, AssetLogger, BackgroundExecutor, Bounds,
    DefiniteLength, Element, ElementId, Entity, GlobalElementId, GpuiImageUsageScope,
    GpuiImageUsageScopeHandle, Hitbox, Image, ImageCache, ImageDecodeRecord, ImageDecodeTarget,
    ImageUsageKind, InspectorElementId, InteractiveElement, Interactivity, IntoElement, LayoutId,
    Length, ObjectFit, Pixels, RenderFingerprint, RenderImage, Resource, SMOOTH_SVG_SCALE_FACTOR,
    SharedString, SharedUri, StyleRefinement, Styled, SvgSize, Task, Window, decode_image_bytes,
    decode_image_bytes_to_target, drop_image_asset_retained, fitted_target_size, hash, px,
    record_image_asset_retained, record_image_decode_metrics_with_threshold, swap_rgba_pa_to_bgra,
};
use anyhow::{Context as _, Result};

use futures::{AsyncReadExt, Future, FutureExt};
use image::{Frame, ImageBuffer, ImageError};
use smallvec::SmallVec;
use std::{
    any::TypeId,
    fs,
    hash::Hash,
    io::{self},
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;
use util::ResultExt;

use super::{Stateful, StatefulInteractiveElement};

/// The delay before showing the loading state.
pub const LOADING_DELAY: Duration = Duration::from_millis(200);

/// A type alias to the resource loader that the `img()` element uses.
///
/// Note: that this is only for Resources, like URLs or file paths.
/// Custom loaders, or external images will not use this asset loader
pub type ImgResourceLoader = AssetLogger<ImageAssetLoader>;

/// Resource loader used when an image must be decoded for a concrete paint-bounds size.
pub type TargetSizeImgResourceLoader = AssetLogger<TargetSizeImageAssetLoader>;

pub(crate) type CompressedImgResourceLoader = AssetLogger<CompressedImageAssetLoader>;

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
        if is_uri(s) {
            Self::Resource(Resource::Uri(s.to_string().into()))
        } else {
            Self::Resource(Resource::Embedded(s.to_string().into()))
        }
    }
}

impl From<String> for ImageSource {
    fn from(s: String) -> Self {
        if is_uri(&s) {
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

/// The style of an image element.
pub struct ImageStyle {
    grayscale: bool,
    object_fit: ObjectFit,
    decode_to_bounds: bool,
    loading: Option<Box<dyn Fn() -> AnyElement>>,
    fallback: Option<Box<dyn Fn() -> AnyElement>>,
}

impl Default for ImageStyle {
    fn default() -> Self {
        Self {
            grayscale: false,
            object_fit: ObjectFit::Contain,
            decode_to_bounds: false,
            loading: None,
            fallback: None,
        }
    }
}

/// Per-image animation playback settings.
///
/// This allows individual image elements to pause animated media or cap playback
/// without changing the application-wide image pipeline defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageAnimationPolicy {
    /// Whether decoded animations should advance beyond their first frame.
    pub play: bool,
    /// Optional maximum playback rate while the window is active.
    pub max_fps: Option<f32>,
    /// Optional maximum playback rate while the window is inactive.
    pub inactive_max_fps: Option<f32>,
}

impl Default for ImageAnimationPolicy {
    fn default() -> Self {
        Self {
            play: true,
            max_fps: None,
            inactive_max_fps: None,
        }
    }
}

impl ImageAnimationPolicy {
    /// Use the application-wide animation settings.
    pub const fn inherit() -> Self {
        Self {
            play: true,
            max_fps: None,
            inactive_max_fps: None,
        }
    }

    /// Paint the first frame of animated images without requesting animation frames.
    pub const fn paused() -> Self {
        Self {
            play: false,
            max_fps: None,
            inactive_max_fps: None,
        }
    }

    /// Play animated images with an optional active-window frame-rate cap.
    pub const fn playing(max_fps: f32) -> Self {
        Self {
            play: true,
            max_fps: Some(max_fps),
            inactive_max_fps: None,
        }
    }

    fn apply_to(self, mut config: crate::AnimatedImageConfig) -> crate::AnimatedImageConfig {
        config.play = config.play && self.play;
        if let Some(max_fps) = self.max_fps {
            config.max_fps = max_fps;
        }
        if let Some(inactive_max_fps) = self.inactive_max_fps {
            config.inactive_max_fps = inactive_max_fps;
        }
        config
    }
}

/// Style an image element.
pub trait StyledImage: Sized {
    /// Get a mutable [ImageStyle] from the element.
    fn image_style(&mut self) -> &mut ImageStyle;

    /// Set the image to be displayed in grayscale.
    fn grayscale(mut self, grayscale: bool) -> Self {
        self.image_style().grayscale = grayscale;
        self
    }

    /// Set the object fit for the image.
    fn object_fit(mut self, object_fit: ObjectFit) -> Self {
        self.image_style().object_fit = object_fit;
        self
    }

    /// Set the object fit for the image.
    fn with_fallback(mut self, fallback: impl Fn() -> AnyElement + 'static) -> Self {
        self.image_style().fallback = Some(Box::new(fallback));
        self
    }

    /// Set the object fit for the image.
    fn with_loading(mut self, loading: impl Fn() -> AnyElement + 'static) -> Self {
        self.image_style().loading = Some(Box::new(loading));
        self
    }

    /// Decode resource images for the actual element bounds in device pixels.
    fn decode_to_bounds(mut self) -> Self {
        self.image_style().decode_to_bounds = true;
        self
    }
}

impl StyledImage for Img {
    fn image_style(&mut self) -> &mut ImageStyle {
        &mut self.style
    }
}

impl StyledImage for Stateful<Img> {
    fn image_style(&mut self) -> &mut ImageStyle {
        &mut self.element.style
    }
}

/// An image element.
pub struct Img {
    interactivity: Interactivity,
    source: ImageSource,
    style: ImageStyle,
    image_cache: Option<AnyImageCache>,
    animation_policy: ImageAnimationPolicy,
    usage: ImageUsageKind,
    usage_scope: Option<GpuiImageUsageScope>,
}

/// Create a new image element.
#[track_caller]
pub fn img(source: impl Into<ImageSource>) -> Img {
    Img {
        interactivity: Interactivity::new(),
        source: source.into(),
        style: ImageStyle::default(),
        image_cache: None,
        animation_policy: ImageAnimationPolicy::default(),
        usage: ImageUsageKind::UiImage,
        usage_scope: None,
    }
}

impl Img {
    /// A list of all format extensions currently supported by this img element
    pub fn extensions() -> &'static [&'static str] {
        &["jpg", "jpeg", "png", "apng", "gif", "webp", "bmp", "svg"]
    }

    /// Sets the image cache for the current node.
    ///
    /// If the `image_cache` is not explicitly provided, the function will determine the image cache by:
    ///
    /// 1. Checking if any ancestor node of the current node contains an `ImageCacheElement`, If such a node exists, the image cache specified by that ancestor will be used.
    /// 2. If no ancestor node contains an `ImageCacheElement`, the global image cache will be used as a fallback.
    ///
    /// This mechanism provides a flexible way to manage image caching, allowing precise control when needed,
    /// while ensuring a default behavior when no cache is explicitly specified.
    #[inline]
    pub fn image_cache<I: ImageCache>(self, image_cache: &Entity<I>) -> Self {
        Self {
            image_cache: Some(image_cache.clone().into()),
            ..self
        }
    }

    /// Sets the animation policy for this image element.
    #[inline]
    pub fn animation_policy(self, animation_policy: ImageAnimationPolicy) -> Self {
        Self {
            animation_policy,
            ..self
        }
    }

    /// Classifies this image for GPUI memory policy and diagnostics.
    #[inline]
    pub fn usage(self, usage: ImageUsageKind) -> Self {
        Self { usage, ..self }
    }

    /// Associates this image with an application-defined GPUI memory usage scope.
    #[inline]
    pub fn usage_scope(self, scope: impl Into<GpuiImageUsageScope>) -> Self {
        Self {
            usage_scope: Some(scope.into()),
            ..self
        }
    }

    /// Associates this image with a framework-owned GPUI memory usage scope handle.
    #[inline]
    pub fn usage_scope_handle(self, scope: &GpuiImageUsageScopeHandle) -> Self {
        self.usage_scope(scope.scope().clone())
    }

    fn fallback_element_id(&self) -> ElementId {
        let mut hasher = RenderFingerprint::new();
        if let Some(location) = self.interactivity.source_location() {
            location.file().hash(&mut hasher);
            location.line().hash(&mut hasher);
            location.column().hash(&mut hasher);
        }

        match &self.source {
            ImageSource::Resource(resource) => {
                0u8.hash(&mut hasher);
                resource.hash(&mut hasher);
            }
            ImageSource::Render(image) => {
                1u8.hash(&mut hasher);
                image.id.hash(&mut hasher);
            }
            ImageSource::Image(image) => {
                2u8.hash(&mut hasher);
                image.id().hash(&mut hasher);
            }
            ImageSource::Bytes(bytes) => {
                3u8.hash(&mut hasher);
                bytes.hash(&mut hasher);
            }
            ImageSource::Custom(load) => {
                4u8.hash(&mut hasher);
                Arc::as_ptr(load).hash(&mut hasher);
            }
        }

        ElementId::NamedInteger("img".into(), hasher.value())
    }

    fn should_decode_to_bounds(&self) -> bool {
        self.style.decode_to_bounds && matches!(self.source, ImageSource::Resource(_))
    }
}

fn frame_duration(delay: image::Delay, config: crate::AnimatedImageConfig) -> Duration {
    let duration = Duration::from(delay);
    let minimum = config.minimum_frame_duration();
    if duration.is_zero() {
        minimum
    } else {
        duration.max(minimum)
    }
}

fn frame_advance_budget(config: crate::AnimatedImageConfig) -> usize {
    config.decode_ahead_frames.clamp(1, 4)
}

fn next_animation_frame(
    data: &RenderImage,
    current_sequence: usize,
    executor: &BackgroundExecutor,
) -> Option<AnimatedFrame> {
    if data.frame_count() == usize::MAX {
        data.next_streaming_frame(current_sequence, executor)
    } else {
        let frame_count = data.frame_count();
        if frame_count == 0 {
            return None;
        }
        data.frame((current_sequence + 1) % frame_count)
    }
}

fn select_animation_frame(
    state: &mut ImgState,
    data: &RenderImage,
    animation_config: crate::AnimatedImageConfig,
    executor: &BackgroundExecutor,
) -> Option<AnimatedFrame> {
    let animation_config = animation_config.clamped();
    let current_time = executor.now();
    let mut current_frame = state.current_frame.clone().or_else(|| data.frame(0))?;

    if !data.is_animated() || !animation_config.play {
        let first_frame = data.frame(0)?;
        state.current_frame = Some(first_frame.clone());
        state.next_frame_at = None;
        return Some(first_frame);
    }

    let mut next_frame_at = state
        .next_frame_at
        .unwrap_or_else(|| current_time + frame_duration(current_frame.delay(), animation_config));

    if current_time < next_frame_at {
        state.current_frame = Some(current_frame.clone());
        state.next_frame_at = Some(next_frame_at);
        return Some(current_frame);
    }

    let mut advanced_frame = false;
    for _ in 0..frame_advance_budget(animation_config) {
        if current_time < next_frame_at {
            break;
        }

        let Some(next_frame) = next_animation_frame(data, current_frame.sequence(), executor)
        else {
            next_frame_at = current_time + animation_config.minimum_frame_duration();
            break;
        };
        next_frame_at += frame_duration(next_frame.delay(), animation_config);
        current_frame = next_frame;
        advanced_frame = true;
    }

    if advanced_frame && current_time >= next_frame_at {
        next_frame_at = current_time + frame_duration(current_frame.delay(), animation_config);
    }

    state.next_frame_at = Some(next_frame_at);
    state.current_frame = Some(current_frame.clone());
    Some(current_frame)
}

fn should_request_image_animation_frame(
    data: &RenderImage,
    animation_config: crate::AnimatedImageConfig,
) -> bool {
    data.is_animated() && animation_config.play
}

impl Deref for Stateful<Img> {
    type Target = Img;

    fn deref(&self) -> &Self::Target {
        &self.element
    }
}

impl DerefMut for Stateful<Img> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.element
    }
}

/// The image state between frames
struct ImgState {
    current_image: Option<Arc<RenderImage>>,
    current_frame: Option<AnimatedFrame>,
    next_frame_at: Option<Instant>,
    started_loading: Option<(Instant, Task<()>)>,
    target_size_asset: Option<TargetSizeImageSource>,
    pending_target_drop: Option<TargetSizeImageSource>,
}

impl ImgState {
    fn new(current_frame: Option<AnimatedFrame>) -> Self {
        Self {
            current_image: None,
            current_frame,
            next_frame_at: None,
            started_loading: None,
            target_size_asset: None,
            pending_target_drop: None,
        }
    }
}

/// The image layout state between frames
pub struct ImgLayoutState {
    frame: Option<AnimatedFrame>,
    replacement: Option<AnyElement>,
}

/// Resource image source plus the device-pixel decode target for bounds-aware loading.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TargetSizeImageSource {
    resource: Resource,
    target: ImageDecodeTarget,
    scale_factor_bits: u32,
    object_fit: ObjectFit,
    diagnostic_label: SharedString,
}

/// Resource image source used to cache compressed bytes before size-specific decoding.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CompressedImageSource {
    resource: Resource,
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
    type Output = Result<Arc<[u8]>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        let asset_source = cx.asset_source().clone();
        async move {
            let bytes = load_image_resource_bytes(source.resource, client, asset_source).await?;
            Ok(Arc::<[u8]>::from(bytes))
        }
    }
}

fn target_size_for_bounds(bounds: Bounds<Pixels>, window: &Window) -> Option<ImageDecodeTarget> {
    let size = bounds.size.to_device_pixels(window.scale_factor());
    let width = u32::try_from(size.width.0.max(0)).ok()?;
    let height = u32::try_from(size.height.0.max(0)).ok()?;
    let overscan = decode_overscan_factor(width, height);
    ImageDecodeTarget::new(
        bucket_decode_dimension(((width as f32) * overscan).ceil() as u32),
        bucket_decode_dimension(((height as f32) * overscan).ceil() as u32),
    )
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

fn use_target_size_data(
    source: ImageSource,
    object_fit: ObjectFit,
    animation_policy: ImageAnimationPolicy,
    usage: ImageUsageKind,
    usage_scope: Option<GpuiImageUsageScope>,
    bounds: Bounds<Pixels>,
    layout_state: &mut ImgLayoutState,
    global_id: Option<&GlobalElementId>,
    window: &mut Window,
    cx: &mut App,
) -> Option<(Arc<RenderImage>, AnimatedFrame)> {
    let ImageSource::Resource(resource) = source else {
        return None;
    };
    let target = target_size_for_bounds(bounds, window)?;
    let requested = TargetSizeImageSource {
        diagnostic_label: resource_diagnostic_label(&resource),
        resource,
        target,
        scale_factor_bits: window.scale_factor().to_bits(),
        object_fit,
    };
    track_resource_image_usage(&requested.resource, usage, usage_scope.as_ref(), cx);
    track_target_size_image_usage(&requested, usage, usage_scope.as_ref(), cx);
    let requested_hash = hash(&requested);

    let animation_config = animation_policy
        .apply_to(cx.image_pipeline_config().animated)
        .clamped();
    if let Some(global_id) = global_id {
        return window.with_element_state(global_id, |state: Option<ImgState>, window| {
            let mut state = state.unwrap_or_else(|| ImgState::new(layout_state.frame.clone()));

            if state.target_size_asset.as_ref() != Some(&requested) {
                if let Some(previous) = state.target_size_asset.replace(requested.clone())
                    && previous != requested
                {
                    if let Some(stale) = state.pending_target_drop.replace(previous) {
                        drop_stale_target_size_asset(&stale, window, cx);
                    }
                }
                state.next_frame_at = None;
            }

            let result = window.use_asset::<TargetSizeImgResourceLoader>(&requested, cx);
            let loaded = match result {
                Some(Ok(data)) => {
                    cx.touch_gpui_target_image_asset(
                        requested_hash,
                        data.decoded_byte_len(),
                        usage,
                        usage_scope.as_ref(),
                    );
                    let frame = if should_request_image_animation_frame(&data, animation_config) {
                        let frame = select_animation_frame(
                            &mut state,
                            &data,
                            animation_config,
                            cx.background_executor(),
                        );
                        window.request_animation_frame_for_image(cx, animation_config);
                        frame
                    } else {
                        data.frame(0)
                    };
                    if let Some(frame) = frame {
                        state.current_image = Some(data.clone());
                        state.current_frame = Some(frame.clone());
                        if let Some(stale) = state.pending_target_drop.take() {
                            drop_stale_target_size_asset(&stale, window, cx);
                        }
                        Some((data, frame))
                    } else {
                        None
                    }
                }
                Some(Err(_)) | None => state.current_image.clone().zip(state.current_frame.clone()),
            };

            (loaded, state)
        });
    }

    let data = window
        .use_asset::<TargetSizeImgResourceLoader>(&requested, cx)?
        .ok()?;
    cx.touch_gpui_target_image_asset(
        requested_hash,
        data.decoded_byte_len(),
        usage,
        usage_scope.as_ref(),
    );
    let frame = data.frame(0)?;
    Some((data, frame))
}

fn bucket_decode_dimension(value: u32) -> u32 {
    const BUCKET: u32 = 16;
    value.max(1).div_ceil(BUCKET) * BUCKET
}

fn drop_stale_target_size_asset(
    previous: &TargetSizeImageSource,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(task) = cx.take_asset::<TargetSizeImgResourceLoader>(previous)
        && let Some(Ok(image)) = task.now_or_never()
    {
        cx.drop_image(image, Some(window));
        let asset_hash = hash(previous);
        drop_image_asset_retained(asset_hash);
        cx.forget_gpui_target_image_asset(asset_hash);
    }
}

fn track_resource_image_usage(
    resource: &Resource,
    usage: ImageUsageKind,
    scope: Option<&GpuiImageUsageScope>,
    cx: &mut App,
) {
    track_image_asset_usage::<ImgResourceLoader, _>(resource, usage, scope, cx);
}

fn track_target_size_image_usage(
    source: &TargetSizeImageSource,
    usage: ImageUsageKind,
    scope: Option<&GpuiImageUsageScope>,
    cx: &mut App,
) {
    track_image_asset_usage::<TargetSizeImgResourceLoader, _>(source, usage, scope, cx);
    track_image_asset_usage::<CompressedImgResourceLoader, _>(
        &CompressedImageSource {
            resource: source.resource.clone(),
        },
        usage,
        scope,
        cx,
    );
}

fn track_image_asset_usage<A, S>(
    source: &S,
    usage: ImageUsageKind,
    scope: Option<&GpuiImageUsageScope>,
    cx: &mut App,
) where
    A: Asset<Source = S>,
    S: Hash,
{
    cx.track_gpui_image_asset_usage(TypeId::of::<A>(), hash(source), usage, scope);
}

impl Element for Img {
    type RequestLayoutState = ImgLayoutState;
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        self.interactivity
            .element_id
            .clone()
            .or_else(|| Some(self.fallback_element_id()))
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        self.interactivity.source_location()
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut layout_state = ImgLayoutState {
            frame: None,
            replacement: None,
        };
        let decode_to_bounds = self.should_decode_to_bounds();

        window.with_optional_element_state(global_id, |state, window| {
            let mut state = state.map(|state| state.unwrap_or_else(|| ImgState::new(None)));

            let layout_id = self.interactivity.request_layout(
                global_id,
                inspector_id,
                window,
                cx,
                |mut style, window, cx| {
                    let mut replacement_id = None;

                    if decode_to_bounds {
                        return window.request_layout(style, replacement_id, cx);
                    }

                    match self.source.use_data(
                        self.image_cache
                            .clone()
                            .or_else(|| window.image_cache_stack.last().cloned()),
                        self.usage,
                        self.usage_scope.as_ref(),
                        window,
                        cx,
                    ) {
                        Some(Ok(data)) => {
                            let animation_config = self
                                .animation_policy
                                .apply_to(cx.image_pipeline_config().animated)
                                .clamped();
                            let mut frame = data.frame(0);

                            if let Some(state) = &mut state {
                                if !data.is_animated() || !animation_config.play {
                                    state.current_frame = frame.clone();
                                    state.next_frame_at = None;
                                } else {
                                    frame = state.current_frame.clone().or(frame);
                                }
                                state.started_loading = None;
                            }

                            let Some(frame) = frame else {
                                return window.request_layout(style, replacement_id, cx);
                            };

                            let image_size: crate::Size<Pixels> = frame
                                .size()
                                .map(|v| (v.0 as f32 / data.scale_factor).into());
                            style.aspect_ratio = Some(image_size.width / image_size.height);

                            if let Length::Auto = style.size.width {
                                style.size.width = match style.size.height {
                                    Length::Definite(DefiniteLength::Absolute(abs_length)) => {
                                        let height_px = abs_length.to_pixels(window.rem_size());
                                        Length::Definite(
                                            px(image_size.width.0 * height_px.0
                                                / image_size.height.0)
                                            .into(),
                                        )
                                    }
                                    _ => Length::Definite(image_size.width.into()),
                                };
                            }

                            if let Length::Auto = style.size.height {
                                style.size.height = match style.size.width {
                                    Length::Definite(DefiniteLength::Absolute(abs_length)) => {
                                        let width_px = abs_length.to_pixels(window.rem_size());
                                        Length::Definite(
                                            px(image_size.height.0 * width_px.0
                                                / image_size.width.0)
                                            .into(),
                                        )
                                    }
                                    _ => Length::Definite(image_size.height.into()),
                                };
                            }

                            layout_state.frame = Some(frame);
                        }
                        Some(_err) => {
                            if let Some(fallback) = self.style.fallback.as_ref() {
                                let mut element = fallback();
                                replacement_id = Some(element.request_layout(window, cx));
                                layout_state.replacement = Some(element);
                            }
                            if let Some(state) = &mut state {
                                state.started_loading = None;
                            }
                        }
                        None => {
                            if let Some(state) = &mut state {
                                if let Some((started_loading, _)) = state.started_loading {
                                    if started_loading.elapsed() > LOADING_DELAY
                                        && let Some(loading) = self.style.loading.as_ref()
                                    {
                                        let mut element = loading();
                                        replacement_id = Some(element.request_layout(window, cx));
                                        layout_state.replacement = Some(element);
                                    }
                                } else {
                                    let current_view = window.current_view();
                                    let task = window.spawn(cx, async move |cx| {
                                        cx.background_executor().timer(LOADING_DELAY).await;
                                        cx.update(move |_, cx| {
                                            cx.notify(current_view);
                                        })
                                        .ok();
                                    });
                                    state.started_loading = Some((Instant::now(), task));
                                }
                            }
                        }
                    }

                    window.request_layout(style, replacement_id, cx)
                },
            );

            ((layout_id, layout_state), state)
        })
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.interactivity.prepaint(
            global_id,
            inspector_id,
            bounds,
            bounds.size,
            window,
            cx,
            |_, _, hitbox, window, cx| {
                if let Some(replacement) = &mut request_layout.replacement {
                    if window.draw_budget_exhausted_for_optional_work() {
                        window.degrade_current_draw();
                        return hitbox;
                    }
                    replacement.prepaint(window, cx);
                }

                hitbox
            },
        )
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout_state: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let source = self.source.clone();
        let decode_to_bounds = self.should_decode_to_bounds();
        let object_fit = self.style.object_fit;
        let grayscale = self.style.grayscale;
        let animation_policy = self.animation_policy;
        self.interactivity.paint(
            global_id,
            inspector_id,
            bounds,
            hitbox.as_ref(),
            window,
            cx,
            |style, window, cx| {
                if window.draw_budget_exhausted_for_optional_work() {
                    window.degrade_current_draw();
                    return;
                }
                if decode_to_bounds {
                    let Some((data, frame)) = use_target_size_data(
                        source,
                        object_fit,
                        animation_policy,
                        self.usage,
                        self.usage_scope.clone(),
                        bounds,
                        layout_state,
                        global_id,
                        window,
                        cx,
                    ) else {
                        if let Some(replacement) = &mut layout_state.replacement {
                            replacement.paint(window, cx);
                        }
                        return;
                    };

                    let new_bounds = object_fit.get_bounds(bounds, frame.size());
                    let corner_radii = style
                        .corner_radii
                        .to_pixels(window.rem_size())
                        .clamp_radii_for_quad_size(new_bounds.size);
                    if window.draw_budget_exhausted_for_optional_work() {
                        window.degrade_current_draw();
                        return;
                    }
                    window
                        .paint_image_frame(new_bounds, corner_radii, data, frame, grayscale)
                        .log_err();
                    return;
                }

                if let Some(Ok(data)) = source.use_data(
                    self.image_cache
                        .clone()
                        .or_else(|| window.image_cache_stack.last().cloned()),
                    self.usage,
                    self.usage_scope.as_ref(),
                    window,
                    cx,
                ) {
                    let animation_config = animation_policy
                        .apply_to(cx.image_pipeline_config().animated)
                        .clamped();
                    let frame = if should_request_image_animation_frame(&data, animation_config)
                        && let Some(global_id) = global_id
                    {
                        window.with_element_state(global_id, |state: Option<ImgState>, window| {
                            let mut state =
                                state.unwrap_or_else(|| ImgState::new(layout_state.frame.clone()));
                            let frame = select_animation_frame(
                                &mut state,
                                &data,
                                animation_config,
                                cx.background_executor(),
                            );
                            window.request_animation_frame_for_image(cx, animation_config);
                            (frame, state)
                        })
                    } else {
                        layout_state.frame.clone()
                    };

                    let Some(frame) = frame else {
                        return;
                    };

                    let new_bounds = object_fit.get_bounds(bounds, frame.size());
                    let corner_radii = style
                        .corner_radii
                        .to_pixels(window.rem_size())
                        .clamp_radii_for_quad_size(new_bounds.size);
                    if window.draw_budget_exhausted_for_optional_work() {
                        window.degrade_current_draw();
                        return;
                    }
                    window
                        .paint_image_frame(new_bounds, corner_radii, data, frame, grayscale)
                        .log_err();
                } else if let Some(replacement) = &mut layout_state.replacement {
                    if window.draw_budget_exhausted_for_optional_work() {
                        window.degrade_current_draw();
                        return;
                    }
                    replacement.paint(window, cx);
                }
            },
        )
    }
}

impl Styled for Img {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.interactivity.base_style
    }
}

impl InteractiveElement for Img {
    fn interactivity(&mut self) -> &mut Interactivity {
        &mut self.interactivity
    }
}

impl IntoElement for Img {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl StatefulInteractiveElement for Img {}

impl ImageSource {
    pub(crate) fn use_data(
        &self,
        cache: Option<AnyImageCache>,
        usage: ImageUsageKind,
        usage_scope: Option<&GpuiImageUsageScope>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        match self {
            ImageSource::Resource(resource) => {
                track_resource_image_usage(resource, usage, usage_scope, cx);
                if let Some(cache) = cache {
                    cache.load(resource, window, cx)
                } else {
                    cx.default_image_cache().load(resource, window, cx)
                }
            }
            ImageSource::Custom(loading_fn) => loading_fn(window, cx),
            ImageSource::Render(data) => Some(Ok(data.to_owned())),
            ImageSource::Image(data) => {
                track_image_asset_usage::<AssetLogger<ImageDecoder>, _>(
                    data,
                    usage,
                    usage_scope,
                    cx,
                );
                window.use_asset::<AssetLogger<ImageDecoder>>(data, cx)
            }
            ImageSource::Bytes(data) => {
                track_image_asset_usage::<AssetLogger<EncodedImageDecoder>, _>(
                    data,
                    usage,
                    usage_scope,
                    cx,
                );
                window.use_asset::<AssetLogger<EncodedImageDecoder>>(data, cx)
            }
        }
    }

    pub(crate) fn get_data(
        &self,
        cache: Option<AnyImageCache>,
        usage: ImageUsageKind,
        usage_scope: Option<&GpuiImageUsageScope>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        match self {
            ImageSource::Resource(resource) => {
                track_resource_image_usage(resource, usage, usage_scope, cx);
                if let Some(cache) = cache {
                    cache.load(resource, window, cx)
                } else {
                    cx.default_image_cache().load(resource, window, cx)
                }
            }
            ImageSource::Custom(loading_fn) => loading_fn(window, cx),
            ImageSource::Render(data) => Some(Ok(data.to_owned())),
            ImageSource::Image(data) => {
                track_image_asset_usage::<AssetLogger<ImageDecoder>, _>(
                    data,
                    usage,
                    usage_scope,
                    cx,
                );
                window.get_asset::<AssetLogger<ImageDecoder>>(data, cx)
            }
            ImageSource::Bytes(data) => {
                track_image_asset_usage::<AssetLogger<EncodedImageDecoder>, _>(
                    data,
                    usage,
                    usage_scope,
                    cx,
                );
                window.get_asset::<AssetLogger<EncodedImageDecoder>>(data, cx)
            }
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
    /// Creates an in-memory encoded image source.
    pub fn new(format: crate::ImageFormat, bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            format,
            bytes: bytes.into(),
        }
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
            source
                .to_image_data_with_config(renderer, config, Some(executor))
                .map_err(Into::into)
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
            let image = Image::from_bytes(source.format, source.bytes.to_vec());
            image
                .to_image_data_with_config(renderer, config, Some(executor))
                .map_err(Into::into)
        }
    }
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
            let bytes = match source.clone() {
                Resource::Path(uri) => fs::read(uri.as_ref())?,
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
                    body
                }
                Resource::Embedded(path) => {
                    let data = asset_source.load(&path).ok().flatten();
                    if let Some(data) = data {
                        data.to_vec()
                    } else {
                        return Err(ImageCacheError::Asset(
                            format!("Embedded resource not found: {}", path).into(),
                        ));
                    }
                }
            };

            let decode_started = Instant::now();
            let mut data = if let Ok(format) = image::guess_format(&bytes) {
                decode_image_bytes(
                    &bytes,
                    format,
                    image_config,
                    Some(background_executor.clone()),
                )?
            } else {
                let pixmap =
                    // TODO: Can we make svgs always rescale?
                    svg_renderer.render_pixmap(&bytes, SvgSize::ScaleFactor(SMOOTH_SVG_SCALE_FACTOR))?;

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
            data = data.with_pipeline_metadata(bytes.len(), decode_duration);
            record_image_decode_metrics_with_threshold(
                bytes.len(),
                data.decoded_byte_len(),
                data.frame_count(),
                decode_duration,
                slow_decode_threshold,
            );
            if decode_duration >= slow_decode_threshold {
                log::debug!(
                    "slow image decode: source={source:?} compressed_bytes={} decoded_bytes={} frames={} decode_ms={:.3}",
                    bytes.len(),
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
        let (compressed_task, _) =
            cx.fetch_asset::<CompressedImgResourceLoader>(&CompressedImageSource {
                resource: source.resource.clone(),
            });
        async move {
            let compressed_bytes = compressed_task.await?;
            let compressed_len = compressed_bytes.len();
            let decode_started = Instant::now();
            let scale_factor = f32::from_bits(source.scale_factor_bits);
            let (mut data, metadata) = if let Ok(format) = image::guess_format(&compressed_bytes) {
                decode_image_bytes_to_target(
                    &compressed_bytes,
                    format,
                    image_config,
                    source.target,
                    source.object_fit,
                )?
            } else {
                let natural_size = svg_renderer.natural_size(&compressed_bytes)?;
                let fitted_target = fitted_target_size(
                    natural_size.map(|dimension| u32::from(dimension)),
                    source.target,
                    source.object_fit,
                );
                let pixmap = svg_renderer
                    .render_pixmap(&compressed_bytes, SvgSize::Size(fitted_target.size()))?;
                let mut buffer =
                    ImageBuffer::from_raw(pixmap.width(), pixmap.height(), pixmap.take())
                        .ok_or_else(|| anyhow::anyhow!("invalid SVG raster dimensions"))?;

                for pixel in buffer.chunks_exact_mut(4) {
                    swap_rgba_pa_to_bgra(pixel);
                }

                (
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
                )
            };

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

async fn load_image_resource_bytes(
    resource: Resource,
    client: Arc<dyn http_client::HttpClient>,
    asset_source: Arc<dyn crate::AssetSource>,
) -> Result<Vec<u8>, ImageCacheError> {
    Ok(match resource {
        Resource::Path(uri) => fs::read(uri.as_ref())?,
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
            body
        }
        Resource::Embedded(path) => {
            let data = asset_source.load(&path).ok().flatten();
            if let Some(data) = data {
                data.to_vec()
            } else {
                return Err(ImageCacheError::Asset(
                    format!("Embedded resource not found: {path}").into(),
                ));
            }
        }
    })
}

/// An error that can occur when interacting with the image cache.
#[derive(Debug, Error, Clone)]
pub enum ImageCacheError {
    /// Some other kind of error occurred
    #[error("error: {0}")]
    Other(#[from] Arc<anyhow::Error>),
    /// An error that occurred while reading the image from disk.
    #[error("IO error: {0}")]
    Io(Arc<std::io::Error>),
    /// An error that occurred while processing an image.
    #[error("unexpected http status for {uri}: {status}, body: {body}")]
    BadStatus {
        /// The URI of the image.
        uri: SharedUri,
        /// The HTTP status code.
        status: http_client::StatusCode,
        /// The HTTP response body.
        body: String,
    },
    /// An error that occurred while processing an asset.
    #[error("asset error: {0}")]
    Asset(SharedString),
    /// An error that occurred while processing an image.
    #[error("image error: {0}")]
    Image(Arc<ImageError>),
    /// An error that occurred while processing an SVG.
    #[error("svg error: {0}")]
    Usvg(Arc<usvg::Error>),
}

impl From<anyhow::Error> for ImageCacheError {
    fn from(value: anyhow::Error) -> Self {
        Self::Other(Arc::new(value))
    }
}

impl From<io::Error> for ImageCacheError {
    fn from(value: io::Error) -> Self {
        Self::Io(Arc::new(value))
    }
}

impl From<usvg::Error> for ImageCacheError {
    fn from(value: usvg::Error) -> Self {
        Self::Usvg(Arc::new(value))
    }
}

impl From<image::ImageError> for ImageCacheError {
    fn from(value: image::ImageError) -> Self {
        Self::Image(Arc::new(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Delay, Frame, ImageBuffer};
    use rand::SeedableRng as _;
    use std::{sync::Arc, time::Duration};

    #[test]
    fn img_has_stable_fallback_element_id() {
        let image = img("examples/image/black-cat-typing.gif");
        assert_eq!(Element::id(&image), Element::id(&image));
    }

    #[test]
    fn explicit_img_element_id_is_preserved() {
        let image = img("examples/image/black-cat-typing.gif").id("explicit");
        assert_eq!(
            Element::id(&image),
            Some(ElementId::Name("explicit".into()))
        );
    }

    #[test]
    fn select_animation_frame_advances_resident_frames() {
        let frame = |color| {
            Frame::from_parts(
                ImageBuffer::from_pixel(1, 1, image::Rgba(color)),
                0,
                0,
                Delay::from_saturating_duration(Duration::from_millis(1)),
            )
        };
        let image = RenderImage::new(vec![frame([255, 0, 0, 255]), frame([0, 255, 0, 255])]);
        let executor = BackgroundExecutor::new(Arc::new(crate::TestDispatcher::new(
            rand::rngs::StdRng::seed_from_u64(1),
        )));
        let now = executor.now();
        let mut state = ImgState::new(Some(image.frame(0).unwrap()));
        state.next_frame_at = Some(now - Duration::from_millis(1));

        let next_frame = select_animation_frame(
            &mut state,
            &image,
            crate::AnimatedImageConfig {
                max_fps: 240.0,
                ..crate::AnimatedImageConfig::default()
            },
            &executor,
        )
        .unwrap();

        assert_eq!(next_frame.sequence(), 1);
    }

    #[test]
    fn select_animation_frame_catches_up_ready_resident_frames() {
        let frame = |color| {
            Frame::from_parts(
                ImageBuffer::from_pixel(1, 1, image::Rgba(color)),
                0,
                0,
                Delay::from_saturating_duration(Duration::from_millis(1)),
            )
        };
        let image = RenderImage::new(vec![
            frame([255, 0, 0, 255]),
            frame([0, 255, 0, 255]),
            frame([0, 0, 255, 255]),
            frame([255, 255, 0, 255]),
            frame([255, 0, 255, 255]),
        ]);
        let executor = BackgroundExecutor::new(Arc::new(crate::TestDispatcher::new(
            rand::rngs::StdRng::seed_from_u64(2),
        )));
        let now = executor.now();
        let mut state = ImgState::new(Some(image.frame(0).unwrap()));
        state.next_frame_at = Some(now - Duration::from_millis(80));

        let next_frame = select_animation_frame(
            &mut state,
            &image,
            crate::AnimatedImageConfig {
                max_fps: 240.0,
                ..crate::AnimatedImageConfig::default()
            },
            &executor,
        )
        .unwrap();

        assert_eq!(next_frame.sequence(), 4);
    }

    #[test]
    fn animated_img_requests_frames_when_policy_plays() {
        let image = RenderImage::new(vec![
            Frame::new(ImageBuffer::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]))),
            Frame::new(ImageBuffer::from_pixel(1, 1, image::Rgba([0, 255, 0, 255]))),
        ]);
        let policy = ImageAnimationPolicy::playing(12.0);
        let config = policy
            .apply_to(crate::AnimatedImageConfig::default())
            .clamped();

        assert!(should_request_image_animation_frame(&image, config));
    }

    #[test]
    fn animated_img_does_not_request_frames_when_policy_pauses() {
        let image = RenderImage::new(vec![
            Frame::new(ImageBuffer::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]))),
            Frame::new(ImageBuffer::from_pixel(1, 1, image::Rgba([0, 255, 0, 255]))),
        ]);
        let policy = ImageAnimationPolicy::paused();
        let config = policy
            .apply_to(crate::AnimatedImageConfig::default())
            .clamped();

        assert!(!should_request_image_animation_frame(&image, config));
    }

    #[test]
    fn static_img_does_not_request_animation_frames() {
        let image = RenderImage::new(vec![Frame::new(ImageBuffer::from_pixel(
            1,
            1,
            image::Rgba([255, 0, 0, 255]),
        ))]);
        let policy = ImageAnimationPolicy::playing(12.0);
        let config = policy
            .apply_to(crate::AnimatedImageConfig::default())
            .clamped();

        assert!(!should_request_image_animation_frame(&image, config));
    }

    #[test]
    fn decode_bounds_dimension_is_bucketed() {
        assert_eq!(bucket_decode_dimension(1), 16);
        assert_eq!(bucket_decode_dimension(38), 48);
        assert_eq!(bucket_decode_dimension(800), 800);
    }
}
