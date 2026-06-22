use super::helpers::*;
use super::model::*;
use super::prelude::*;

pub(super) fn render_image_from_decoded_tile(
    tile: DecodedTileImage,
) -> Result<
    (
        Arc<RenderImage>,
        Arc<[u8]>,
        TilePixelFormat,
        u32,
        u32,
        usize,
    ),
    String,
> {
    render_image_from_decoded_tile_parts(tile.width, tile.height, tile.pixel_format, tile.pixels)
}

pub(super) fn render_image_from_decoded_tile_parts(
    width: u32,
    height: u32,
    pixel_format: TilePixelFormat,
    pixels: Vec<u8>,
) -> Result<
    (
        Arc<RenderImage>,
        Arc<[u8]>,
        TilePixelFormat,
        u32,
        u32,
        usize,
    ),
    String,
> {
    let estimated_bytes = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .unwrap_or(pixels.len());
    let image_pixel_format = match pixel_format {
        TilePixelFormat::Rgba8 => gpui::RenderImagePixelFormat::Rgba8,
        TilePixelFormat::Bgra8 => gpui::RenderImagePixelFormat::Bgra8,
    };
    let pixels = Arc::<[u8]>::from(pixels);
    let image = RenderImage::from_raw_pixels(
        width,
        height,
        image_pixel_format,
        Vec::from(pixels.as_ref()),
    )
    .map_err(|error| format!("瓦片图像尺寸无效: {width}x{height}: {error}"))?;
    Ok((
        Arc::new(image),
        pixels,
        pixel_format,
        width,
        height,
        estimated_bytes,
    ))
}

pub(super) fn render_image_from_decoded_tile_shared(
    width: u32,
    height: u32,
    pixel_format: TilePixelFormat,
    pixels: Arc<[u8]>,
) -> Result<
    (
        Arc<RenderImage>,
        Arc<[u8]>,
        TilePixelFormat,
        u32,
        u32,
        usize,
    ),
    String,
> {
    let (image, _, pixel_format, width, height, estimated_bytes) =
        render_image_from_decoded_tile_parts(
            width,
            height,
            pixel_format,
            Vec::from(pixels.as_ref()),
        )?;
    Ok((image, pixels, pixel_format, width, height, estimated_bytes))
}

pub(super) fn merge_ui_cache_stats(
    stats: &mut RenderPipelineStats,
    cache_stats: &RenderPipelineStats,
) {
    stats.cache_hits = stats.cache_hits.saturating_add(cache_stats.cache_hits);
    stats.cache_misses = stats.cache_misses.saturating_add(cache_stats.cache_misses);
    stats.cache_memory_hits = stats
        .cache_memory_hits
        .saturating_add(cache_stats.cache_memory_hits);
    stats.cache_disk_fresh_hits = stats
        .cache_disk_fresh_hits
        .saturating_add(cache_stats.cache_disk_fresh_hits);
    stats.cache_disk_stale_hits = stats
        .cache_disk_stale_hits
        .saturating_add(cache_stats.cache_disk_stale_hits);
    stats.cache_empty_negative_hits = stats
        .cache_empty_negative_hits
        .saturating_add(cache_stats.cache_empty_negative_hits);
    stats.cache_probes = stats.cache_probes.saturating_add(cache_stats.cache_probes);
    stats.cache_validation_mismatches = stats
        .cache_validation_mismatches
        .saturating_add(cache_stats.cache_validation_mismatches);
    stats.cache_read_ms = stats
        .cache_read_ms
        .saturating_add(cache_stats.cache_read_ms);
    stats.cache_decode_ms = stats
        .cache_decode_ms
        .saturating_add(cache_stats.cache_decode_ms);
    stats.cache_first_ready_ms = stats
        .cache_first_ready_ms
        .saturating_add(cache_stats.cache_first_ready_ms);
}

