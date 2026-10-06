// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

mod animation_bounds;
mod batch;
mod bounds_tree;
mod composite;
mod display_list;
mod geometry;
mod path;
mod path_builder;
mod prepared;
mod primitive;
mod renderer_extension;
#[cfg(test)]
mod tests;
mod transform;

pub(crate) type DrawOrder = u32;

pub(crate) use batch::*;
pub(crate) use bounds_tree::BoundsTree;
pub(crate) use display_list::*;
pub use path::Path;
pub(crate) use path::{PathCacheId, PathGeometryGeneration, PathId, PathVertex_ScaledPixels};
pub use path_builder::*;
pub(crate) use prepared::*;
pub(crate) use primitive::*;
pub use primitive::{BackdropBlurOverlapMode, BackdropBlurStyle, BorderStyle};
pub(crate) use renderer_extension::PaintRendererExtension;
pub use renderer_extension::{
    MemoryTrimLevel, RendererExtension, RendererExtensionContext, RendererExtensionRenderer,
};
pub use transform::TransformationMatrix;
