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
        Ok(gfx_core::ResourceDevice::write_texture_batch(
            device,
            writes.iter().copied(),
        )?)
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
    atlas_stats: AtlasUploadStats,
) {
    let upload_bytes = frame_upload_bytes;
    crate::diagnostics::performance_metrics::record_upload_bytes(
        upload_bytes.saturating_add(atlas_stats.uploaded_bytes),
    );
    crate::diagnostics::performance_metrics::record_upload_arena_metrics(
        upload_bytes,
        atlas_stats.arena_capacity,
        upload_bytes,
        atlas_stats.arena_used_bytes,
    );
}
