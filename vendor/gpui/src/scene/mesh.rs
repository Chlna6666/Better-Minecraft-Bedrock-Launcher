use crate::{Bounds, ContentMask, ScaledPixels, WgslShaderSource};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering::SeqCst},
};

use super::{DrawOrder, Primitive};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
/// Stable identity for a GPU-backed 3D mesh.
pub struct GpuMesh3dId(pub usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
/// Stable identity for a runtime WGSL shader used by a GPU-backed 3D mesh.
pub struct GpuMesh3dShaderId(pub usize);

/// Runtime WGSL shader entry points for a GPU-backed 3D mesh.
#[derive(Clone, Debug)]
pub struct GpuMesh3dShader {
    /// Shader identity used by renderers to cache backend pipeline objects.
    pub id: GpuMesh3dShaderId,
    /// Validated WGSL source supplied by application code.
    pub source: Arc<WgslShaderSource>,
    /// Vertex entry point name.
    pub vertex_entry_point: String,
    /// Fragment entry point name.
    pub fragment_entry_point: String,
}

impl GpuMesh3dShader {
    /// Creates a GPU mesh shader from validated WGSL source and entry point names.
    pub fn new(
        source: Arc<WgslShaderSource>,
        vertex_entry_point: impl Into<String>,
        fragment_entry_point: impl Into<String>,
    ) -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

        Self {
            id: GpuMesh3dShaderId(NEXT_ID.fetch_add(1, SeqCst)),
            source,
            vertex_entry_point: vertex_entry_point.into(),
            fragment_entry_point: fragment_entry_point.into(),
        }
    }
}

/// A vertex in a GPU-backed 3D mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct GpuMesh3dVertex {
    /// Model-space x, y, z position.
    pub position: [f32; 3],
    /// Linear RGBA color used by the mesh fragment shader.
    pub color: [f32; 4],
}

/// A contiguous index range inside a GPU-backed 3D mesh.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct GpuMesh3dRange {
    /// First index in the range.
    pub start: u32,
    /// Number of indices in the range.
    pub count: u32,
}

/// Draw ranges for the supported 3D mesh material passes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct GpuMesh3dDrawRanges {
    /// Opaque geometry drawn with depth writes enabled.
    pub opaque: GpuMesh3dRange,
    /// Transparent glass geometry drawn after opaque geometry.
    pub glass: GpuMesh3dRange,
    /// Transparent water geometry drawn after glass geometry.
    pub water: GpuMesh3dRange,
}

/// Per-draw parameters for a GPU-backed 3D mesh draw.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct GpuMesh3dDrawParameters {
    /// Combined view-projection-model matrix in column-major order.
    pub view_projection_model: [[f32; 4]; 4],
}

/// Immutable 3D mesh data that can be drawn by GPUI's renderer.
#[derive(Clone, Debug)]
pub struct GpuMesh3d {
    /// Mesh identity used by renderers to cache uploaded vertex buffers.
    pub id: GpuMesh3dId,
    /// Mesh generation used to invalidate cached GPU buffers for this id.
    pub generation: u64,
    /// Packed vertex buffer for all draw ranges.
    pub vertices: Arc<[GpuMesh3dVertex]>,
    /// Packed index buffer for all draw ranges.
    pub indices: Arc<[u32]>,
    /// Material index ranges within `indices`.
    pub ranges: GpuMesh3dDrawRanges,
    /// Model-space center for renderer diagnostics and optional sorting.
    pub center: [f32; 3],
    /// Fit scale for renderer diagnostics.
    pub fit_scale: f32,
    /// Vertical scale for renderer diagnostics.
    pub vertical_scale: f32,
    /// Runtime shader used to draw this mesh.
    pub shader: Arc<GpuMesh3dShader>,
}

impl GpuMesh3d {
    /// Creates a new GPU-backed 3D mesh with a fresh renderer cache id.
    pub fn new(
        vertices: impl Into<Arc<[GpuMesh3dVertex]>>,
        indices: impl Into<Arc<[u32]>>,
        ranges: GpuMesh3dDrawRanges,
        center: [f32; 3],
        fit_scale: f32,
        vertical_scale: f32,
        shader: Arc<GpuMesh3dShader>,
    ) -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

        Self {
            id: GpuMesh3dId(NEXT_ID.fetch_add(1, SeqCst)),
            generation: 0,
            vertices: vertices.into(),
            indices: indices.into(),
            ranges,
            center,
            fit_scale,
            vertical_scale,
            shader,
        }
    }

    /// Sets the generation used by renderers to refresh cached GPU buffers.
    pub fn with_generation(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PaintGpuMesh3d {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub mesh: Arc<GpuMesh3d>,
    pub parameters: GpuMesh3dDrawParameters,
}

impl From<PaintGpuMesh3d> for Primitive {
    fn from(mesh: PaintGpuMesh3d) -> Self {
        Primitive::GpuMesh3d(mesh)
    }
}
