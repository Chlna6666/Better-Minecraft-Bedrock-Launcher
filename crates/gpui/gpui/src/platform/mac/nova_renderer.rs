//! AppKit adapter for the shared Nova renderer. Window and layer operations
//! remain on the AppKit owner thread; Scene rendering uses the common GPU graph.
use crate::{
    DevicePixels, GpuSpecs, PlatformAtlas, PresentationPacket, RendererOptions, Size,
    platform::nova::NovaRenderer,
};
use cocoa::base::id;
use foreign_types::ForeignType;
use std::{ffi::c_void, ptr::NonNull, sync::Arc};
use winit::raw_window_handle::{
    AppKitDisplayHandle, AppKitWindowHandle, DisplayHandle, HandleError, HasDisplayHandle,
    HasWindowHandle, WindowHandle,
};

pub(crate) type Context = RendererOptions;

// This target is borrowed only while creating the native surface. The AppKit
// window owns the NSView until the renderer is destroyed.
struct AppKitTarget(NonNull<c_void>);

impl HasWindowHandle for AppKitTarget {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: new_renderer is called with the live alloc/init NSView.
        Ok(unsafe { WindowHandle::borrow_raw(AppKitWindowHandle::new(self.0).into()) })
    }
}
impl HasDisplayHandle for AppKitTarget {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(unsafe { DisplayHandle::borrow_raw(AppKitDisplayHandle::new().into()) })
    }
}

pub(crate) struct Renderer {
    nova: NovaRenderer,
    // Stable CAMetalLayer identity for the lifetime of the Nova swapchain.
    layer: NonNull<c_void>,
}

pub(crate) unsafe fn new_renderer(
    context: Context,
    native_view: id,
    drawable_size: Size<DevicePixels>,
) -> crate::Result<Renderer> {
    let view = NonNull::new(native_view.cast()).ok_or_else(|| anyhow::anyhow!("null AppKit view"))?;
    let mut nova = NovaRenderer::new(
        &AppKitTarget(view),
        context.backend,
        &context,
        context.submission_mode,
        drawable_size,
        false,
    )?;
    let layer = match nova.metal_presentation_layer().and_then(|ptr| {
        NonNull::new(ptr).ok_or_else(|| anyhow::anyhow!("null Nova Metal layer"))
    }) {
        Ok(layer) => layer,
        Err(error) => {
            nova.destroy();
            return Err(error);
        }
    };
    // CoreAnimation uses the same layer as the native swapchain. Do not
    // allocate an additional legacy renderer, atlas, or command queue.
    Ok(Renderer { nova, layer })
}

impl Renderer {
    pub(crate) fn layer_ptr(&self) -> *mut c_void {
        self.layer.as_ptr()
    }

    pub(crate) fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.nova.platform_atlas()
    }

    pub(crate) fn gpu_specs(&self) -> GpuSpecs {
        self.nova.gpu_specs()
    }

    pub(crate) fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        self.nova.update_drawable_size(size, None);
    }

    pub(crate) fn update_transparency(&mut self, transparent: bool) {
        self.nova.update_transparency(transparent);
    }

    pub(crate) fn set_presents_with_transaction(&mut self, enabled: bool) {
        // Legacy CVDisplayLink fallback may request transaction presentation.
        // The Nova command-buffer present path must retain GPU ordering, so
        // leave presentsWithTransaction disabled rather than presenting early.
        let _ = enabled;
    }

    pub(crate) fn set_display_link_attached(&mut self, attached: bool) {
        if let Err(error) = self.nova.set_metal_display_link_attached(attached) {
            log::error!("Nova Metal display link state: {error:#}");
        }
    }

    pub(crate) fn set_frame_drawable(&mut self, drawable: Option<metal::MetalDrawable>) {
        let ptr = drawable.as_ref().and_then(|drawable| NonNull::new(drawable.as_ptr().cast()));
        // SAFETY: the retained foreign-types drawable remains live throughout
        // this call. Nova takes its own native reference until consumption.
        if let Err(error) = unsafe { self.nova.set_metal_display_link_drawable(ptr) } {
            log::debug!("Nova Metal drawable credit rejected: {error:#}");
        }
    }

    pub(crate) fn draw(&mut self, packet: PresentationPacket) -> bool {
        match self.nova.draw(packet) {
            Ok(submitted) => submitted,
            Err(error) => {
                log::error!("Nova Metal presentation: {error:#}");
                false
            }
        }
    }

    pub(crate) fn has_active_presentation(&self) -> bool {
        self.nova.has_active_presentation_animations()
    }

    pub(crate) fn set_frame_interval(&mut self, interval: Option<std::time::Duration>) {
        self.nova.set_frame_interval(interval);
    }

    pub(crate) fn present_active_frame(
        &mut self,
        now: std::time::Instant,
    ) -> smallvec::SmallVec<[crate::SceneAnimationCompletion; 4]> {
        let mut timing = crate::platform::frame::ActivePresentationTiming::from_frame_start(now);
        timing.display_tick_at = Some(now);
        match self.nova.present_active_frame(now, Some(timing)) {
            Ok(Some(frame)) => frame.completed_animations,
            Ok(None) => smallvec::SmallVec::new(),
            Err(error) => {
                log::error!("Nova Metal active presentation: {error:#}");
                smallvec::SmallVec::new()
            }
        }
    }

    pub(crate) fn take_animation_completions(&mut self) -> smallvec::SmallVec<[crate::SceneAnimationCompletion; 4]> {
        self.nova.take_animation_completions()
    }

    pub(crate) fn trim_gpui_memory(&mut self, level: crate::GpuiMemoryTrimLevel) {
        self.nova.trim_gpui_memory(level);
    }

    pub(crate) fn destroy(&mut self) {
        self.nova.destroy();
    }
}