pub(super) fn decoded_tile_byte_len(width: u32, height: u32) -> Result<usize, String> {
    let pixels = width
        .checked_mul(height)
        .ok_or_else(|| format!("decoded tile dimensions overflow: {width}x{height}"))?;
    let bytes = pixels
        .checked_mul(4)
        .ok_or_else(|| format!("decoded tile byte length overflow: {width}x{height}"))?;
    usize::try_from(bytes)
        .map_err(|_| format!("decoded tile byte length does not fit usize: {width}x{height}"))
}

#[cfg(test)]
pub(super) fn ui_decoded_tile_cache_key(
    world_path: &std::path::Path,
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
    mode: RenderMode,
    layout: RenderLayout,
    planned: &PlannedTile,
) -> TileCacheKey {
    let identity = decoded_cache_identity(world_path, render_backend, render_gpu_backend);
    ui_decoded_tile_cache_key_with_identity(&identity, mode, layout, planned)
}

pub(super) fn ui_decoded_tile_cache_key_with_identity(
    identity: &RenderCacheIdentity,
    mode: RenderMode,
    layout: RenderLayout,
    planned: &PlannedTile,
) -> TileCacheKey {
    TileCacheKey {
        world_id: identity.world_id.clone(),
        world_signature: identity.renderer_signature.clone(),
        renderer_version: RENDERER_CACHE_VERSION,
        palette_version: DEFAULT_PALETTE_VERSION,
        dimension: planned.job.coord.dimension,
        mode: render_mode_cache_slug(mode),
        chunks_per_tile: layout.chunks_per_tile,
        blocks_per_pixel: layout.blocks_per_pixel,
        pixels_per_block: layout.pixels_per_block,
        tile_x: planned.job.coord.x,
        tile_z: planned.job.coord.z,
        extension: UI_DECODED_TILE_CACHE_EXTENSION.to_string(),
    }
}

#[cfg(test)]
pub(super) fn ui_decoded_tile_cache_path(
    world_path: &std::path::Path,
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
    key: &TileCacheKey,
) -> PathBuf {
    let identity = decoded_cache_identity(world_path, render_backend, render_gpu_backend);
    ui_decoded_tile_cache_path_with_identity(&identity, key)
}

pub(super) fn ui_decoded_tile_cache_path_with_identity(
    identity: &RenderCacheIdentity,
    key: &TileCacheKey,
) -> PathBuf {
    let shard_x = key.tile_x.div_euclid(UI_DECODED_TILE_CACHE_SHARD_WIDTH);
    let shard_z = key.tile_z.div_euclid(UI_DECODED_TILE_CACHE_SHARD_WIDTH);
    file_ops::cache_subdir("bedrock-render")
        .join("ui-decoded-tiles")
        .join(&identity.world_id)
        .join(&identity.renderer_signature)
        .join(format!(
            "r{}-p{}",
            key.renderer_version, key.palette_version
        ))
        .join(format!("dimension-{}", key.dimension.id()))
        .join(&key.mode)
        .join(format!(
            "{}c-{}bpp-{}ppb",
            key.chunks_per_tile, key.blocks_per_pixel, key.pixels_per_block
        ))
        .join(format!("x{shard_x}-z{shard_z}"))
        .join(format!(
            "{}_{}.{}",
            key.tile_x, key.tile_z, UI_DECODED_TILE_CACHE_FILE_EXTENSION
        ))
}

pub(super) fn remove_ui_decoded_tile_cache_file_for_tile(
    world_path: &std::path::Path,
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
    mode: RenderMode,
    dimension: Dimension,
    layout: RenderLayout,
    coord: (i32, i32),
    reason: &'static str,
) {
    let identity = decoded_cache_identity(world_path, render_backend, render_gpu_backend);
    let key = TileCacheKey {
        world_id: identity.world_id.clone(),
        world_signature: identity.renderer_signature.clone(),
        renderer_version: RENDERER_CACHE_VERSION,
        palette_version: DEFAULT_PALETTE_VERSION,
        dimension,
        mode: render_mode_cache_slug(mode),
        chunks_per_tile: layout.chunks_per_tile,
        blocks_per_pixel: layout.blocks_per_pixel,
        pixels_per_block: layout.pixels_per_block,
        tile_x: coord.0,
        tile_z: coord.1,
        extension: UI_DECODED_TILE_CACHE_EXTENSION.to_string(),
    };
    let path = ui_decoded_tile_cache_path_with_identity(&identity, &key);
    remove_stale_ui_decoded_tile_cache_file(&path, reason);
}

