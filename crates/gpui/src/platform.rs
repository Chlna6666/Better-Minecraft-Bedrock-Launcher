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
mod traits;
mod winit;

#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

/// Keeps an operating-system activity alive until the guard is dropped.
///
/// GPUI uses this for scoped policies such as preventing idle system sleep during long-running
/// foreground work. Dropping the guard releases the platform request exactly once.
pub struct ActivityGuard {
    release: Option<Box<dyn FnOnce() + Send>>,
}

impl ActivityGuard {
    /// Creates a guard that invokes `release` when dropped.
    pub fn new(release: impl FnOnce() + Send + 'static) -> Self {
        Self {
            release: Some(Box::new(release)),
        }
    }

    /// Creates a guard that performs no platform action when dropped.
    pub fn noop() -> Self {
        Self::new(|| {})
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}


#[cfg(target_os = "windows")]
static TEXT_RASTERIZATION_GENERATION: AtomicU64 = AtomicU64::new(0);

#[cfg(target_os = "windows")]
pub(crate) fn text_rasterization_generation() -> u64 {
    // This counter is only a change token. It does not publish or guard any associated data, so
    // stronger ordering would add synchronization semantics without improving correctness.
    TEXT_RASTERIZATION_GENERATION.load(AtomicOrdering::Relaxed)
}

#[cfg(target_os = "windows")]
pub(crate) fn advance_text_rasterization_generation() {
    TEXT_RASTERIZATION_GENERATION.fetch_add(1, AtomicOrdering::Relaxed);
}

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

pub use app_menu::*;
pub(crate) use atlas::*;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use bootstrap::TestDispatcher;
pub(crate) use bootstrap::current_platform;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub use bootstrap::guess_compositor;
#[cfg(target_os = "windows")]
pub use bootstrap::windows_manifest_path;
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
pub use semantic_version::SemanticVersion;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use test::*;
pub use traits::*;
#[cfg(target_os = "windows")]
pub(crate) use windows::*;
