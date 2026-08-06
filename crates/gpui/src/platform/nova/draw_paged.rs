use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

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

    for step in steps.iter_mut() {
        let RenderStepDescriptor::DrawIndexed(step) = step else {
            continue;
        };
        let Some((offset, format)) = index_bindings.get(&step.base_vertex).copied() else {
            continue;
        };
        step.index_buffer.offset = offset;
        step.index_buffer.format = format;
    }

    if mode == NovaDrawStepMode::Present {
        record_custom_mesh_3d_draw_profile(steps);
    }
}

fn record_custom_mesh_3d_draw_profile(steps: &[RenderStepDescriptor]) {
    static PROFILE_FRAME: AtomicU64 = AtomicU64::new(0);

    let frame = PROFILE_FRAME.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    let mut indexed_draws = 0usize;
    let mut uint16_draws = 0usize;
    let mut uint32_draws = 0usize;
    let mut submitted_indices = 0u64;
    let mut submitted_instances = 0u64;
    let mut pipelines = FxHashSet::default();
    let mut index_pages = FxHashSet::default();

    for step in steps {
        let RenderStepDescriptor::DrawIndexed(step) = step else {
            continue;
        };
        indexed_draws = indexed_draws.saturating_add(1);
        submitted_indices = submitted_indices
            .saturating_add(u64::from(step.index_count).saturating_mul(u64::from(step.instance_count)));
        submitted_instances = submitted_instances.saturating_add(u64::from(step.instance_count));
        pipelines.insert(step.pipeline);
        index_pages.insert((
            step.index_buffer.offset,
            matches!(step.index_buffer.format, IndexFormat::Uint16),
        ));
        match step.index_buffer.format {
            IndexFormat::Uint16 => uint16_draws = uint16_draws.saturating_add(1),
            IndexFormat::Uint32 => uint32_draws = uint32_draws.saturating_add(1),
        }
    }

    if indexed_draws > 0 && (frame == 1 || frame % 120 == 0) {
        tracing::debug!(
            frame,
            indexed_draws,
            uint16_draws,
            uint32_draws,
            submitted_indices,
            submitted_instances,
            pipeline_count = pipelines.len(),
            index_page_count = index_pages.len(),
            native_multi_draw_indirect = false,
            strategy = "paged_index_binding+material_sort_ready",
            "nova custom 3D draw profile"
        );
    }
}
