use super::*;

pub(in crate::platform::nova) const MAX_PATH_RASTERIZATION_CACHE_ENTRIES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::platform::nova) struct PathRasterizationCacheKey {
    pub(in crate::platform::nova) path_id: crate::PathCacheId,
    pub(in crate::platform::nova) generation: crate::PathGeometryGeneration,
    pub(in crate::platform::nova) vertex_count: usize,
    pub(in crate::platform::nova) geometry_hash: u64,
    pub(in crate::platform::nova) paint_hash: u64,
}

#[derive(Clone, Debug)]
pub(in crate::platform::nova) struct PathRasterizationCacheEntry {
    pub(in crate::platform::nova) bytes: Arc<[u8]>,
    pub(in crate::platform::nova) vertex_count: u32,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::platform::nova) struct PathGeometryHashMemo {
    pub(in crate::platform::nova) generation: crate::PathGeometryGeneration,
    pub(in crate::platform::nova) vertex_count: usize,
    pub(in crate::platform::nova) first_xy_bits: (u32, u32),
    pub(in crate::platform::nova) last_xy_bits: (u32, u32),
    pub(in crate::platform::nova) geometry_hash: u64,
}
