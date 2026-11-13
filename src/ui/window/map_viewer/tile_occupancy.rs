use super::model::*;
use super::prelude::*;

pub(super) struct LoadedIndex {
    pub(super) index: Arc<bedrock_render::cache::occupancy::Index>,
    pub(super) source: bedrock_render::cache::occupancy::LoadSource,
}

pub(super) fn load_index(
    world_path: PathBuf,
    dimension: Dimension,
    layout: RenderLayout,
    cancel: RenderTaskControl,
) -> Result<LoadedIndex, String> {
    let request = bedrock_render::cache::occupancy::Request::new(
        world_path,
        file_ops::cache_subdir("bedrock-render"),
        dimension,
        layout,
    );
    let result = bedrock_render::cache::occupancy::load_or_build_index(request, &cancel)
        .map_err(|error| format!("加载地图占用索引失败: {error}"))?;
    Ok(LoadedIndex {
        index: Arc::new(result.index),
        source: result.source,
    })
}

pub(super) fn materialize_chunks(
    index: &bedrock_render::cache::occupancy::Index,
    coord: (i32, i32),
) -> Option<TileChunkPositions> {
    index
        .chunk_positions(coord.0, coord.1)
        .map(|positions| Arc::<[ChunkPos]>::from(positions))
}

/// Occupancy 索引只负责判断区块是否存在，不应改变用户视口中心。
/// 地图首次打开由 level.dat 的 SpawnX/SpawnZ 决定中心；读取失败时保留默认 (0, 0)。
pub(super) const fn center_block(
    _index: &bedrock_render::cache::occupancy::Index,
) -> Option<(i32, i32)> {
    None
}
