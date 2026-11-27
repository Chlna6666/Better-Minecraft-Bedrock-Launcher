//! Metal scene renderer for the legacy AppKit presentation path.

mod metal_atlas;
mod metal_renderer;

pub use metal_renderer::{Context, Renderer, new_renderer};
