//! Native composition root for GPUI applications.

#![cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd", target_os = "macos"))]

use std::rc::Rc;

use gpui::{Application, BackgroundExecutor, RendererOptions};
pub use gpui::Platform;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub use gpui_linux::guess_compositor;
pub use gpui::enumerate_gpu_adapters;

/// Creates the native application with default renderer options.
pub fn application() -> Application {
    application_with_renderer_options(RendererOptions::default())
}

/// Creates the native application with the requested renderer options.
pub fn application_with_renderer_options(options: RendererOptions) -> Application {
    let options = options.resolve();
    gpui::diagnostics::performance_metrics::record_renderer_backend(options.backend);
    Application::with_platform(current_platform(false, options))
}

/// Creates an application with no visible windows.
pub fn headless() -> Application {
    Application::with_platform(current_platform(true, RendererOptions::default()))
}

/// Constructs a native or headless platform provider.
pub fn current_platform(headless: bool, options: RendererOptions) -> Rc<dyn Platform> {
    #[cfg(target_os = "windows")]
    {
        if headless {
            return Rc::new(gpui_windows::WindowsPlatform::new_headless());
        }
        Rc::new(
            gpui_windows::WindowsPlatform::new(options)
                .inspect_err(|error| gpui_windows::dialogs::show_error("Failed to launch", error.to_string()))
                .expect("failed to initialize Windows platform"),
        )
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        gpui_linux::current_platform(headless, options)
    }
    #[cfg(target_os = "macos")]
    Rc::new(gpui_macos::MacPlatform::with_renderer_options(headless, options))
}

/// Returns a native background executor without opening a window.
pub fn background_executor() -> BackgroundExecutor {
    current_platform(true, RendererOptions::default()).background_executor()
}
