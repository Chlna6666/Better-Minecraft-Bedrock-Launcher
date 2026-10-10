use super::*;

/// Drain pending writes together with their page metadata. GPU texture pages
/// may be allocated by another thread *after* the renderer's regular texture
/// synchronization but *before* its upload phase. Preparing from the drained
/// batch closes that race without holding the atlas mutex during GPU work.
pub(super) fn upload_pending_atlas<D>(
    atlas: &NovaAtlas,
    device: &mut D,
    gpu_textures: &mut FxHashMap<AtlasTextureId, NovaGpuAtlasTexture>,
    backend_name: &str,
    descriptor: &AtlasResourceDescriptor,
) -> Result<AtlasUploadStats>
where
    D: BackendResources,
{
    let started_at = Instant::now();
    let batch = atlas.take_pending_uploads();
    let missing_pages: Vec<_> = batch
        .texture_infos()
        .iter()
        .copied()
        .filter(|info| {
            !gpu_textures
                .get(&info.id)
                .is_some_and(|texture| texture.size == info.size)
        })
        .collect();

    for info in missing_pages {
        let new_texture = match create_atlas_texture_resources(
            device,
            backend_name,
            info.id,
            info.size,
            descriptor,
            NovaAtlasResourceSetMode::UsedByTextureKind,
        ) {
            Ok(texture) => texture,
            Err(error) => {
                atlas.restore_upload_batch(batch);
                return Err(error);
            }
        };
        if let Some(replaced) = gpu_textures.insert(info.id, new_texture) {
            destroy_gpu_atlas_texture(device, replaced, backend_name, info.id);
        }
    }

    let stats = atlas.upload_taken_rgba_pixels(
        batch,
        |atlas_id| {
            gpu_textures
                .get(&atlas_id)
                .map(|texture| texture.texture)
                .ok_or_else(|| anyhow::anyhow!(
                    "missing nova atlas texture {:?}/{} in atomic upload snapshot",
                    atlas_id.kind,
                    atlas_id.index,
                ))
        },
        |writes| Ok(gfx_core::ResourceDevice::write_texture_batch(device, writes.iter().copied())?),
    )?;
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
