use super::element::SizedImageLoader;
use super::loader::{ImageRenderRequest, image_size_for_window};
use super::playback::{
    request_next_image_animation_frame, select_animation_frame,
    should_request_image_animation_frame,
};
use super::retained::{SizedImageElementState, SizedImageRequestLease};
use super::source::ImageSource;
use super::style::ImageAnimationPolicy;
use super::layout::ImageLayout;
use crate::{
    AnimatedFrame, App, Bounds, GlobalElementId, ImageBoundsPolicy, ObjectFit, Pixels, RenderImage,
    Window,
};
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
        return window.with_element_state(
            global_id,
            |state: Option<SizedImageElementState>, window| {
                let mut state = state
                    .unwrap_or_else(|| SizedImageElementState::new(layout_state.frame.clone()));

                // The committed lease owns the request currently visible on screen. The pending
                // lease owns only the newest replacement. Replacing B with C moves ownership to C
                // before B is released, while A remains retained until C produces a drawable frame.
                let current_matches_requested = state
                    .sized_image_request
                    .as_ref()
                    .is_some_and(|lease| lease.request() == &requested);
                if let Some(stale_pending) = update_pending_sized_request(
                    current_matches_requested,
                    &mut state.pending_sized_image_drop,
                    &requested,
                    cx,
                ) {
                    release_sized_image_lease(stale_pending, None, Some(window), cx);
                }

                if current_matches_requested {
                    let loaded =
                        render_current_sized_image(&mut state, animation_config, window, cx);
                    return (loaded, state);
                }

                let result = window.use_asset::<SizedImageLoader>(&requested, cx);
                let loaded = match result {
                    Some(Ok(render_image)) => {
                        let previous_frame = state.playback.current_frame.clone();
                        let previous_next_frame_at = state.playback.next_frame_at;
                        state.playback.current_frame = render_image.frame(0);
                        state.playback.next_frame_at = None;

                        let frame = if should_request_image_animation_frame(
                            &render_image,
                            animation_config,
                        ) {
                            let frame = select_animation_frame(
                                &mut state.playback,
                                &render_image,
                                animation_config,
                                cx.background_executor(),
                            );
                            request_next_image_animation_frame(
                                &state.playback,
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
                            state.playback.current_frame = Some(frame.clone());
                            if !render_image.is_animated() || !animation_config.play {
                                state.playback.next_frame_at = None;
                            }

                            if let Some(previous_request) = previous_request {
                                release_sized_image_lease(
                                    previous_request,
                                    previous_image,
                                    Some(window),
                                    cx,
                                );
                            }

                            Some((render_image, frame))
                        } else {
                            state.playback.current_frame = previous_frame;
                            state.playback.next_frame_at = previous_next_frame_at;
                            render_current_sized_image(
                                &mut state,
                                animation_config,
                                window,
                                cx,
                            )
                        }
                    }
                    Some(Err(_)) | None => {
                        render_current_sized_image(&mut state, animation_config, window, cx)
                    }
                };

                (loaded, state)
            },
        );
    }

    let render_image = window.use_asset::<SizedImageLoader>(&requested, cx)?.ok()?;
    let frame = render_image.frame(0)?;
    Some((render_image, frame))
}

fn update_pending_sized_request(
    current_matches_requested: bool,
    pending: &mut Option<SizedImageRequestLease>,
    requested: &ImageRenderRequest,
    cx: &mut App,
) -> Option<SizedImageRequestLease> {
    if current_matches_requested {
        return pending.take();
    }

    if pending
        .as_ref()
        .is_some_and(|pending_request| pending_request.request() == requested)
    {
        return None;
    }

    let replacement = SizedImageRequestLease::acquire(requested, cx);
    pending.replace(replacement)
}

fn commit_sized_image_request(
    current: &mut Option<SizedImageRequestLease>,
    pending: &mut Option<SizedImageRequestLease>,
    requested: &ImageRenderRequest,
) -> Option<SizedImageRequestLease> {
    let committed = pending
        .take()
        .expect("sized image became drawable without a pending request lease");
    debug_assert_eq!(committed.request(), requested);
    current.replace(committed)
}

fn render_current_sized_image(
    state: &mut SizedImageElementState,
    animation_config: crate::AnimatedImageConfig,
    window: &mut Window,
    cx: &App,
) -> Option<(Arc<RenderImage>, AnimatedFrame)> {
    let render_image = state.current_image.clone()?;
    let frame = if should_request_image_animation_frame(&render_image, animation_config) {
        let frame = select_animation_frame(
            &mut state.playback,
            &render_image,
            animation_config,
            cx.background_executor(),
        );
        request_next_image_animation_frame(&state.playback, window, cx, animation_config);
        frame?
    } else {
        let frame = render_image.frame(0)?;
        state.playback.current_frame = Some(frame.clone());
        state.playback.next_frame_at = None;
        frame
    };

    Some((render_image, frame))
}

fn release_sized_image_lease(
    lease: SizedImageRequestLease,
    image: Option<Arc<RenderImage>>,
    current_window: Option<&mut Window>,
    cx: &mut App,
) {
    let request = lease.into_request();
    cx.release_sized_image_element_request(&request, image, current_window);
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
        let test = crate::TestAppContext::single();
        let request_a = request("a", 256);
        let request_b = request("b", 320);
        let request_c = request("c", 384);

        test.update(|cx| {
            let mut current = Some(SizedImageRequestLease::acquire(&request_a, cx));
            let mut pending = None;

            assert!(
                update_pending_sized_request(false, &mut pending, &request_b, cx).is_none()
            );
            assert_eq!(
                pending.as_ref().map(SizedImageRequestLease::request),
                Some(&request_b)
            );
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request_a), 1);
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request_b), 1);

            let stale = update_pending_sized_request(false, &mut pending, &request_c, cx)
                .expect("intermediate request should be replaced");
            assert_eq!(stale.request(), &request_b);
            release_sized_image_lease(stale, None, None, cx);
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request_b), 0);
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request_c), 1);
            assert_eq!(
                current.as_ref().map(SizedImageRequestLease::request),
                Some(&request_a)
            );

            let previous = commit_sized_image_request(&mut current, &mut pending, &request_c)
                .expect("committed request should replace A");
            assert_eq!(previous.request(), &request_a);
            release_sized_image_lease(previous, None, None, cx);
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request_a), 0);
            assert_eq!(
                current.as_ref().map(SizedImageRequestLease::request),
                Some(&request_c)
            );
            assert!(pending.is_none());

            release_sized_image_lease(current.take().unwrap(), None, None, cx);
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request_c), 0);
        });
    }

    #[test]
    fn returning_to_committed_size_releases_pending_request() {
        let test = crate::TestAppContext::single();
        let request_a = request("a", 256);
        let request_b = request("b", 320);

        test.update(|cx| {
            let current = SizedImageRequestLease::acquire(&request_a, cx);
            let mut pending = Some(SizedImageRequestLease::acquire(&request_b, cx));

            let stale = update_pending_sized_request(true, &mut pending, &request_a, cx)
                .expect("pending request should be cancelled");
            assert_eq!(stale.request(), &request_b);
            release_sized_image_lease(stale, None, None, cx);
            assert!(pending.is_none());
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request_a), 1);
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request_b), 0);

            release_sized_image_lease(current, None, None, cx);
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request_a), 0);
        });
    }
}
