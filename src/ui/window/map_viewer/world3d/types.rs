//! Shared state types for the independent 3D world editor.

use bedrock_render::{ChunkPos, Dimension};
use bedrock_world::block::BlockState;
use bedrock_world::chunk::{SubChunk, SubChunkFormat};
use bedrock_world::surface::ChunkLoadStats;
use gpui_3d::{Material, Mesh, TextureAsset};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Default horizontal streaming radius in chunks.
pub const WORLD3D_DEFAULT_VIEW_RADIUS: i32 = 8;
/// Largest accepted streaming radius in chunks.
pub const WORLD3D_MAX_VIEW_RADIUS: i32 = 256;
/// Vertical camera field of view used by the world editor.
pub const WORLD3D_DEFAULT_FOV_Y_RADIANS: f32 = 70.0_f32.to_radians();
/// Default camera far-plane distance in blocks.
pub const WORLD3D_DEFAULT_VIEW_DISTANCE: f32 = 320.0;
/// Near plane for the world camera.
pub const WORLD3D_NEAR_PLANE: f32 = 0.05;
/// Camera eye offset above a resolved safe surface block.
pub const WORLD3D_SURFACE_EYE_OFFSET: f64 = 2.2;
/// Camera movement speed in blocks per second at the default wheel setting.
pub const WORLD3D_DEFAULT_SPEED: f32 = 14.0;
/// Largest number of blocks a single 3D edit request may touch.
pub const WORLD3D_MAX_EDIT_BLOCKS: usize = 262_144;

/// Canonical stable identity of one world document.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct World3dWorldKey(pub String);

impl World3dWorldKey {
    /// Builds the canonical key for a world folder. Case differences from Windows
    /// path spelling are folded so two map windows for the same folder share one editor.
    #[must_use]
    pub fn from_path(path: &std::path::Path) -> Self {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mut text = canonical.to_string_lossy().replace('\\', "/");
        if cfg!(windows) {
            text = text.to_ascii_lowercase();
        }
        Self(text)
    }
}

/// Vertical placement strategy for a 2D → 3D teleport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeleportHeightMode {
    /// Resolve the highest safe surface at the target column asynchronously.
    SurfaceSafe,
    /// Use the map viewer's current Y layer.
    CurrentMapLayer,
    /// Use an explicit camera Y supplied by the caller.
    Exact,
}

impl TeleportHeightMode {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SurfaceSafe => "地表",
            Self::CurrentMapLayer => "当前 Y 层",
            Self::Exact => "精确 Y",
        }
    }
}

/// One navigation command from the 2D map (or an explicit 3D request).
#[derive(Clone, Debug)]
pub struct WorldNavigationTarget {
    pub world_key: World3dWorldKey,
    pub world_path: std::path::PathBuf,
    pub world_name: String,
    pub dimension: Dimension,
    pub block_x: i32,
    pub block_z: i32,
    pub height_mode: TeleportHeightMode,
    pub requested_y: Option<f64>,
    pub activate: bool,
    pub request_generation: u64,
}

/// How much of the 3D camera is projected onto the 2D tile map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CameraOverlayMode {
    /// No overlay at all.
    #[default]
    Hidden,
    /// Camera icon and facing arrow only.
    Position,
    /// Camera icon, facing arrow and horizontal field-of-view sector.
    PositionFrustum,
    /// The frustum variant capped by the camera view distance.
    PositionFrustumDistance,
}

impl CameraOverlayMode {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hidden => "隐藏",
            Self::Position => "位置+箭头",
            Self::PositionFrustum => "位置+视角",
            Self::PositionFrustumDistance => "包含视距",
        }
    }

    #[must_use]
    pub const fn shows_frustum(self) -> bool {
        matches!(self, Self::PositionFrustum | Self::PositionFrustumDistance)
    }
}

/// Camera projection family mirrored into the 2D overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum World3dProjectionKind {
    #[default]
    Perspective,
    Orthographic,
}

/// Read-only camera pose broadcast to the owning 2D map window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct World3dCameraSnapshot {
    pub dimension: Dimension,
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub vertical_fov_radians: f32,
    pub aspect: f32,
    pub view_distance: f32,
    pub projection: World3dProjectionKind,
    pub ortho_height: f32,
    pub generation: u64,
}

impl World3dCameraSnapshot {
    /// Horizontal field of view derived from the vertical FOV and the real viewport aspect.
    #[must_use]
    pub fn horizontal_fov_radians(&self) -> f32 {
        let aspect = self.aspect.clamp(0.05, 20.0);
        (2.0 * ((self.vertical_fov_radians * 0.5).tan() * aspect).atan())
            .clamp(0.01, std::f32::consts::PI - 0.01)
    }

