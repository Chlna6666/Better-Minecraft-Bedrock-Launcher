use super::element::SizedImageLoader;
use super::loader::{ImageRenderRequest, image_size_for_window};
use super::playback::{
    request_next_image_animation_frame, select_animation_frame,
    should_request_image_animation_frame,
};
use super::source::ImageSource;
use super::style::ImageAnimationPolicy;
use super::{layout::ImageLayout, retained::ImageElementState};
use crate::{
    AnimatedFrame, App, Bounds, GlobalElementId, ImageBoundsPolicy, ObjectFit, Pixels, RenderImage,
    Window, drop_image_asset_retained, hash,
};
use futures::FutureExt;
use std::sync::Arc;

pub(super) fn render_sized_image(
    source: ImageSource,
    object_fit: ObjectFit,
    animation_policy: ImageAnimationPolicy,
    bounds: Bounds<Pixels>,
    layout_state: &mut ImageLayout,
    global_id: Option<&GlobalElementId>,
    window: &mut Window,
    cx: &mut App,
) -> Option<(Arc<RenderImage>, AnimatedFrame)> {
    let ImageSource::Asset(resource) = source else {
        return None;
    };
    if cx.image_pipeline_config().bounds_policy == ImageBoundsPolicy::Visible
        && bounds
            .intersect(&window.visual_content_mask().bounds)
            .is_empty()
    {
        return None;
    }
    let target = image_size_for_window(bounds, window)?;
    let requested = ImageRenderRequest::new(resource, target, window.scale_factor(), object_fit);

    let animation_config = animation_policy
        .apply_to(cx.image_pipeline_config().animated)
        .clamped();
    if let Some(global_id) = global_id {
        return window.with_element_state(global_id, |state: Option<ImageElementState>, window| {
            let mut state = state.unwrap_or(ImageElementState {
                current_image: None,
                current_frame: layout_state.frame.clone(),
                next_frame_at: None,
                started_loading: None,
                sized_image_request: None,
                pending_sized_image_drop: None,
            });

            if state.sized_image_request.as_ref() != Some(&requested) {
                if let Some(previous) = state.sized_image_request.replace(requested.clone())
                    && previous != requested
                {
                    if let Some(stale) = state.pending_sized_image_drop.replace(previous) {
                        drop_stale_sized_image(&stale, window, cx);
                    }
                }
                state.next_frame_at = None;
            }

            let result = window.use_asset::<SizedImageLoader>(&requested, cx);
            let loaded = match result {
                Some(Ok(render_image)) => {
                    let frame =
                        if should_request_image_animation_frame(&render_image, animation_config) {
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
                            frame
                        } else {
                            render_image.frame(0)
                        };
                    if let Some(frame) = frame {
                        state.current_image = Some(render_image.clone());
                        state.current_frame = Some(frame.clone());
                        if let Some(stale) = state.pending_sized_image_drop.take() {
                            drop_stale_sized_image(&stale, window, cx);
                        }
                        Some((render_image, frame))
                    } else {
                        None
                    }
                }
                Some(Err(_)) | None => state.current_image.clone().zip(state.current_frame.clone()),
            };

            (loaded, state)
        });
    }

    let render_image = window.use_asset::<SizedImageLoader>(&requested, cx)?.ok()?;
    let frame = render_image.frame(0)?;
    Some((render_image, frame))
}

fn drop_stale_sized_image(previous: &ImageRenderRequest, window: &mut Window, cx: &mut App) {
    if let Some(task) = cx.take_asset::<SizedImageLoader>(previous)
        && let Some(Ok(image)) = task.now_or_never()
    {
        cx.drop_image(image, Some(window));
        drop_image_asset_retained(hash(previous));
    }
}
