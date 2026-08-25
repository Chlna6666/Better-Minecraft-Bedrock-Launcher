use crate::{
    AnyImageCache, App, AssetLogger, Bounds, DefiniteLength, Element, ElementId, Entity,
    GlobalElementId, Hitbox, ImageBoundsPolicy, ImageCache, InspectorElementId, InteractiveElement,
    Interactivity, IntoElement, LayoutId, Length, ObjectFit, Pixels, RenderImage, StyleRefinement,
    Styled, Task, Window, px,
};
use anyhow::Result;

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    ops::{Deref, DerefMut},
    sync::Arc,
    time::{Duration, Instant},
};
use util::ResultExt;

use super::super::{Stateful, StatefulInteractiveElement};
use super::error::ImageCacheError;
use super::loader::*;
use super::playback::*;
use super::sizing::*;
use super::source::*;
use super::style::*;
use super::{layout::ImageLayout, retained::ImageElementState};

/// The delay before showing the loading state.
pub const LOADING_DELAY: Duration = Duration::from_millis(200);

/// A type alias to the resource loader that the `img()` element uses.
///
/// Note: that this is only for Resources, like URLs or file paths.
/// Custom loaders, or external images will not use this asset loader
pub type ResourceImageLoader = AssetLogger<ImageAssetLoader>;

/// AssetLocation loader used when an image must be decoded for a concrete paint-bounds size.
pub type SizedImageLoader = AssetLogger<SizedImageAssetLoader>;

pub(crate) type CompressedImageLoader = AssetLogger<CompressedImageAssetLoader>;

/// A handle to compressed image bytes retained in GPUI's global asset cache.
///
/// This can be used by applications to prefetch file, embedded, or network image bytes before an
/// [`img()`] element knows its final paint bounds. Bounds-aware image elements reuse this task and
/// only perform the final target-size decode once layout has produced concrete dimensions.
pub type CompressedImageTask =
    futures::future::Shared<Task<Result<CompressedImageBytes, ImageCacheError>>>;

/// A handle to a target-size image processing retained in GPUI's global asset cache.
///
/// Applications can use this to prewarm images whose target size is known before the element tree
/// reaches paint. The same cache entry is reused by [`StyledImage::render_to_bounds`].
pub type SizedImageTask = futures::future::Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>;

/// An image element.
pub struct Img {
    interactivity: Interactivity,
    source: ImageSource,
    pub(super) style: ImageStyle,
    image_cache: Option<AnyImageCache>,
    animation_policy: ImageAnimationPolicy,
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

    fn fallback_element_id(&self) -> ElementId {
        let mut hasher = DefaultHasher::new();
        if let Some(location) = self.interactivity.source_location() {
            location.file().hash(&mut hasher);
            location.line().hash(&mut hasher);
            location.column().hash(&mut hasher);
        }

        match &self.source {
            ImageSource::Asset(resource) => {
                0u8.hash(&mut hasher);
                resource.hash(&mut hasher);
            }
            ImageSource::RenderImage(image) => {
                1u8.hash(&mut hasher);
                image.id.hash(&mut hasher);
            }
            ImageSource::Clipboard(image) => {
                2u8.hash(&mut hasher);
                image.id().hash(&mut hasher);
            }
            ImageSource::Encoded(bytes) => {
                3u8.hash(&mut hasher);
                // Hash the buffer identity instead of its contents; this method runs every
                // frame and the encoded bytes can be large.
                bytes.hash_identity(&mut hasher);
            }
            ImageSource::Loader(load) => {
                4u8.hash(&mut hasher);
                Arc::as_ptr(load).hash(&mut hasher);
            }
        }

        ElementId::NamedInteger("img".into(), hasher.finish())
    }

    fn should_render_to_bounds(&self, bounds_policy: ImageBoundsPolicy) -> bool {
        matches!(self.source, ImageSource::Asset(_))
            && (self.style.render_to_bounds || bounds_policy == ImageBoundsPolicy::Visible)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_policy_enables_bounds_aware_resource_render() {
        let image = img("images/test.png");

        assert!(image.should_render_to_bounds(ImageBoundsPolicy::Visible));
        assert!(!image.should_render_to_bounds(ImageBoundsPolicy::Explicit));
    }

    #[test]
    fn explicit_bounds_render_remains_available() {
        let image = img("images/test.png").render_to_bounds();

        assert!(image.should_render_to_bounds(ImageBoundsPolicy::Explicit));
    }
}

impl Element for Img {
    type RequestLayoutState = ImageLayout;
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
        let mut layout_state = ImageLayout {
            frame: None,
            replacement: None,
        };
        let render_to_bounds =
            self.should_render_to_bounds(cx.image_pipeline_config().bounds_policy);

