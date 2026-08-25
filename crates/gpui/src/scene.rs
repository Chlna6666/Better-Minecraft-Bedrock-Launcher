// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

mod batch;
mod bounds_tree;
mod display_list;
mod geometry;
mod mesh;
mod path;
mod path_builder;
mod prepared;
mod primitive;
#[cfg(test)]
mod tests;
mod transform;

pub(crate) type DrawOrder = u32;

pub(crate) use batch::*;
pub(crate) use bounds_tree::BoundsTree;
pub(crate) use display_list::*;
pub(crate) use mesh::PaintGpuMesh3d;
pub use mesh::{
    GpuMesh3d, GpuMesh3dDrawParameters, GpuMesh3dDrawRanges, GpuMesh3dId, GpuMesh3dRange,
    GpuMesh3dShader, GpuMesh3dShaderId, GpuMesh3dVertex,
};
pub use path::Path;
pub(crate) use path::{PathCacheId, PathGeometryGeneration, PathId, PathVertex_ScaledPixels};
pub use path_builder::*;
pub(crate) use prepared::*;
pub(crate) use primitive::*;
pub use primitive::{BackdropBlurOverlapMode, BackdropBlurStyle, BorderStyle};
pub use transform::TransformationMatrix;
