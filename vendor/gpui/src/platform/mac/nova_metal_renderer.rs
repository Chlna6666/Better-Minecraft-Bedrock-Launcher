use crate::{
    DevicePixels, GpuSpecs, GpuiMemoryTrimLevel, PlatformAtlas, RendererBackend, Scene, Size,
    platform::NovaRenderer,
};
use anyhow::Result;
use cocoa::foundation::NSSize;
use foreign_types::{ForeignType, ForeignTypeRef};
use raw_window_handle as rwh;
use std::{
    ffi::c_void,
    ptr::NonNull,
    sync::{Arc, Mutex},
};

pub type Context = Arc<Mutex<NovaMetalContext>>;
pub type Renderer = NovaMetalRenderer;

#[derive(Default)]
pub struct NovaMetalContext;

pub unsafe fn new_renderer(
    _context: self::Context,
    _native_window: *mut c_void,
    native_view: *mut c_void,
    bounds: crate::Size<f32>,
    transparent: bool,
) -> Renderer {
    let native_view = NonNull::new(native_view).unwrap_or_else(|| {
        log::error!("nova-gfx Metal renderer received a null NSView pointer");
        std::process::exit(1);
    });
    match NovaMetalRenderer::new(native_view, bounds, transparent) {
        Ok(renderer) => renderer,
        Err(error) => {
            log::error!("failed to create nova-gfx Metal renderer: {error:#}");
            std::process::exit(1);
        }
    }
}

pub struct NovaMetalRenderer {
    layer: metal::MetalLayer,
    renderer: NovaRenderer,
}

impl NovaMetalRenderer {
    fn new(native_view: NonNull<c_void>, bounds: Size<f32>, transparent: bool) -> Result<Self> {
        let window = NativeViewWindow { native_view };
        let drawable_size = Size {
            width: DevicePixels(bounds.width.max(1.0).ceil() as i32),
            height: DevicePixels(bounds.height.max(1.0).ceil() as i32),
        };
        let renderer = NovaRenderer::new(
            &window,
            RendererBackend::NovaMetal,
            crate::GpuSubmissionMode::Deferred,
            drawable_size,
            transparent,
        )?;
        let layer = metal_layer_from_view(native_view)?;
        Ok(Self { layer, renderer })
    }

    pub fn layer(&self) -> &metal::MetalLayerRef {
        &self.layer
    }

    pub fn layer_ptr(&self) -> *mut metal::CAMetalLayer {
        self.layer.as_ptr()
    }

    pub fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.renderer.sprite_atlas()
    }

    pub fn set_presents_with_transaction(&mut self, presents_with_transaction: bool) {
        self.layer
            .set_presents_with_transaction(presents_with_transaction);
    }

    pub fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        let drawable_size = NSSize {
            width: size.width.0.max(1) as f64,
            height: size.height.0.max(1) as f64,
        };
        self.layer.set_drawable_size(metal::CGSize::new(
            drawable_size.width,
            drawable_size.height,
        ));
        if let Err(error) = self.renderer.resize(size) {
            log::error!("failed to resize nova-gfx Metal swapchain: {error:#}");
        }
    }

    pub fn update_transparency(&mut self, transparent: bool) {
        self.renderer.update_transparency(transparent);
    }

    pub fn destroy(&mut self) {
        self.renderer.destroy();
    }

    pub fn draw(&mut self, scene: &Scene) {
        if let Err(error) = self.renderer.draw_scene_for_platform(scene) {
            log::error!("failed to draw nova-gfx Metal scene: {error:#}");
        }
    }

    pub fn render(&mut self, scene: &Scene) {
        self.draw(scene);
    }

    pub fn gpu_specs(&self) -> GpuSpecs {
        self.renderer.gpu_specs()
    }

    pub fn trim_gpui_memory(&mut self, level: GpuiMemoryTrimLevel) {
        self.renderer.trim_gpui_memory(level);
    }
}

struct NativeViewWindow {
    native_view: NonNull<c_void>,
}

impl rwh::HasWindowHandle for NativeViewWindow {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        let handle = rwh::AppKitWindowHandle::new(self.native_view);
        // SAFETY: `native_view` is provided by the owning AppKit window and remains alive while
        // this borrowed raw-window-handle is used to create the Metal surface.
        Ok(unsafe { rwh::WindowHandle::borrow_raw(rwh::RawWindowHandle::AppKit(handle)) })
    }
}

impl rwh::HasDisplayHandle for NativeViewWindow {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        let handle = rwh::AppKitDisplayHandle::new();
        // SAFETY: AppKit display handles do not carry additional borrowed state.
        Ok(unsafe { rwh::DisplayHandle::borrow_raw(handle.into()) })
    }
}

fn metal_layer_from_view(native_view: NonNull<c_void>) -> Result<metal::MetalLayer> {
    let view = native_view.as_ptr() as cocoa::base::id;
    // SAFETY: `native_view` points to a live NSView. The nova-gfx Metal backend sets a
    // CAMetalLayer on that view during surface creation, so this layer pointer is retainable.
    let layer = unsafe {
        let layer: cocoa::base::id = objc::msg_send![view, layer];
        anyhow::ensure!(
            !layer.is_null(),
            "nova-gfx Metal did not attach a CAMetalLayer"
        );
        let retained_layer: cocoa::base::id = objc::msg_send![layer, retain];
        metal::MetalLayer::from_ptr(retained_layer.cast())
    };
    Ok(layer)
}
