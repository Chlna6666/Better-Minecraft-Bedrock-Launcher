use super::model::*;
use super::prelude::*;

pub(super) struct LoadedTileOccupancy {
    pub(super) index: Arc<TileOccupancyIndex>,
    pub(super) source: TileOccupancyIndexSource,
}

pub(super) fn load_tile_occupancy_index(
    world_path: PathBuf,
    dimension: Dimension,
    layout: RenderLayout,
    cancel: RenderTaskControl,
) -> Result<LoadedTileOccupancy, String> {
    let request = TileOccupancyIndexRequest::new(
        world_path,
        file_ops::cache_subdir("bedrock-render"),
        dimension,
        layout,
    );
    let result = load_or_build_tile_occupancy_index_blocking(request, &cancel)
        .map_err(|error| format!("加载地图占用索引失败: {error}"))?;
    Ok(LoadedTileOccupancy {
        index: Arc::new(result.index),
        source: result.source,
    })
}

pub(super) fn materialize_occupancy_chunks(
    index: &TileOccupancyIndex,
    coord: (i32, i32),
) -> Option<TileChunkPositions> {
    index
        .chunk_positions(coord.0, coord.1)
        .map(|positions| Arc::<[ChunkPos]>::from(positions))
}

pub(super) fn occupancy_center_block(index: &TileOccupancyIndex) -> Option<(i32, i32)> {
    let bounds = index.bounds()?;
    let center_chunk_x = bounds
        .min_chunk_x
        .saturating_add(bounds.max_chunk_x)
        .div_euclid(2);
    let center_chunk_z = bounds
        .min_chunk_z
        .saturating_add(bounds.max_chunk_z)
        .div_euclid(2);
    Some((
        center_chunk_x.saturating_mul(16).saturating_add(8),
        center_chunk_z.saturating_mul(16).saturating_add(8),
    ))
}
