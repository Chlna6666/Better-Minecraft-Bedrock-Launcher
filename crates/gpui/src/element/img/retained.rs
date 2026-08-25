use std::{sync::Arc, time::Instant};

use crate::{AnimatedFrame, App, AsyncApp, RenderImage, Task};

use super::loader::ImageRenderRequest;

/// Playback and loading values retained by ordinary image elements between frames.
pub(crate) struct ImageElementState {
    pub(crate) current_image: Option<Arc<RenderImage>>,
    pub(crate) current_frame: Option<AnimatedFrame>,
    pub(super) next_frame_at: Option<Instant>,
    pub(super) started_loading: Option<(Instant, Task<()>)>,
}

/// One element-owned reference to a concrete bounds-aware image request.
///
/// The token itself is intentionally not `Clone`: moving it between pending/current slots transfers
/// ownership without changing the application-wide owner count. If the whole element state leaves
/// the frame, release work is posted to the foreground executor so it runs after the current window
/// update has returned the window to `App.windows`.
pub(super) struct SizedImageRequestLease {
    request: ImageRenderRequest,
    app: AsyncApp,
}

impl SizedImageRequestLease {
    pub(super) fn acquire(request: &ImageRenderRequest, cx: &mut App) -> Self {
        cx.retain_sized_image_element_request(request);
        Self {
            request: request.clone(),
            app: cx.to_async(),
        }
    }

    pub(super) fn request(&self) -> &ImageRenderRequest {
        &self.request
    }

    pub(crate) fn into_request(self) -> ImageRenderRequest {
        self.request
    }

    fn defer_release(self, image: Option<Arc<RenderImage>>) {
        let Self { request, app } = self;
        app.spawn(async move |cx| {
            let _ = cx.update(|cx| {
                cx.release_sized_image_element_request(&request, image, None);
            });
        })
        .detach();
    }
}

/// Bounds-aware image state is kept separate from ordinary image playback state so switching
/// rendering modes naturally retires the old state at the frame boundary.
pub(crate) struct SizedImageElementState {
    pub(crate) playback: ImageElementState,
    pub(crate) current_image: Option<Arc<RenderImage>>,
    pub(super) sized_image_request: Option<SizedImageRequestLease>,
    pub(super) pending_sized_image_drop: Option<SizedImageRequestLease>,
}

impl SizedImageElementState {
    pub(crate) fn new(current_frame: Option<AnimatedFrame>) -> Self {
        Self {
            playback: ImageElementState {
                current_image: None,
                current_frame,
                next_frame_at: None,
                started_loading: None,
            },
            current_image: None,
            sized_image_request: None,
            pending_sized_image_drop: None,
        }
    }
}

impl Drop for SizedImageElementState {
    fn drop(&mut self) {
        if let Some(current) = self.sized_image_request.take() {
            current.defer_release(self.current_image.take());
        } else {
            self.current_image = None;
        }

        if let Some(pending) = self.pending_sized_image_drop.take() {
            pending.defer_release(None);
        }
    }
}
