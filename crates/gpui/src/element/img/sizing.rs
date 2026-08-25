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

            // `sized_image_request` owns the image currently committed to the screen. The second
            // slot is the newest not-yet-committed request. Never evict the committed request just
            // because another resize arrived: it remains the visual fallback until the replacement
            // has produced a drawable frame.
            let current_matches_requested = state.sized_image_request.as_ref() == Some(&requested);
            if let Some(stale_pending) = update_pending_sized_request(
                current_matches_requested,
                &mut state.pending_sized_image_drop,
                &requested,
            ) {
                cancel_pending_sized_image(&stale_pending, window, cx);
            }

            if current_matches_requested {
                let loaded = render_current_sized_image(
                    &mut state,
                    animation_config,
                    window,
                    cx,
                );
                return (loaded, state);
            }

            let result = window.use_asset::<SizedImageLoader>(&requested, cx);
            let loaded = match result {
                Some(Ok(render_image)) => {
                    let previous_frame = state.current_frame.clone();
                    let previous_next_frame_at = state.next_frame_at;
                    state.current_frame = render_image.frame(0);
                    state.next_frame_at = None;

                    let frame = if should_request_image_animation_frame(
                        &render_image,
                        animation_config,
                    ) {
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
                        let previous_request = commit_sized_image_request(
                            &mut state.sized_image_request,
                            &mut state.pending_sized_image_drop,
                            &requested,
                        );
                        let previous_image = state.current_image.replace(render_image.clone());
                        state.current_frame = Some(frame.clone());
                        if !render_image.is_animated() || !animation_config.play {
                            state.next_frame_at = None;
                        }

                        if let Some(previous_request) = previous_request {
                            release_committed_sized_image(
                                &previous_request,
                                previous_image,
                                window,
                                cx,
                            );
                        }

                        Some((render_image, frame))
                    } else {
                        state.current_frame = previous_frame;
                        state.next_frame_at = previous_next_frame_at;
                        render_current_sized_image(
                            &mut state,
                            animation_config,
                            window,
                            cx,
                        )
                    }
                }
                Some(Err(_)) | None => render_current_sized_image(
                    &mut state,
                    animation_config,
                    window,
                    cx,
                ),
            };

            (loaded, state)
        });
    }

    let render_image = window.use_asset::<SizedImageLoader>(&requested, cx)?.ok()?;
    let frame = render_image.frame(0)?;
    Some((render_image, frame))
}

fn update_pending_sized_request(
    current_matches_requested: bool,
    pending: &mut Option<ImageRenderRequest>,
    requested: &ImageRenderRequest,
) -> Option<ImageRenderRequest> {
    if current_matches_requested {
        return pending
            .take()
            .filter(|pending_request| pending_request != requested);
    }

    if pending.as_ref() == Some(requested) {
        return None;
    }

    pending.replace(requested.clone())
}

fn commit_sized_image_request(
    current: &mut Option<ImageRenderRequest>,
    pending: &mut Option<ImageRenderRequest>,
    requested: &ImageRenderRequest,
) -> Option<ImageRenderRequest> {
    debug_assert!(pending.as_ref().is_none_or(|pending| pending == requested));
    pending.take();
    current
        .replace(requested.clone())
        .filter(|previous| previous != requested)
}

fn render_current_sized_image(
    state: &mut ImageElementState,
    animation_config: crate::AnimatedImageConfig,
    window: &mut Window,
    cx: &App,
) -> Option<(Arc<RenderImage>, AnimatedFrame)> {
    let render_image = state.current_image.clone()?;
    let frame = if should_request_image_animation_frame(&render_image, animation_config) {
        let frame = select_animation_frame(
            state,
            &render_image,
            animation_config,
            cx.background_executor(),
        );
        request_next_image_animation_frame(state, window, cx, animation_config);
        frame?
    } else {
        let frame = render_image.frame(0)?;
        state.current_frame = Some(frame.clone());
        state.next_frame_at = None;
        frame
    };

    Some((render_image, frame))
}

fn cancel_pending_sized_image(request: &ImageRenderRequest, window: &mut Window, cx: &mut App) {
    if let Some(task) = cx.take_asset::<SizedImageLoader>(request)
        && let Some(Ok(image)) = task.now_or_never()
    {
        cx.drop_image(image, Some(window));
        drop_image_asset_retained(hash(request));
    }
}

fn release_committed_sized_image(
    request: &ImageRenderRequest,
    current_image: Option<Arc<RenderImage>>,
    window: &mut Window,
    cx: &mut App,
) {
    let cached_image = cx
        .take_asset::<SizedImageLoader>(request)
        .and_then(|task| task.now_or_never())
        .and_then(Result::ok);

    if let Some(image) = current_image.or(cached_image) {
        cx.drop_image(image, Some(window));
    }
    drop_image_asset_retained(hash(request));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(label: &'static str, size: u32) -> ImageRenderRequest {
        ImageRenderRequest::new(
            crate::AssetLocation::Embedded(label.into()),
            crate::ImageRenderSize::new(size, size).unwrap(),
            1.0,
            ObjectFit::Cover,
        )
    }

    #[test]
    fn rapid_resize_keeps_committed_request_until_latest_is_ready() {
        let request_a = request("a", 256);
        let request_b = request("b", 320);
        let request_c = request("c", 384);
        let mut current = Some(request_a.clone());
        let mut pending = None;

        assert_eq!(
            update_pending_sized_request(false, &mut pending, &request_b),
            None
        );
        assert_eq!(pending.as_ref(), Some(&request_b));

        assert_eq!(
            update_pending_sized_request(false, &mut pending, &request_c),
            Some(request_b)
        );
        assert_eq!(current.as_ref(), Some(&request_a));
        assert_eq!(pending.as_ref(), Some(&request_c));

        assert_eq!(
            commit_sized_image_request(&mut current, &mut pending, &request_c),
            Some(request_a)
        );
        assert_eq!(current.as_ref(), Some(&request_c));
        assert!(pending.is_none());
    }

    #[test]
    fn returning_to_committed_size_cancels_pending_request() {
        let request_a = request("a", 256);
        let request_b = request("b", 320);
        let mut pending = Some(request_b.clone());

        assert_eq!(
            update_pending_sized_request(true, &mut pending, &request_a),
            Some(request_b)
        );
        assert!(pending.is_none());
    }
}
