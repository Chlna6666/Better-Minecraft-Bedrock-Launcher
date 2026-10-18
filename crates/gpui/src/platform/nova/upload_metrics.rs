use super::*;

/// Upload exactly the writes used by this frame's damage plan. The snapshot
/// includes page metadata, so later atlas allocation cannot change this batch.
/// An error before consumption automatically restores its queued writes.
pub(super) fn upload_pending_atlas<D>(
    batch: frame_upload::upload_queue::AtlasUpload<'_>,
    device: &mut D,
    gpu_textures: &mut FxHashMap<AtlasTextureId, NovaGpuAtlasTexture>,
    backend_name: &str,
    descriptor: &AtlasResourceDescriptor,
) -> Result<AtlasUploadStats>
where
    D: BackendResources,
{
    let started_at = Instant::now();
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
        let new_texture = create_atlas_texture_resources(
            device,
            backend_name,
            info.id,
            info.size,
            descriptor,
            NovaAtlasResourceSetMode::UsedByTextureKind,
        )?;
        if let Some(replaced) = gpu_textures.insert(info.id, new_texture) {
            destroy_gpu_atlas_texture(device, replaced, backend_name, info.id);
        }
    }

    let stats = batch.upload(
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
