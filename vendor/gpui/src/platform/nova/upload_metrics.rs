use super::*;

pub(super) fn upload_pending_atlas<D>(
    atlas: &NovaAtlas,
    device: &mut D,
    resolve_texture: impl FnMut(AtlasTextureId) -> Result<TextureId>,
) -> Result<AtlasUploadStats>
where
    D: BackendResources,
{
    let started_at = Instant::now();
    let stats = atlas.upload_pending_rgba_pixels(resolve_texture, |writes| {
        Ok(device.write_texture_batch(writes.iter().copied())?)
    })?;
    if stats.upload_count > 0 {
        crate::diagnostics::performance_metrics::record_atlas_upload_metrics(
            stats.uploaded_bytes,
            stats.upload_count,
            started_at.elapsed(),
        );
    }
    Ok(stats)
}

pub(super) fn record_nova_upload_metrics(
    frame_upload_bytes: usize,
    mesh_upload_bytes: usize,
    mesh_retained_bytes: usize,
    mesh_buffer_count: usize,
    atlas_stats: AtlasUploadStats,
) {
    let atlas_texture_bytes =
        NOVA_ATLAS_SIZE as usize * NOVA_ATLAS_SIZE as usize * NOVA_ATLAS_BYTES_PER_PIXEL;
    let upload_bytes = frame_upload_bytes.saturating_add(mesh_upload_bytes);
    crate::diagnostics::performance_metrics::record_upload_bytes(
        upload_bytes.saturating_add(atlas_stats.uploaded_bytes),
    );
    crate::diagnostics::performance_metrics::record_upload_arena_metrics(
        upload_bytes,
        atlas_stats.arena_capacity,
        upload_bytes,
        atlas_stats.arena_used_bytes,
    );
    crate::diagnostics::performance_metrics::record_gpu_resource_breakdown(
        atlas_texture_bytes,
        false,
        false,
        false,
        false,
        0,
        mesh_buffer_count,
    );
    crate::diagnostics::performance_metrics::record_gpu_retained_bytes(
        atlas_texture_bytes
            .saturating_add(frame_upload_bytes)
            .saturating_add(mesh_retained_bytes),
    );
}