pub(super) fn remove_ui_decoded_tile_cache_scope(
    world_path: &std::path::Path,
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
    mode: RenderMode,
    dimension: Dimension,
    layout: RenderLayout,
    reason: &'static str,
) {
    let identity = decoded_cache_identity(world_path, render_backend, render_gpu_backend);
    let path = file_ops::cache_subdir("bedrock-render")
        .join("ui-decoded-tiles")
        .join(&identity.world_id)
        .join(&identity.renderer_signature)
        .join(format!(
            "r{}-p{}",
            RENDERER_CACHE_VERSION, DEFAULT_PALETTE_VERSION
        ))
        .join(format!("dimension-{}", dimension.id()))
        .join(render_mode_cache_slug(mode))
        .join(format!(
            "{}c-{}bpp-{}ppb",
            layout.chunks_per_tile, layout.blocks_per_pixel, layout.pixels_per_block
        ));
    match std::fs::remove_dir_all(&path) {
        Ok(()) => tracing::debug!(
            path = %path.display(),
            reason,
            "map_viewer decoded_tile_cache_scope_removed"
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => tracing::debug!(
            path = %path.display(),
            reason,
            %error,
            "map_viewer decoded_tile_cache_scope_remove_failed"
        ),
    }
}

pub(super) fn decode_ui_decoded_tile_cache_header(
    bytes: &[u8],
) -> Result<UiDecodedTileCacheHeader, String> {
    if bytes.len() < UI_DECODED_TILE_CACHE_HEADER_LEN {
        return Err("decoded tile cache header is truncated".to_string());
    }
    if bytes[..UI_DECODED_TILE_CACHE_MAGIC.len()] != UI_DECODED_TILE_CACHE_MAGIC {
        return Err("decoded tile cache magic mismatch".to_string());
    }
    let version = read_cache_u32(bytes, 8)?;
    if version != UI_DECODED_TILE_CACHE_VERSION {
        return Err(format!("unsupported decoded tile cache version {version}"));
    }
    let header = UiDecodedTileCacheHeader {
        width: read_cache_u32(bytes, 12)?,
        height: read_cache_u32(bytes, 16)?,
        pixel_format: read_cache_u32(bytes, 20)?,
        flags: read_cache_u32(bytes, 24)?,
        validation_kind: read_cache_u32(bytes, 28)?,
        validation_value: read_cache_u64(bytes, 32)?,
        raw_len: read_cache_u64(bytes, 40)?,
    };
    validate_ui_decoded_tile_cache_header(header)?;
    Ok(header)
}

pub(super) fn encode_ui_decoded_tile_cache_header(
    header: UiDecodedTileCacheHeader,
) -> Result<Vec<u8>, String> {
    validate_ui_decoded_tile_cache_header(header)?;
    let mut bytes = Vec::with_capacity(UI_DECODED_TILE_CACHE_HEADER_LEN);
    bytes.extend_from_slice(&UI_DECODED_TILE_CACHE_MAGIC);
    bytes.extend_from_slice(&UI_DECODED_TILE_CACHE_VERSION.to_le_bytes());
    bytes.extend_from_slice(&header.width.to_le_bytes());
    bytes.extend_from_slice(&header.height.to_le_bytes());
    bytes.extend_from_slice(&header.pixel_format.to_le_bytes());
    bytes.extend_from_slice(&header.flags.to_le_bytes());
    bytes.extend_from_slice(&header.validation_kind.to_le_bytes());
    bytes.extend_from_slice(&header.validation_value.to_le_bytes());
    bytes.extend_from_slice(&header.raw_len.to_le_bytes());
    Ok(bytes)
}

pub(super) fn validate_ui_decoded_tile_cache_header(
    header: UiDecodedTileCacheHeader,
) -> Result<(), String> {
    if header.pixel_format != UI_DECODED_TILE_CACHE_PIXEL_FORMAT_RGBA8 {
        return Err(format!(
            "unsupported decoded tile pixel format {}",
            header.pixel_format
        ));
    }
    if header.validation_kind != UI_DECODED_TILE_CACHE_VALIDATION_KIND_SIMPLE_TILE {
        return Err(format!(
            "unsupported decoded tile validation kind {}",
            header.validation_kind
        ));
    }
    if header.flags & !UI_DECODED_TILE_CACHE_KNOWN_FLAGS != 0 {
        return Err(format!(
            "unsupported decoded tile flags {:#x}",
            header.flags
        ));
    }
    if header.is_non_empty() == header.is_empty_negative() {
        return Err("decoded tile cache must be exactly one of non-empty or empty".to_string());
    }
    let expected_len = decoded_tile_byte_len(header.width, header.height)?;
    if header.raw_len
        != u64::try_from(expected_len)
            .map_err(|_| "decoded tile byte length is too large".to_string())?
    {
        return Err(format!(
            "decoded tile raw length mismatch: expected {expected_len}, got {}",
            header.raw_len
        ));
    }
    Ok(())
}

pub(super) fn read_cache_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let end = offset.saturating_add(4);
    let slice = bytes
        .get(offset..end)
        .ok_or_else(|| format!("decoded tile cache u32 at {offset} is truncated"))?;
    let mut array = [0_u8; 4];
    array.copy_from_slice(slice);
    Ok(u32::from_le_bytes(array))
}