        // Bounds-aware images own a distinct retained-state type during paint. Do not touch the
        // ordinary image state here: switching rendering modes then makes the previous state stale
        // and lets the frame lifecycle retire it without branch-specific cleanup.
        if render_to_bounds {
            let layout_id = self.interactivity.request_layout(
                global_id,
                inspector_id,
                window,
                cx,
                |style, window, cx| window.request_layout(style, None, cx),
            );
            return (layout_id, layout_state);
        }

        window.with_optional_element_state(global_id, |state, window| {
            let mut state = state.map(|state| {
                state.unwrap_or(ImageElementState {
                    current_image: None,
                    current_frame: None,
                    next_frame_at: None,
                    started_loading: None,
                })
            });

            let layout_id = self.interactivity.request_layout(
                global_id,
                inspector_id,
                window,
                cx,
                |mut style, window, cx| {
                    let mut replacement_id = None;

                    match self.source.use_render_image(
                        self.image_cache
                            .clone()
                            .or_else(|| window.image_cache_stack.last().cloned()),
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
        let render_to_bounds =
            self.should_render_to_bounds(cx.image_pipeline_config().bounds_policy);
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
                if render_to_bounds {
                    let Some((render_image, frame)) = render_sized_image(
                        source,
                        object_fit,
                        animation_policy,
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

                    let new_bounds = object_fit.bounds(bounds, frame.size());
                    let corner_radii = style
                        .corner_radii
                        .to_pixels(window.rem_size())
                        .clamp_radii_for_quad_size(if object_fit == ObjectFit::Cover {
                            bounds.size
                        } else {
                            new_bounds.size
                        });
                    if object_fit == ObjectFit::Cover {
                        window
                            .paint_image_frame_clipped(
                                new_bounds,
                                bounds,
                                corner_radii,
                                render_image,
                                frame,
                                grayscale,
                            )
                            .log_err();
                    } else {
                        window
                            .paint_image_frame(
                                new_bounds,
                                corner_radii,
                                render_image,
                                frame,
                                grayscale,
                            )
                            .log_err();
                    }
                    return;
                }

                if let Some(Ok(render_image)) = source.use_render_image(
                    self.image_cache
                        .clone()
                        .or_else(|| window.image_cache_stack.last().cloned()),
                    window,
                    cx,
                ) {
                    let animation_config = animation_policy
                        .apply_to(cx.image_pipeline_config().animated)
                        .clamped();
                    let frame =
                        if should_request_image_animation_frame(&render_image, animation_config)
                            && let Some(global_id) = global_id
                        {
                            window.with_element_state(
                                global_id,
                                |state: Option<ImageElementState>, window| {
                                    let mut state = state.unwrap_or(ImageElementState {
                                        current_image: None,
                                        current_frame: layout_state.frame.clone(),
                                        next_frame_at: None,
                                        started_loading: None,
                                    });
                                    let frame = select_animation_frame(
                                        &mut state,
                                        &render_image,
                                        animation_config,
                                        cx.background_executor(),
                                    );
                                    request_next_image_animation_frame(
                                        &state,
                                        window,
                                        cx,
                                        animation_config,
                                    );
                                    (frame, state)
                                },
                            )
                        } else {
                            layout_state.frame.clone()
                        };

                    let Some(frame) = frame else {
                        return;
                    };

                    let new_bounds = object_fit.bounds(bounds, frame.size());
                    let corner_radii = style
                        .corner_radii
                        .to_pixels(window.rem_size())
                        .clamp_radii_for_quad_size(if object_fit == ObjectFit::Cover {
                            bounds.size
                        } else {
                            new_bounds.size
                        });
                    if object_fit == ObjectFit::Cover {
                        window
                            .paint_image_frame_clipped(
                                new_bounds,
                                bounds,
                                corner_radii,
                                render_image,
                                frame,
                                grayscale,
                            )
                            .log_err();
                    } else {
                        window
                            .paint_image_frame(
                                new_bounds,
                                corner_radii,
                                render_image,
                                frame,
                                grayscale,
                            )
                            .log_err();
                    }
                } else if let Some(replacement) = &mut layout_state.replacement {
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
