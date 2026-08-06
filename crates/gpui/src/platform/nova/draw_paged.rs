use super::*;

pub(super) use super::draw_legacy::{
    NovaBackdropBlurRenderPass, NovaDrawStepMode, apply_scissor_to_steps,
    backdrop_blur_render_passes_for_targets_into, partial_scissor_for_plan,
    path_mask_draw_steps_for_upload, path_mask_draw_steps_for_upload_into,
    scaled_pixels_ceil_u32, scaled_pixels_floor_u32,
};

pub(super) fn draw_steps_for_upload(
    upload: &NovaFrameUpload,
    pipelines: &NovaPipelines,
    blend_pipelines: NovaBlendPipelines,
    quad_resource_set: ResourceSetId,
    shadow_resource_set: ResourceSetId,
    path_resource_set: ResourceSetId,
    sprite_resource_set: impl FnMut(AtlasTextureId) -> Option<ResourceSetId>,
    custom_mesh_3d_pipeline: impl FnMut(GpuMesh3dShaderId) -> Option<RenderPipelineId>,
    custom_mesh_3d_cache_entry: impl FnMut(GpuMesh3dId, u64) -> Option<NovaMeshCacheEntry>,
    underline_resource_set: ResourceSetId,
    backdrop_blur_resource_set: ResourceSetId,
    custom_mesh_3d_resource_set: ResourceSetId,
    custom_mesh_3d_indices_buffer: BufferId,
    mode: NovaDrawStepMode,
) -> Vec<RenderStepDescriptor> {
    let mut steps = Vec::new();
    draw_steps_for_upload_into(
        upload,
        pipelines,
        blend_pipelines,
        quad_resource_set,
        shadow_resource_set,
        path_resource_set,
        sprite_resource_set,
        custom_mesh_3d_pipeline,
        custom_mesh_3d_cache_entry,
        underline_resource_set,
        backdrop_blur_resource_set,
        custom_mesh_3d_resource_set,
        custom_mesh_3d_indices_buffer,
        mode,
        &mut steps,
    );
    steps
}

pub(super) fn draw_steps_for_upload_into(
    upload: &NovaFrameUpload,
    pipelines: &NovaPipelines,
    blend_pipelines: NovaBlendPipelines,
    quad_resource_set: ResourceSetId,
    shadow_resource_set: ResourceSetId,
    path_resource_set: ResourceSetId,
    sprite_resource_set: impl FnMut(AtlasTextureId) -> Option<ResourceSetId>,
    custom_mesh_3d_pipeline: impl FnMut(GpuMesh3dShaderId) -> Option<RenderPipelineId>,
    mut custom_mesh_3d_cache_entry: impl FnMut(GpuMesh3dId, u64) -> Option<NovaMeshCacheEntry>,
    underline_resource_set: ResourceSetId,
    backdrop_blur_resource_set: ResourceSetId,
    custom_mesh_3d_resource_set: ResourceSetId,
    custom_mesh_3d_indices_buffer: BufferId,
    mode: NovaDrawStepMode,
    steps: &mut Vec<RenderStepDescriptor>,
) {
    let mut index_bindings = FxHashMap::<i32, (u64, IndexFormat)>::default();
    super::draw_legacy::draw_steps_for_upload_into(
        upload,
        pipelines,
        blend_pipelines,
        quad_resource_set,
        shadow_resource_set,
        path_resource_set,
        sprite_resource_set,
        custom_mesh_3d_pipeline,
        |mesh_id, generation| {
            let entry = custom_mesh_3d_cache_entry(mesh_id, generation)?;
            let base_vertex = i32::try_from(entry.vertex_offset).ok()?;
            index_bindings.insert(
                base_vertex,
                (
                    u64::from(custom_mesh_3d_index_byte_offset(entry)),
                    custom_mesh_3d_index_format(entry),
                ),
            );
            Some(NovaMeshCacheEntry {
                index_offset: 0,
                ..entry
            })
        },
        underline_resource_set,
        backdrop_blur_resource_set,
        custom_mesh_3d_resource_set,
        custom_mesh_3d_indices_buffer,
        mode,
        steps,
    );

    for step in steps {
        let RenderStepDescriptor::DrawIndexed(step) = step else {
            continue;
        };
        let Some((offset, format)) = index_bindings.get(&step.base_vertex).copied() else {
            continue;
        };
        step.index_buffer.offset = offset;
        step.index_buffer.format = format;
    }
}
