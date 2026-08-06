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

/// Occupancy 索引只负责判断区块是否存在，不应改变用户视口中心。
/// 地图首次打开由 level.dat 的 SpawnX/SpawnZ 决定中心；读取失败时保留默认 (0, 0)。
pub(super) const fn occupancy_center_block(_index: &TileOccupancyIndex) -> Option<(i32, i32)> {
    None
}
