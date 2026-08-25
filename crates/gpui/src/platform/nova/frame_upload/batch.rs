use super::*;

#[derive(Clone, Copy, Default)]
pub(in crate::platform::nova) struct UnsupportedBatchSummary {
    pub(in crate::platform::nova) paths: u32,
    pub(in crate::platform::nova) surfaces: u32,
    pub(in crate::platform::nova) backdrop_blurs: u32,
    pub(in crate::platform::nova) backdrop_blur_tint_fallbacks: u32,
    pub(in crate::platform::nova) gpu_meshes_3d: u32,
}

impl UnsupportedBatchSummary {
    pub(in crate::platform::nova) fn total(self) -> u32 {
        self.paths
            .saturating_add(self.surfaces)
            .saturating_add(self.backdrop_blurs)
            .saturating_add(self.backdrop_blur_tint_fallbacks)
            .saturating_add(self.gpu_meshes_3d)
    }
}

#[derive(Clone, Copy)]
pub(in crate::platform::nova) enum UploadedBatch {
    SolidQuads {
        first: u32,
        count: u32,
    },
    Quads {
        first: u32,
        count: u32,
    },
    Shadows {
        first: u32,
        count: u32,
    },
    PathRasterization {
        first_vertex: u32,
        vertex_count: u32,
    },
    Paths {
        first: u32,
        count: u32,
    },
    MonoSprites {
        texture_id: AtlasTextureId,
        first: u32,
        count: u32,
    },
    PolySprites {
        texture_id: AtlasTextureId,
        first: u32,
        count: u32,
    },
    Underlines {
        first: u32,
        count: u32,
    },
    BackdropBlurs {
        first: u32,
        count: u32,
    },
    CustomMesh3d {
        mesh_id: GpuMesh3dId,
        generation: u64,
        shader_id: GpuMesh3dShaderId,
        range: GpuMesh3dRange,
        first_parameter_index: u32,
    },
}
