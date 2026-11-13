use super::{BladeContext, BladeRenderer, BladeSurfaceConfig};
use std::{ffi::c_void, ptr::NonNull};

#[derive(Clone)]
pub struct Context {
    inner: BladeContext,
}
impl Default for Context {
    fn default() -> Self {
        Self {
            inner: BladeContext::new().unwrap(),
        }
    }
}

pub type Renderer = BladeRenderer;

pub unsafe fn new_renderer(
    context: Context,
    _native_window: *mut c_void,
    native_view: *mut c_void,
    bounds: crate::Size<f32>,
    transparent: bool,
) -> Renderer {
    use ::winit::raw_window_handle;
    struct RawWindow {
        view: *mut c_void,
    }

    impl raw_window_handle::HasWindowHandle for RawWindow {
        fn window_handle(
            &self,
        ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
            let view = NonNull::new(self.view).unwrap();
            let handle = raw_window_handle::AppKitWindowHandle::new(view);
            Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(handle.into()) })
        }
    }
    impl raw_window_handle::HasDisplayHandle for RawWindow {
        fn display_handle(
            &self,
        ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
            let handle = raw_window_handle::AppKitDisplayHandle::new();
            Ok(unsafe { raw_window_handle::DisplayHandle::borrow_raw(handle.into()) })
        }
    }

    BladeRenderer::new(
        &context.inner,
        &RawWindow {
            view: native_view as *mut _,
        },
        BladeSurfaceConfig {
            size: blade_graphics::Extent {
                width: bounds.width as u32,
                height: bounds.height as u32,
                depth: 1,
            },
            transparent,
        },
    )
    .unwrap()
}