    /// World-space forward vector (yaw/pitch convention shared with the map viewer).
    #[must_use]
    pub fn forward(&self) -> [f64; 3] {
        let (sin_yaw, cos_yaw) = (self.yaw.sin() as f64, self.yaw.cos() as f64);
        let (sin_pitch, cos_pitch) = (self.pitch.sin() as f64, self.pitch.cos() as f64);
        [-sin_yaw * cos_pitch, -sin_pitch, -cos_yaw * cos_pitch]
    }

    /// Horizontal forward direction used by the 2D facing arrow.
    #[must_use]
    pub fn forward_xz(&self) -> [f32; 2] {
        let yaw = self.yaw;
        [-yaw.sin(), -yaw.cos()]
    }
}

/// A normalized block volume shared between the 2D selection and the 3D editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct World3dSelection {
    pub dimension: Dimension,
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl World3dSelection {
    #[must_use]
    pub fn from_corners(dimension: Dimension, a: [i32; 3], b: [i32; 3]) -> Self {
        Self {
            dimension,
            min: [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])],
            max: [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])],
        }
    }

    #[must_use]
    pub fn volume(&self) -> u64 {
        let size = [
            u64::try_from(self.max[0].saturating_sub(self.min[0]).saturating_add(1)).unwrap_or(1),
            u64::try_from(self.max[1].saturating_sub(self.min[1]).saturating_add(1)).unwrap_or(1),
            u64::try_from(self.max[2].saturating_sub(self.min[2]).saturating_add(1)).unwrap_or(1),
        ];
        size[0].saturating_mul(size[1]).saturating_mul(size[2])
    }

    #[must_use]
    pub fn size(&self) -> [i32; 3] {
        [
            self.max[0].saturating_sub(self.min[0]).saturating_add(1),
            self.max[1].saturating_sub(self.min[1]).saturating_add(1),
            self.max[2].saturating_sub(self.min[2]).saturating_add(1),
        ]
    }

    #[must_use]
    pub fn contains(&self, p: [i32; 3]) -> bool {
        (0..3).all(|axis| p[axis] >= self.min[axis] && p[axis] <= self.max[axis])
    }

    #[must_use]
    pub fn chunk_positions(&self) -> Vec<ChunkPos> {
        let mut chunks = Vec::new();
        for z in self.min[2].div_euclid(16)..=self.max[2].div_euclid(16) {
            for x in self.min[0].div_euclid(16)..=self.max[0].div_euclid(16) {
                chunks.push(ChunkPos {
                    x,
                    z,
                    dimension: self.dimension,
                });
            }
        }
        chunks
    }
}

/// A raycast result against real world blocks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct World3dTargetBlock {
    pub dimension: Dimension,
    pub position: [i32; 3],
    pub normal: [i32; 3],
    pub block_name: String,
    pub chunk: ChunkPos,
    pub loaded: bool,
    pub fluid: bool,
}

impl World3dTargetBlock {
    #[must_use]
    pub fn face_adjacent(&self) -> [i32; 3] {
        [
            self.position[0] + self.normal[0],
            self.position[1] + self.normal[1],
            self.position[2] + self.normal[2],
        ]
    }
}

/// One independently uploaded height section of a streamed chunk.
#[derive(Clone, Debug)]
pub struct World3dChunkMesh {
    pub occluders: Arc<[gpui_3d::Occluder]>,
    pub pos: ChunkPos,
    pub section_y: i32,
    pub origin: [i32; 3],
    pub mesh: Arc<Mesh>,
    pub materials: Arc<[Arc<Material>]>,
    pub textures: Arc<[Arc<TextureAsset>]>,
    pub triangles: usize,
    pub faces: usize,
    pub missing_texture_faces: usize,
}

impl World3dChunkMesh {
    #[must_use]
    pub fn estimated_cpu_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(std::mem::size_of_val(self.occluders.as_ref()))
            .saturating_add(self.triangles.saturating_mul(3 * 32))
            .saturating_add(
                self.textures
                    .iter()
                    .map(|texture| {
                        usize::try_from(texture.width())
                            .unwrap_or(0)
                            .saturating_mul(usize::try_from(texture.height()).unwrap_or(0))
                            .saturating_mul(4)
                    })
                    .sum::<usize>(),
            )
    }
}