pub(super) fn read_cache_u64(bytes: &[u8], offset: usize) -> Result<u64, String> {
    let end = offset.saturating_add(8);
    let slice = bytes
        .get(offset..end)
        .ok_or_else(|| format!("decoded tile cache u64 at {offset} is truncated"))?;
    let mut array = [0_u8; 8];
    array.copy_from_slice(slice);
    Ok(u64::from_le_bytes(array))
}

pub(super) fn remove_stale_ui_decoded_tile_cache_file(
    path: &std::path::Path,
    reason: &'static str,
) {
    match std::fs::remove_file(path) {
        Ok(()) => tracing::debug!(
            path = %path.display(),
            reason,
            "map_viewer decoded_tile_cache_stale_removed"
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => tracing::debug!(
            path = %path.display(),
            reason,
            %error,
            "map_viewer decoded_tile_cache_stale_remove_failed"
        ),
    }
}

pub(super) fn shared_ui_decoded_tile_cache_writer()
-> &'static std_mpsc::SyncSender<UiDecodedTileCacheWrite> {
    static WRITER: OnceLock<std_mpsc::SyncSender<UiDecodedTileCacheWrite>> = OnceLock::new();
    WRITER.get_or_init(spawn_ui_decoded_tile_cache_writer)
}

fn spawn_ui_decoded_tile_cache_writer() -> std_mpsc::SyncSender<UiDecodedTileCacheWrite> {
    let (sender, receiver) = std_mpsc::sync_channel::<UiDecodedTileCacheWrite>(64);
    thread::spawn(move || {
        for write in receiver {
            if let Err(error) = write_ui_decoded_tile_cache_entry(write) {
                tracing::debug!(%error, "map_viewer decoded_tile_cache_write_failed");
            }
        }
    });
    sender
}

