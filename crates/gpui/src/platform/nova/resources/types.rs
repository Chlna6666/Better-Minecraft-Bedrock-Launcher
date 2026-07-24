use super::super::*;
use super::buffers::NovaFrameResourceBuffers;
use super::resource_sets::NovaFrameResourceSets;

#[derive(Clone, Copy)]
pub(in crate::platform::nova) struct NovaFrameResources {
    pub(in crate::platform::nova) buffers: NovaFrameResourceBuffers,
    pub(in crate::platform::nova) resource_sets: NovaFrameResourceSets,
    pub(in crate::platform::nova) path_resource_set: ResourceSetId,
    pub(in crate::platform::nova) present_cache_resource_set: ResourceSetId,
    pub(in crate::platform::nova) mono_sprite_resource_set: ResourceSetId,
    pub(in crate::platform::nova) poly_sprite_resource_set: ResourceSetId,
}

pub(in crate::platform::nova) struct NovaRendererResources {
    pub(in crate::platform::nova) render_pass: RenderPassId,
    pub(in crate::platform::nova) pipelines: NovaPipelines,
    pub(in crate::platform::nova) depth_texture: TextureId,
    pub(in crate::platform::nova) depth_texture_view: TextureViewId,
    pub(in crate::platform::nova) frame_resources: Vec<NovaFrameResources>,
    pub(in crate::platform::nova) custom_mesh_3d_vertices_buffer: BufferId,
    pub(in crate::platform::nova) custom_mesh_3d_indices_buffer: BufferId,
    pub(in crate::platform::nova) path_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) mono_sprite_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) poly_sprite_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) backdrop_blur_pass_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) backdrop_blur_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) custom_mesh_3d_pipeline_layout: PipelineLayoutId,
    pub(in crate::platform::nova) backdrop_blur_targets: Option<NovaBackdropBlurTargets>,
    pub(in crate::platform::nova) atlas_texture: TextureId,
    pub(in crate::platform::nova) atlas_texture_view: TextureViewId,
    pub(in crate::platform::nova) atlas_sampler: SamplerId,
    pub(in crate::platform::nova) path_texture: TextureId,
    pub(in crate::platform::nova) path_texture_view: TextureViewId,
    pub(in crate::platform::nova) present_cache_texture: TextureId,
    pub(in crate::platform::nova) present_cache_texture_view: TextureViewId,
}