/// Probe flag: a renderable non-air block occupies this palette entry.
pub const PROBE_FLAG_SOLID: u8 = 1;
/// Probe flag: this palette entry is a fluid.
pub const PROBE_FLAG_FLUID: u8 = 2;

/// One visible block resolved by a probe.
#[derive(Clone, Copy, Debug)]
pub struct ProbedBlock<'a> {
    pub state: &'a BlockState,
    pub solid: bool,
    pub fluid: bool,
    pub storage_index: usize,
}

/// Compact decoded chunk data kept by the UI for synchronous raycasting.
#[derive(Clone, Debug)]
pub struct World3dChunkProbe {
    pub pos: ChunkPos,
    pub min_y: i32,
    pub subchunks: BTreeMap<i8, ProbingSubChunk>,
    pub state_version: Option<i32>,
    /// Logical CPU payload estimate, computed once when this immutable probe is built.
    pub(super) storage_bytes: usize,
}

/// One decoded subchunk plus per-storage palette flags for picking.
#[derive(Clone, Debug)]
pub struct ProbingSubChunk {
    pub subchunk: SubChunk,
    /// Per-storage palette flags indexed by palette entry.
    pub flags: Vec<Vec<u8>>,
}

impl ProbingSubChunk {
    pub(super) fn estimated_cpu_bytes(&self) -> usize {
        self.subchunk
            .estimated_cpu_bytes()
            .saturating_add(std::mem::size_of::<Self>() - std::mem::size_of::<SubChunk>())
            .saturating_add(self.flags.capacity() * std::mem::size_of::<Vec<u8>>())
            .saturating_add(self.flags.iter().map(Vec::capacity).sum::<usize>())
    }
}

impl World3dChunkProbe {
    /// Returns the top-most visible non-air state at a world block position.
    #[must_use]
    pub fn block_at(&self, world_x: i32, world_y: i32, world_z: i32) -> Option<ProbedBlock<'_>> {
        let local_x = world_x.rem_euclid(16);
        let local_z = world_z.rem_euclid(16);
        let local_y = world_y.rem_euclid(16);
        let subchunk_y = i8::try_from(world_y.div_euclid(16)).ok()?;
        let probe = self.subchunks.get(&subchunk_y)?;
        let SubChunkFormat::Paletted { storages, .. } = &probe.subchunk.format else {
            return None;
        };
        for (storage_index, storage) in storages.iter().enumerate().rev() {
            let palette_index = usize::from(storage.palette_index_at(
                u8::try_from(local_x).ok()?,
                u8::try_from(local_y).ok()?,
                u8::try_from(local_z).ok()?,
            )?);
            let flags = probe
                .flags
                .get(storage_index)
                .and_then(|flags| flags.get(palette_index))
                .copied()
                .unwrap_or(PROBE_FLAG_SOLID);
            if flags & PROBE_FLAG_SOLID == 0 {
                continue;
            }
            let Some(state) = storage.states.get(palette_index) else {
                continue;
            };
            return Some(ProbedBlock {
                state,
                solid: true,
                fluid: flags & PROBE_FLAG_FLUID != 0,
                storage_index,
            });
        }
        None
    }

    /// Whether the chunk covers the requested world Y range.
    #[must_use]
    pub fn contains_y(&self, world_y: i32) -> bool {
        i8::try_from(world_y.div_euclid(16))
            .ok()
            .is_some_and(|subchunk_y| self.subchunks.contains_key(&subchunk_y))
    }
}

/// New texture assets produced while meshing.
pub struct World3dTextureUpload {
    pub texture: Arc<TextureAsset>,
}

/// Streaming event carrying one finished chunk.
pub struct World3dChunkPayload {
    pub(super) lighting: super::rendering::LightingMode,
    pub generation: u64,
    pub mesh: Arc<World3dChunkMesh>,
    pub probe: Arc<World3dChunkProbe>,
}

/// Reconciles all sections of a chunk after a complete remesh.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct World3dLocalLight {
    /// Stable two-block cluster and direct-specular class within the owning column.
    pub key: [i32; 4],
    pub light: gpui_3d::Light,
}

/// Reconciles geometry and independent emitters, including invisible light blocks.
pub struct World3dChunkCompletion {
    pub(super) lights: Arc<[World3dLocalLight]>,
    pub(super) lighting: super::rendering::LightingMode,
    pub pos: ChunkPos,
    pub sections: Vec<i32>,
    /// Previously published meshes reused by this completion, retained until it is consumed.
    pub retained_meshes: Vec<Arc<World3dChunkMesh>>,
    pub probe: Arc<World3dChunkProbe>,
}