#[allow(clippy::too_many_arguments)]
pub(super) fn queue_ui_decoded_tile_cache_write_for_ready_tile_with_identity(
    identity: &RenderCacheIdentity,
    mode: RenderMode,
    layout: RenderLayout,
    validation_seed: u64,
    planned: &PlannedTile,
    width: u32,
    height: u32,
    pixel_format: TilePixelFormat,
    pixels: Arc<[u8]>,
) {
    if validation_seed == 0 || pixel_format != TilePixelFormat::Rgba8 {
        return;
    }
    let Some(chunk_positions) = planned.chunk_positions.as_deref() else {
        return;
    };
    if chunk_positions.is_empty() || pixels.chunks_exact(4).all(|pixel| pixel[3] == 0) {
        return;
    }
    let raw_len = match decoded_tile_byte_len(width, height)
        .and_then(|len| u64::try_from(len).map_err(|_| "decoded tile too large".to_string()))
    {
        Ok(raw_len) => raw_len,
        Err(error) => {
            tracing::debug!(%error, "map_viewer decoded_tile_cache_write_size_rejected");
            return;
        }
    };
    let key = ui_decoded_tile_cache_key_with_identity(identity, mode, layout, planned);
    let validation_value = bedrock_render::tile_cache_validation_value(
        &key,
        &planned.region,
        chunk_positions,
        validation_seed,
    );
    let write = UiDecodedTileCacheWrite {
        path: ui_decoded_tile_cache_path_with_identity(identity, &key),
        header: UiDecodedTileCacheHeader {
            width,
            height,
            pixel_format: UI_DECODED_TILE_CACHE_PIXEL_FORMAT_RGBA8,
            flags: UI_DECODED_TILE_CACHE_FLAG_NON_EMPTY,
            validation_kind: UI_DECODED_TILE_CACHE_VALIDATION_KIND_SIMPLE_TILE,
            validation_value,
            raw_len,
        },
        pixels: Some(pixels),
    };
    match shared_ui_decoded_tile_cache_writer().try_send(write) {
        Ok(()) => {}
        Err(std_mpsc::TrySendError::Full(_)) => {
            tracing::debug!("map_viewer decoded_tile_cache_write_queue_full");
        }
        Err(std_mpsc::TrySendError::Disconnected(_)) => {
            tracing::debug!("map_viewer decoded_tile_cache_writer_closed");
        }
    }
}

pub(super) fn write_ui_decoded_tile_cache_entry(
    write: UiDecodedTileCacheWrite,
) -> Result<(), String> {
    let mut bytes = encode_ui_decoded_tile_cache_header(write.header)?;
    if let Some(pixels) = write.pixels {
        let expected_len = usize::try_from(write.header.raw_len)
            .map_err(|_| "decoded tile cache raw length does not fit usize".to_string())?;
        if pixels.len() != expected_len {
            return Err(format!(
                "decoded tile cache write length mismatch: expected {expected_len}, got {}",
                pixels.len()
            ));
        }
        let compressed = zstd::bulk::compress(&pixels, UI_DECODED_TILE_CACHE_ZSTD_LEVEL)
            .map_err(|error| format!("压缩地图瓦片缓存失败: {error}"))?;
        bytes.extend_from_slice(&compressed);
    }
    write_ui_decoded_tile_cache_bytes(&write.path, &bytes)
}

pub(super) fn write_ui_decoded_tile_cache_bytes(
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建地图瓦片缓存目录失败: {} ({error})", parent.display()))?;
    }
    let temp_path = path.with_extension("rgbatile.tmp");
    {
        let mut file = std::fs::File::create(&temp_path)
            .map_err(|error| format!("写入地图瓦片缓存失败: {} ({error})", temp_path.display()))?;
        file.write_all(bytes)
            .map_err(|error| format!("写入地图瓦片缓存失败: {} ({error})", temp_path.display()))?;
        file.flush()
            .map_err(|error| format!("刷新地图瓦片缓存失败: {} ({error})", temp_path.display()))?;
    }
    match std::fs::rename(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            std::fs::remove_file(path).map_err(|remove_error| {
                format!("替换地图瓦片缓存失败: {} ({remove_error})", path.display())
            })?;
            std::fs::rename(&temp_path, path).map_err(|rename_error| {
                format!("替换地图瓦片缓存失败: {} ({rename_error})", path.display())
            })
        }
        Err(error) => Err(format!(
            "替换地图瓦片缓存失败: {} ({error})",
            path.display()
        )),
    }
}
