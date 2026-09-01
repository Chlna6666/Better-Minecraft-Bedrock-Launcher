#![cfg_attr(
    not(any(
        all(feature = "nova-gfx-dx12", target_os = "windows"),
        all(feature = "nova-gfx-metal", target_os = "macos"),
        all(
            feature = "nova-gfx-vulkan",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        )
    )),
    allow(
        dead_code,
        unreachable_code,
        unused_assignments,
        unused_imports,
        unused_variables
    )
)]

mod atlas;
mod atlas_resources;
mod backend;
mod blur_damage;
mod composite_target;
mod diagnostics;
mod draw;
mod frame_upload;
mod limits;
mod pipeline;
mod prelude;
mod renderer;
mod rendering_parameters;
mod resource_bindings;
mod resource_layouts;
mod resources;
mod shader;
mod surface;
mod surface_plan;
mod swapchain;
mod targets;
mod upload_metrics;
mod upload_packing;

use crate::{DirtyRegion, ImageId};
use atlas::*;
use atlas_resources::*;
use backend::*;
use blur_damage::*;
use composite_target::*;
use diagnostics::*;
use draw::*;
use frame_upload::*;
use limits::*;
use pipeline::*;
use prelude::*;
#[cfg(test)]
use renderer::nova_present_mode_for_backend;
use renderer::{DrawableSize, MeshCacheEntry};
pub(crate) use renderer::{NovaRenderer, NovaRendererAtlas};
use rendering_parameters::*;
use resource_bindings::*;
use resource_layouts::*;
use resources::*;
use shader::*;
use surface::*;
use surface_plan::*;
use swapchain::*;
use targets::*;
use upload_metrics::*;
use upload_packing::*;

#[cfg(feature = "bench")]
pub(crate) use frame_upload::{
    upload_encoding::AtlasPixelEncodingBenchmarkCore, upload_queue::AtlasUploadBenchmarkCore,
};

#[cfg(feature = "bench")]
pub(crate) struct FrameUploadBenchmarkCore {
    scene: crate::Scene,
    upload: FrameUpload,
    rendering_parameters: RenderingParameters,
}

#[cfg(feature = "bench")]
impl FrameUploadBenchmarkCore {
    pub(crate) fn new(scene: crate::Scene) -> Self {
        Self {
            scene,
            upload: FrameUpload::default(),
            rendering_parameters: RenderingParameters::from_env(),
        }
    }

    pub(crate) fn next_frame(&mut self) -> (usize, usize, usize, usize, usize) {
        let summary = self.upload.encode(
            &self.scene,
            DrawableSize {
                width: 1_920,
                height: 1_080,
            },
            &self.rendering_parameters,
            true,
            BackdropBlurQuality::Full,
        );
        (
            usize::try_from(summary.quad_count)
                .unwrap_or(usize::MAX)
                .saturating_add(usize::try_from(summary.backdrop_blur_count).unwrap_or(usize::MAX)),
            self.upload.batches.len(),
            self.upload.uploaded_bytes(),
            self.upload.retained_byte_capacity(),
            self.upload.backdrop_blur_configs().len(),
        )
    }
}

#[cfg(feature = "bench")]
pub(crate) struct PathPackingBenchmarkCore {
    scene: crate::Scene,
    upload: FrameUpload,
    rendering_parameters: RenderingParameters,
}

#[cfg(feature = "bench")]
impl PathPackingBenchmarkCore {
    pub(crate) fn new(scene: crate::Scene) -> Self {
        Self {
            scene,
            upload: FrameUpload::default(),
            rendering_parameters: RenderingParameters::from_env(),
        }
    }

    pub(crate) fn encode_cache_miss(&mut self) -> usize {
        self.upload.path_rasterization_cache.clear();
        self.upload.path_geometry_hash_memo.clear();
        let summary = self.upload.encode(
            &self.scene,
            DrawableSize {
                width: 1_920,
                height: 1_080,
            },
            &self.rendering_parameters,
            true,
            BackdropBlurQuality::Full,
        );
        std::hint::black_box(self.upload.path_rasterization_vertices.as_slice());
        summary.path_vertex_count as usize
    }
}

#[cfg(feature = "bench")]
pub(crate) struct MeshPackingBenchmarkCore {
    vertices: Vec<crate::GpuMesh3dVertex>,
    indices: Vec<u32>,
    vertex_bytes: Vec<u8>,
    index_bytes: Vec<u8>,
    uses_u16: bool,
}

#[cfg(feature = "bench")]
impl MeshPackingBenchmarkCore {
    pub(crate) fn new(vertex_count: usize, uses_u16: bool) -> Self {
        let vertices = (0..vertex_count)
            .map(|index| {
                let value = index as f32;
                crate::GpuMesh3dVertex {
                    position: [value, value * 0.5, value * 0.25],
                    color: [
                        (index % 251) as f32 / 250.0,
                        (index % 127) as f32 / 126.0,
                        (index % 61) as f32 / 60.0,
                        (index % 8) as f32 * 0.25,
                    ],
                }
            })
            .collect();
        let index_count = vertex_count.saturating_mul(3);
        let indices = (0..index_count)
            .map(|index| (index % vertex_count) as u32)
            .collect();

        Self {
            vertices,
            indices,
            vertex_bytes: Vec::new(),
            index_bytes: Vec::new(),
            uses_u16,
        }
    }

    pub(crate) fn pack(&mut self) -> (usize, usize) {
        self.pack_with(write_custom_mesh_3d_indices)
    }

    pub(crate) fn pack_scalar(&mut self) -> (usize, usize) {
        self.pack_with(write_custom_mesh_3d_indices_scalar)
    }

    pub(crate) fn pack_simd(&mut self) -> (usize, usize) {
        self.pack_with(write_custom_mesh_3d_indices_simd)
    }

    fn pack_with(
        &mut self,
        pack_indices: fn(&mut Vec<u8>, &[u32], bool) -> Result<()>,
    ) -> (usize, usize) {
        self.vertex_bytes.clear();
        self.vertex_bytes.reserve(
            self.vertices
                .len()
                .saturating_mul(PACKED_CUSTOM_MESH_3D_VERTEX_BYTES),
        );
        for vertex in self.vertices.iter().copied() {
            write_custom_mesh_3d_vertex(&mut self.vertex_bytes, vertex);
        }

        self.index_bytes.clear();
        pack_indices(&mut self.index_bytes, &self.indices, self.uses_u16)
            .expect("benchmark mesh indices must fit their selected format");

        std::hint::black_box((self.vertex_bytes.as_slice(), self.index_bytes.as_slice()));
        (self.vertex_bytes.len(), self.index_bytes.len())
    }
}

#[cfg(test)]
mod tests;
