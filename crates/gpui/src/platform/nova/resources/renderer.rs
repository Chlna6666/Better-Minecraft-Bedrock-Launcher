use super::super::*;
use super::FrameResources;

pub(in crate::platform::nova) struct RendererResources {
    pub(in crate::platform::nova) render_pass: RenderPassId,
    pub(in crate::platform::nova) pipelines: Pipelines,
    pub(in crate::platform::nova) depth_texture: TextureId,
    pub(in crate::platform::nova) depth_texture_view: TextureViewId,
    pub(in crate::platform::nova) frame_resources: Vec<FrameResources>,
    pub(in crate::platform::nova) custom_mesh_3d_vertices_buffer: BufferId,
    pub(in crate::platform::nova) custom_mesh_3d_indices_buffer: BufferId,
    pub(in crate::platform::nova) custom_mesh_3d_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) path_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) mono_sprite_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) poly_sprite_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) backdrop_blur_pass_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) backdrop_blur_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) custom_mesh_3d_pipeline_layout: PipelineLayoutId,
    pub(in crate::platform::nova) backdrop_blur_targets: Option<BackdropBlurTargets>,
    pub(in crate::platform::nova) atlas_texture: TextureId,
    pub(in crate::platform::nova) atlas_texture_view: TextureViewId,
    pub(in crate::platform::nova) atlas_sampler: SamplerId,
    pub(in crate::platform::nova) path_texture: TextureId,
    pub(in crate::platform::nova) path_texture_view: TextureViewId,
}
