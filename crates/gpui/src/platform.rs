mod app_menu;
mod atlas;
mod bootstrap;
mod clipboard;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod cosmic_text_system;
mod display;
mod foreground_tasks;
mod frame;
mod gpu;
mod input_handler;
mod interaction;
mod keyboard;
mod platform_traits;
mod screen_capture;
mod winit;

#[cfg(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "linux",
    target_os = "freebsd"
))]
mod nova;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux;

#[cfg(target_os = "macos")]
mod mac;

#[cfg(all(target_os = "macos", feature = "macos-blade"))]
mod blade;

#[cfg(any(test, feature = "test-support"))]
mod test;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(all(
    feature = "screen-capture",
    any(
        target_os = "windows",
        all(
            any(target_os = "linux", target_os = "freebsd"),
            any(feature = "wayland", feature = "x11"),
        )
    )
))]
pub(crate) mod scap_screen_capture;

pub use app_menu::*;
pub(crate) use atlas::*;
pub(crate) use bootstrap::current_platform;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub use bootstrap::guess_compositor;
#[cfg(target_os = "windows")]
pub use bootstrap::windows_manifest_path;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use bootstrap::{TestDispatcher, TestScreenCaptureSource, TestScreenCaptureStream};
pub use bootstrap::{background_executor, enumerate_gpu_adapters};
pub use clipboard::*;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) use cosmic_text_system::*;
pub use display::*;
pub(crate) use foreground_tasks::*;
pub(crate) use frame::*;
pub use gpu::*;
pub use input_handler::*;
pub use interaction::*;
pub use keyboard::*;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) use linux::*;
#[cfg(target_os = "macos")]
pub(crate) use mac::*;
#[cfg(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "linux",
    target_os = "freebsd"
))]
pub(crate) use nova::*;
pub use platform_traits::*;
pub use screen_capture::*;
pub use semantic_version::SemanticVersion;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use test::*;
#[cfg(target_os = "windows")]
pub(crate) use windows::*;
