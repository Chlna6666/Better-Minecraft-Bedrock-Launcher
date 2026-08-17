//! Region overlay and map-analysis queries.

pub use super::implementation::{
    BlockEntityOverlay, EntityOverlay, HardcodedSpawnAreaOverlay, PendingTickOverlay,
    RegionOverlayQuery, RegionOverlayQueryOptions, VillageOverlay, VillageOverlayIndex,
    query_region_overlays_blocking, query_region_overlays_blocking_with_control,
};
