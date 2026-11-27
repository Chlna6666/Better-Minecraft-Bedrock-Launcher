mod app_menu;
#[doc(hidden)]
pub mod atlas;
mod bootstrap;
mod clipboard;
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos"))]
mod cosmic_text_system;
mod display;
#[doc(hidden)]
pub mod foreground_tasks;
#[doc(hidden)]
pub mod frame;
mod gpu;
mod input_handler;
mod interaction;
mod keyboard;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod presentation_owner;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
#[doc(hidden)]
pub mod render_owner;
#[doc(hidden)]
pub mod traits;

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
#[doc(hidden)]
pub mod nova;


#[cfg(all(target_os = "macos", not(feature = "macos-blade"), not(feature = "nova-gfx-metal")))]
#[doc(hidden)]
pub mod metal;

#[cfg(all(target_os = "macos", feature = "macos-blade"))]
#[doc(hidden)]
pub mod blade;

#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
mod test;


pub use app_menu::*;
pub use atlas::*;
#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
pub(crate) use bootstrap::TestDispatcher;
#[cfg(target_os = "windows")]
pub use bootstrap::windows_manifest_path;
pub use bootstrap::enumerate_gpu_adapters;
pub use clipboard::*;
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos"))]
#[doc(hidden)]
pub use cosmic_text_system::CosmicTextSystem;
pub use display::*;
pub use foreground_tasks::*;
pub use frame::*;
pub use gpu::*;
pub use input_handler::*;
pub use interaction::*;
pub use keyboard::*;
#[cfg(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "linux",
    target_os = "freebsd"
))]
pub use nova::{NovaRenderer, NovaRendererAtlas};
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd"))]
pub(crate) use nova::*;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[doc(hidden)]
pub use presentation_owner::OwnedNovaRenderer;
pub use semantic_version::SemanticVersion;
#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
pub(crate) use test::*;
pub use traits::*;