/// Streaming status shown in the HUD.
#[derive(Clone, Debug)]
pub struct ChunkServiceStatus {
    pub generation: u64,
    pub dimension: Dimension,
    pub radius: i32,
    pub loaded_chunks: usize,
    pub coarse_chunks: usize,
    pub pending_chunks: usize,
    pub meshed_chunks: usize,
    pub face_count: usize,
    pub texture_count: usize,
    pub texture_bytes: usize,
    pub cache_bytes: usize,
    pub missing_chunks: usize,
    pub failed_chunks: usize,
    pub last_error: Option<String>,
    pub last_chunk_ms: u64,
    pub last_query_ms: u64,
    pub last_collect_ms: u64,
    pub last_load_stats: ChunkLoadStats,
    pub last_stage_ms: u64,
    pub last_face_ms: u64,
    pub last_convert_ms: u64,
    pub last_mesh_targets: usize,
    pub model_count: usize,
    pub pack_count: usize,
}

impl Default for ChunkServiceStatus {
    fn default() -> Self {
        Self {
            generation: 0,
            dimension: Dimension::Overworld,
            radius: WORLD3D_DEFAULT_VIEW_RADIUS,
            loaded_chunks: 0,
            coarse_chunks: 0,
            pending_chunks: 0,
            meshed_chunks: 0,
            face_count: 0,
            texture_count: 0,
            texture_bytes: 0,
            cache_bytes: 0,
            missing_chunks: 0,
            failed_chunks: 0,
            last_error: None,
            last_chunk_ms: 0,
            last_query_ms: 0,
            last_collect_ms: 0,
            last_load_stats: ChunkLoadStats::default(),
            last_stage_ms: 0,
            last_face_ms: 0,
            last_convert_ms: 0,
            last_mesh_targets: 0,
            model_count: 0,
            pack_count: 0,
        }
    }
}

/// World resource information emitted once after the worker initializes.
#[derive(Clone, Debug)]
pub struct World3dWaterTexture {
    pub asset: Arc<gpui_3d::TextureAsset>,
    pub seconds_per_frame: f32,
}

/// World resource information emitted once after the worker initializes.
#[derive(Clone, Debug, Default)]
pub struct ChunkServiceInfo {
    pub cloud_texture: Option<Arc<gpui_3d::TextureAsset>>,
    pub water_textures: [Option<World3dWaterTexture>; 2],
    pub block_model_count: usize,
    pub resource_pack_count: usize,
    pub texture_resolver_errors: usize,
}

/// Streaming command sent from the UI thread to the background chunk worker.
pub enum ChunkServiceCommand {
    SetLighting(super::rendering::LightingMode),
    ViewUpdated,
    SetView {
        generation: u64,
        dimension: Dimension,
        camera: [f64; 3],
        forward_xz: [f32; 2],
        forward_y: f32,
        radius: i32,
        view_distance: f32,
    },
    ProbeSurface {
        request_id: u64,
        dimension: Dimension,
        positions: Vec<(i32, i32)>,
    },
    Invalidate {
        generation: u64,
        chunks: Vec<ChunkPos>,
    },
    Shutdown,
}

/// Streaming event sent from the background worker to the UI thread.
pub enum ChunkServiceEvent {
    Ready(ChunkServiceInfo),
    Chunk(World3dChunkPayload),
    Completed(Vec<World3dChunkCompletion>),
    Unloaded {
        generation: u64,
        pos: ChunkPos,
    },
    Status(ChunkServiceStatus),
    Surface {
        request_id: u64,
        heights: Vec<Option<i32>>,
    },
    Fatal {
        error: String,
    },
}

/// Result of one 3D edit routed through the owning 2D document history.
#[derive(Clone, Debug)]
pub struct World3dEditOutcome {
    pub request_id: u64,
    pub ok: bool,
    pub message: String,
    pub affected_chunks: Vec<ChunkPos>,
}

/// A block edit requested by the 3D editor and executed by the owning map document.
#[derive(Clone, Debug)]
pub struct World3dBlockEditRequest {
    pub request_id: u64,
    pub dimension: Dimension,
    pub min: [i32; 3],
    pub max: [i32; 3],
    pub kind: World3dBlockEditKind,
}

/// Supported 3D edit operations routed through the shared map edit history.
#[derive(Clone, Debug)]
pub enum World3dBlockEditKind {
    Fill { block_name: String },
    Clear,
}

/// Built-in brush palette entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct World3dBrush {
    pub label: &'static str,
    pub block_name: &'static str,
}

