// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

mod batch;
mod bounds_tree;
mod mesh;
mod path;
mod path_builder;
mod prepared;
mod primitive;
mod scene_model;
#[cfg(test)]
mod tests;
mod transform;
mod util;

pub(crate) type DrawOrder = u32;

pub(crate) use batch::*;
pub(crate) use bounds_tree::BoundsTree;
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
pub use primitive::{BackdropBlurStyle, BorderStyle};
pub(crate) use scene_model::*;
pub use transform::TransformationMatrix;
