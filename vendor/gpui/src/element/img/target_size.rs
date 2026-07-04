use super::element::TargetSizeImgResourceLoader;
use super::loader::{TargetSizeImageSource, target_size_for_bounds};
use super::source::ImageSource;
use super::state::{
    ImgLayoutState, ImgState, request_next_image_animation_frame, select_animation_frame,
    should_request_image_animation_frame,
};
use super::style::ImageAnimationPolicy;
use crate::{
    AnimatedFrame, App, Bounds, GlobalElementId, ObjectFit, Pixels, RenderImage, Window,
    drop_image_asset_retained, hash,
};
use futures::FutureExt;
use std::sync::Arc;

pub(super) fn use_target_size_data(
    source: ImageSource,
    object_fit: ObjectFit,
    animation_policy: ImageAnimationPolicy,
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
    let requested = TargetSizeImageSource::new(resource, target, window.scale_factor(), object_fit);

    let animation_config = animation_policy
        .apply_to(cx.image_pipeline_config().animated)
        .clamped();
    if let Some(global_id) = global_id {
        return window.with_element_state(global_id, |state: Option<ImgState>, window| {
            let mut state = state.unwrap_or(ImgState {
                current_image: None,
                current_frame: layout_state.frame.clone(),
                next_frame_at: None,
                started_loading: None,
                target_size_asset: None,
                pending_target_drop: None,
            });

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
                    let frame = if should_request_image_animation_frame(&data, animation_config) {
                        let frame = select_animation_frame(
                            &mut state,
                            &data,
                            animation_config,
                            cx.background_executor(),
                        );
                        request_next_image_animation_frame(&state, window, cx, animation_config);
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
    let frame = data.frame(0)?;
    Some((data, frame))
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
        drop_image_asset_retained(hash(previous));
    }
}