/// Common blocks offered by the 3D editor's brush palette.
pub const WORLD3D_BRUSHES: [World3dBrush; 10] = [
    World3dBrush {
        label: "石头",
        block_name: "minecraft:stone",
    },
    World3dBrush {
        label: "泥土",
        block_name: "minecraft:dirt",
    },
    World3dBrush {
        label: "草方块",
        block_name: "minecraft:grass_block",
    },
    World3dBrush {
        label: "橡木木板",
        block_name: "minecraft:oak_planks",
    },
    World3dBrush {
        label: "玻璃",
        block_name: "minecraft:glass",
    },
    World3dBrush {
        label: "水",
        block_name: "minecraft:water",
    },
    World3dBrush {
        label: "沙子",
        block_name: "minecraft:sand",
    },
    World3dBrush {
        label: "圆石",
        block_name: "minecraft:cobblestone",
    },
    World3dBrush {
        label: "发光石",
        block_name: "minecraft:glowstone",
    },
    World3dBrush {
        label: "空气",
        block_name: "minecraft:air",
    },
];

/// Active left-toolbar tool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum World3dTool {
    /// Fly and look; left click picks a target only.
    #[default]
    Navigate,
    /// Click-drag a box selection in the world.
    Select,
    /// Position the linked 2D import preview before writing it to the world.
    Import,
    /// Place the active brush on the clicked face.
    Place,
    /// Fill the active selection with the brush.
    Fill,
    /// Clear the active selection to air.
    Clear,
}

impl World3dTool {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Navigate => "导航",
            Self::Select => "3D 选择",
            Self::Import => "导入定位",
            Self::Place => "放置",
            Self::Fill => "填充",
            Self::Clear => "清除",
        }
    }
}

/// Formats a Bedrock dimension for UI labels.
#[must_use]
pub fn dimension_label(dimension: Dimension) -> &'static str {
    match dimension {
        Dimension::Overworld => "主世界",
        Dimension::Nether => "下界",
        Dimension::End => "末地",
        Dimension::Unknown(_) => "未知维度",
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_normalizes_corners_and_reports_size() {
        let selection =
            World3dSelection::from_corners(Dimension::Overworld, [10, 70, -5], [-2, 64, 7]);
        assert_eq!(selection.min, [-2, 64, -5]);
        assert_eq!(selection.max, [10, 70, 7]);
        assert_eq!(selection.size(), [13, 7, 13]);
        assert_eq!(selection.volume(), 13 * 7 * 13);
        assert!(selection.contains([0, 66, 0]));
        assert!(!selection.contains([0, 63, 0]));
    }

    #[test]
    fn selection_chunk_positions_cross_negative_boundaries() {
        let selection =
            World3dSelection::from_corners(Dimension::Nether, [-1, 60, -17], [16, 64, 0]);
        let chunks = selection.chunk_positions();
        assert_eq!(chunks.len(), 9);
        assert!(chunks.contains(&ChunkPos {
            x: -1,
            z: -2,
            dimension: Dimension::Nether,
        }));
        assert!(chunks.contains(&ChunkPos {
            x: 1,
            z: 0,
            dimension: Dimension::Nether,
        }));
    }

    #[test]
    fn camera_snapshot_forward_uses_yaw_and_pitch() {
        let snapshot = World3dCameraSnapshot {
            dimension: Dimension::Overworld,
            position: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            vertical_fov_radians: 1.0,
            aspect: 1.0,
            view_distance: 64.0,
            projection: World3dProjectionKind::Perspective,
            ortho_height: 8.0,
            generation: 1,
        };
        let forward = snapshot.forward();
        assert!(forward[0].abs() < 1.0e-6);
        assert!(forward[1].abs() < 1.0e-6);
        assert!((forward[2] + 1.0).abs() < 1.0e-6);
        let xz = snapshot.forward_xz();
        assert!(xz[0].abs() < 1.0e-6);
        assert!((xz[1] + 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn world_keys_fold_case_only_on_windows() {
        let key = World3dWorldKey::from_path(std::path::Path::new("target"));
        assert!(!key.0.is_empty());
        if cfg!(windows) {
            assert_eq!(key.0, key.0.to_ascii_lowercase());
        }
    }

    #[test]
    fn teleport_height_labels_are_stable() {
        assert_eq!(TeleportHeightMode::SurfaceSafe.label(), "地表");
        assert_eq!(TeleportHeightMode::CurrentMapLayer.label(), "当前 Y 层");
        assert!(CameraOverlayMode::PositionFrustum.shows_frustum());
        assert!(!CameraOverlayMode::Position.shows_frustum());
    }
}
