use once_cell::sync::Lazy;
use std::sync::Mutex;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MapViewerMemorySnapshot {
    pub tile_bytes: usize,
    pub tile_count: usize,
    pub canvas_snapshot_bytes: usize,
    pub canvas_snapshot_tile_count: usize,
    pub paste_preview_bytes: usize,
    pub paste_preview_count: usize,
    pub copied_import_preview_bytes: usize,
    pub copied_import_preview_count: usize,
    pub preview_3d_mesh_bytes: usize,
    pub preview_3d_surface_bytes: usize,
    pub preview_3d_chunk_mesh_count: usize,
    pub preview_3d_vertex_count: usize,
    pub preview_3d_render_in_flight: bool,
}

impl MapViewerMemorySnapshot {
    pub fn total_estimated_bytes(&self) -> usize {
        self.tile_bytes
            .saturating_add(self.canvas_snapshot_bytes)
            .saturating_add(self.paste_preview_bytes)
            .saturating_add(self.copied_import_preview_bytes)
            .saturating_add(self.preview_3d_mesh_bytes)
            .saturating_add(self.preview_3d_surface_bytes)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BmcblMemorySnapshot {
    pub map_viewer: MapViewerMemorySnapshot,
}

impl BmcblMemorySnapshot {
    pub fn total_estimated_bytes(&self) -> usize {
        self.map_viewer.total_estimated_bytes()
    }
}

static BMCBL_MEMORY_SNAPSHOT: Lazy<Mutex<BmcblMemorySnapshot>> =
    Lazy::new(|| Mutex::new(BmcblMemorySnapshot::default()));

pub fn record_map_viewer_memory(snapshot: MapViewerMemorySnapshot) {
    let mut state = BMCBL_MEMORY_SNAPSHOT
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    state.map_viewer = snapshot;
}

pub fn clear_map_viewer_memory() {
    record_map_viewer_memory(MapViewerMemorySnapshot::default());
}

pub fn snapshot_bmcbl_memory() -> BmcblMemorySnapshot {
    BMCBL_MEMORY_SNAPSHOT
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}
