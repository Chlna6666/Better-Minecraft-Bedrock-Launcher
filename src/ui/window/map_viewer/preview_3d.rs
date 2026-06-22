use super::model::{CopiedChunkData, CopiedChunkSnapshot};
use bedrock_block_model::{
    BlockFace, BlockGeometry, BlockModelRepository, BlockStateQuery, BlockStateValue, GeometryBone,
    GeometryCube, ModelCuboid, ModelPlane, ModelShape, ModelWarning,
    block_export_material_name_for_block, block_export_material_name_for_face,
    block_export_material_name_for_plane, block_face_for_normal, canonical_block_name_for_state,
    default_block_face_uvs_from_corners, detail_material_block_name_for_state,
    model_family_has_detail_shape, model_shape_for_block_state,
};
use bedrock_render::{ChunkPos, RenderPalette, RgbaColor};
use bedrock_world::NbtTag;
use bedrock_world::{
    BedrockWorld, BlockState, CancelFlag, ExactSurfaceBiomeLoad, ExactSurfaceSubchunkPolicy,
    ParsedBiomeStorage, ParsedChunkRecordValue, RenderChunkData, RenderChunkLoadOptions,
    RenderChunkPriority, RenderChunkRequest, SlimeChunkBounds, SubChunkDecodeMode,
    TerrainColumnBiome, WorldPipelineOptions, WorldThreadingOptions,
};
use gpui::{
    Bounds, GpuMesh3d, GpuMesh3dCamera, GpuMesh3dDrawRanges, GpuMesh3dRange, GpuMesh3dVertex,
    Pixels, Point, SharedString, Window, px,
};
use rayon::prelude::*;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

const PREVIEW_3D_VERTICAL_SCALE: f32 = 1.0;
const PREVIEW_3D_WATER_ALPHA: f32 = 0.46;
const PREVIEW_3D_DEFAULT_WATER_RGB: [f32; 3] = [28.0 / 255.0, 76.0 / 255.0, 158.0 / 255.0];
const PREVIEW_3D_LAVA_ALPHA: f32 = 1.0;
const PREVIEW_3D_GLASS_ALPHA: f32 = 0.34;
const PREVIEW_3D_MIN_PITCH: f32 = -std::f32::consts::PI + 0.02;
const PREVIEW_3D_MAX_PITCH: f32 = std::f32::consts::PI - 0.02;
const PREVIEW_3D_MODEL_MIN_PITCH: f32 = -std::f32::consts::FRAC_PI_2 + 0.02;
const PREVIEW_3D_MODEL_MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.02;
const PREVIEW_3D_GPU_BUFFER_BUDGET_BYTES: usize = 64 * 1024 * 1024;
const PREVIEW_3D_GPU_VERTEX_BUDGET: usize =
    PREVIEW_3D_GPU_BUFFER_BUDGET_BYTES / std::mem::size_of::<GpuMesh3dVertex>();
const PREVIEW_3D_FACE_BUDGET: usize = PREVIEW_3D_GPU_VERTEX_BUDGET / 6;
const PREVIEW_3D_OPAQUE_FACE_BUDGET: usize = PREVIEW_3D_FACE_BUDGET * 3 / 5;
const PREVIEW_3D_GLASS_FACE_BUDGET: usize = PREVIEW_3D_FACE_BUDGET / 5;
const PREVIEW_3D_WATER_FACE_BUDGET: usize =
    PREVIEW_3D_FACE_BUDGET - PREVIEW_3D_OPAQUE_FACE_BUDGET - PREVIEW_3D_GLASS_FACE_BUDGET;
const PREVIEW_3D_BLOCK_RECORD_BUDGET: usize = PREVIEW_3D_FACE_BUDGET;
const PREVIEW_3D_MIN_ZOOM: f32 = 0.05;
const PREVIEW_3D_MAX_ZOOM: f32 = 64.0;
const PREVIEW_3D_DEFAULT_DISTANCE: f32 = 3.0;
const PREVIEW_3D_FREE_MOVE_SPEED: f32 = 0.65;
const PREVIEW_3D_BASE_FOV_Y_RADIANS: f32 = 55.0_f32.to_radians();
const PREVIEW_3D_NEAR_PLANE: f32 = 0.02;
const PREVIEW_3D_FAR_PLANE: f32 = 256.0;
const PREVIEW_3D_INCREMENTAL_TARGET_UPDATES: usize = 12;
type Preview3dMaterialName = Arc<str>;
type Preview3dMaterialSlot = Arc<str>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Preview3dCamera {
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) zoom: f32,
    pub(super) position: [f32; 3],
}

impl Default for Preview3dCamera {
    fn default() -> Self {
        Self::new(-0.65, 0.68, 1.0)
    }
}

impl Preview3dCamera {
    pub(super) fn new(yaw: f32, pitch: f32, zoom: f32) -> Self {
        let yaw = yaw.rem_euclid(std::f32::consts::TAU);
        let pitch = wrap_preview_3d_pitch(pitch);
        Self {
            yaw,
            pitch,
            zoom,
            position: preview_3d_default_camera_position(yaw, pitch),
        }
    }

    pub(super) fn rotate_view(&mut self, delta_x: f32, delta_y: f32) {
        self.yaw = (self.yaw + delta_x * 0.01).rem_euclid(std::f32::consts::TAU);
        self.pitch = wrap_preview_3d_pitch(self.pitch - delta_y * 0.008);
    }

    pub(super) fn rotate_orbit(&mut self, delta_x: f32, delta_y: f32) {
        self.rotate_view(delta_x, delta_y);
    }

    pub(super) fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(PREVIEW_3D_MIN_ZOOM, PREVIEW_3D_MAX_ZOOM);
    }

    pub(super) fn zoom_by_for_mesh(&mut self, factor: f32, mesh: &Preview3dMesh) {
        let span = mesh.horizontal_span().max(1.0);
        let dynamic_max = PREVIEW_3D_MAX_ZOOM.max(span / 4.0).min(4096.0);
        self.zoom = (self.zoom * factor).clamp(PREVIEW_3D_MIN_ZOOM, dynamic_max);
    }

    pub(super) fn move_from_input(&mut self, input: Preview3dMovementInput, delta_seconds: f32) {
        let mut movement = [0.0, 0.0, 0.0];
        if input.forward {
            movement = vec3_add(movement, self.forward());
        }
        if input.backward {
            movement = vec3_sub(movement, self.forward());
        }
        if input.right {
            movement = vec3_add(movement, self.right());
        }
        if input.left {
            movement = vec3_sub(movement, self.right());
        }
        if input.ascend {
            movement = vec3_add(movement, [0.0, 1.0, 0.0]);
        }
        if input.descend {
            movement = vec3_add(movement, [0.0, -1.0, 0.0]);
        }
        if vec3_length_squared(movement) <= f32::EPSILON {
            return;
        }

        let speed = PREVIEW_3D_FREE_MOVE_SPEED;
        let movement = vec3_scale(
            vec3_normalize(movement),
            speed * delta_seconds.clamp(0.0, 0.05),
        );
        self.position = vec3_add(self.position, movement);
    }

    pub(super) fn forward(self) -> [f32; 3] {
        preview_3d_camera_forward(self.yaw, self.pitch)
    }

    pub(super) fn right(self) -> [f32; 3] {
        preview_3d_camera_right(self.yaw)
    }

    pub(super) const fn gpu_camera(self) -> GpuMesh3dCamera {
        GpuMesh3dCamera {
            yaw: self.yaw,
            pitch: self.pitch,
            zoom: self.zoom,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Preview3dModelRotation {
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) mirror_x: bool,
    pub(super) mirror_z: bool,
}

impl Preview3dModelRotation {
    pub(super) fn rotate_drag(&mut self, delta_x: f32, delta_y: f32) {
        self.yaw = (self.yaw + delta_x * 0.01).rem_euclid(std::f32::consts::TAU);
        self.pitch = wrap_preview_3d_model_pitch(self.pitch - delta_y * 0.008);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Preview3dMovementInput {
    pub(super) forward: bool,
    pub(super) backward: bool,
    pub(super) left: bool,
    pub(super) right: bool,
    pub(super) ascend: bool,
    pub(super) descend: bool,
}

impl Preview3dMovementInput {
    pub(super) fn set_key_pressed(&mut self, key: &str, is_pressed: bool) -> Option<bool> {
        let slot = match key {
            "w" => &mut self.forward,
            "s" => &mut self.backward,
            "a" => &mut self.left,
            "d" => &mut self.right,
            "space" => &mut self.ascend,
            "shift" => &mut self.descend,
            _ => return None,
        };
        let changed = *slot != is_pressed;
        *slot = is_pressed;
        Some(changed)
    }

    pub(super) const fn any_active(self) -> bool {
        self.forward || self.backward || self.left || self.right || self.ascend || self.descend
    }
}

fn wrap_preview_3d_pitch(pitch: f32) -> f32 {
    if !pitch.is_finite() {
        return 0.0;
    }
    let wrapped =
        (pitch + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    wrapped.clamp(PREVIEW_3D_MIN_PITCH, PREVIEW_3D_MAX_PITCH)
}

fn wrap_preview_3d_model_pitch(pitch: f32) -> f32 {
    if !pitch.is_finite() {
        return 0.0;
    }
    pitch.clamp(PREVIEW_3D_MODEL_MIN_PITCH, PREVIEW_3D_MODEL_MAX_PITCH)
}

#[derive(Clone, Debug)]
pub(super) struct Preview3dMesh {
    pub(super) chunk_meshes: Vec<Preview3dChunkMesh>,
    pub(super) min_y: i16,
    pub(super) max_y: i16,
    pub(super) min_x: i32,
    pub(super) max_x: i32,
    pub(super) min_z: i32,
    pub(super) max_z: i32,
    pub(super) missing_chunks: usize,
    pub(super) chunk_count: usize,
    pub(super) processed_chunk_count: usize,
    pub(super) subchunk_count: usize,
    pub(super) solid_block_count: usize,
    pub(super) glass_block_count: usize,
    pub(super) water_block_count: usize,
    pub(super) lava_block_count: usize,
    pub(super) face_count: usize,
    pub(super) glass_face_count: usize,
    pub(super) water_face_count: usize,
    pub(super) lava_face_count: usize,
    pub(super) culled_face_count: usize,
    pub(super) omitted_face_count: usize,
    pub(super) truncated_chunk_count: usize,
    pub(super) vertex_budget: usize,
}

impl Preview3dMesh {
    pub(super) fn vertex_count(&self) -> usize {
        self.chunk_meshes
            .iter()
            .map(|mesh| mesh.gpu_mesh.vertices.len())
            .sum()
    }

    pub(super) fn chunk_mesh_count(&self) -> usize {
        self.chunk_meshes.len()
    }

    pub(super) fn estimated_cpu_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(
                self.chunk_meshes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Preview3dChunkMesh>()),
            )
            .saturating_add(
                self.chunk_meshes
                    .iter()
                    .map(Preview3dChunkMesh::estimated_cpu_bytes)
                    .sum::<usize>(),
            )
    }

    pub(super) fn rendered_chunk_count(&self) -> usize {
        self.processed_chunk_count
            .saturating_sub(self.missing_chunks)
            .min(self.chunk_count)
    }

    pub(super) fn surface_face_count(&self) -> usize {
        self.face_count
            .saturating_add(self.glass_face_count)
            .saturating_add(self.water_face_count)
            .saturating_add(self.lava_face_count)
    }

    pub(super) fn horizontal_span(&self) -> f32 {
        if self.chunk_count == 0 {
            return 1.0;
        }
        let span_x = self
            .max_x
            .saturating_sub(self.min_x)
            .saturating_add(1)
            .max(1) as f32;
        let span_z = self
            .max_z
            .saturating_sub(self.min_z)
            .saturating_add(1)
            .max(1) as f32;
        span_x.max(span_z)
    }
}

#[derive(Clone, Debug)]
pub(super) struct Preview3dChunkMesh {
    pub(super) gpu_mesh: Arc<GpuMesh3d>,
    pub(super) face_materials: Arc<[Preview3dMaterialName]>,
    pub(super) face_uvs: Arc<[[[f32; 2]; 4]]>,
}

impl Preview3dChunkMesh {
    fn estimated_cpu_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(std::mem::size_of::<GpuMesh3d>())
            .saturating_add(
                self.gpu_mesh
                    .vertices
                    .capacity()
                    .saturating_mul(std::mem::size_of::<GpuMesh3dVertex>()),
            )
            .saturating_add(
                self.face_materials
                    .len()
                    .saturating_mul(std::mem::size_of::<Preview3dMaterialName>()),
            )
            .saturating_add(
                self.face_materials
                    .iter()
                    .map(|material| material.len())
                    .sum::<usize>(),
            )
            .saturating_add(
                self.face_uvs
                    .len()
                    .saturating_mul(std::mem::size_of::<[[f32; 2]; 4]>()),
            )
    }
}

#[derive(Clone, Debug)]
struct Preview3dFace {
    corners: [[f32; 3]; 4],
    color: [f32; 4],
    shade: f32,
    normal: [i32; 3],
    material: Preview3dMaterialName,
    uv: Option<[[f32; 2]; 4]>,
}

#[derive(Clone, Debug)]
struct Preview3dBlockRecord {
    key: BlockKey,
    colors: Preview3dBlockFaceColors,
    material: Preview3dMaterialName,
}

impl Preview3dBlockRecord {
    const fn new(
        key: BlockKey,
        colors: Preview3dBlockFaceColors,
        material: Preview3dMaterialName,
    ) -> Self {
        Self {
            key,
            colors,
            material,
        }
    }

    const fn uniform(key: BlockKey, color: [f32; 4], material: Preview3dMaterialName) -> Self {
        Self::new(key, Preview3dBlockFaceColors::uniform(color), material)
    }
}

#[derive(Clone, Copy, Debug)]
struct Preview3dBlockFaceColors {
    up: [f32; 4],
    down: [f32; 4],
    side: [f32; 4],
}

impl Preview3dBlockFaceColors {
    const fn uniform(color: [f32; 4]) -> Self {
        Self {
            up: color,
            down: color,
            side: color,
        }
    }

    const fn color_for_normal(self, normal: [i32; 3]) -> [f32; 4] {
        if normal[1] > 0 {
            self.up
        } else if normal[1] < 0 {
            self.down
        } else {
            self.side
        }
    }
}

#[derive(Clone, Debug)]
struct Preview3dDetailBlock {
    key: BlockKey,
    normalized_name: Arc<str>,
    inferred_connections: bool,
    shape: Preview3dDetailShape,
    color: [f32; 4],
    material: Preview3dMaterialName,
}

#[derive(Clone, Debug, Default)]
struct Preview3dDetailShape {
    cuboids: Vec<Preview3dCuboid>,
    planes: Vec<Preview3dPlane>,
}

impl Preview3dDetailShape {
    fn from_cuboids(cuboids: impl Into<Vec<Preview3dCuboid>>) -> Self {
        Self {
            cuboids: cuboids.into(),
            planes: Vec::new(),
        }
    }

    fn with_planes(mut self, planes: impl Into<Vec<Preview3dPlane>>) -> Self {
        self.planes = planes.into();
        self
    }

    const fn is_empty(&self) -> bool {
        self.cuboids.is_empty() && self.planes.is_empty()
    }
}

#[derive(Clone, Debug)]
struct Preview3dCuboid {
    min: [f32; 3],
    max: [f32; 3],
    material_slot: Option<Preview3dMaterialSlot>,
    face_material_slots: BTreeMap<BlockFace, Preview3dMaterialSlot>,
    face_uvs: BTreeMap<BlockFace, [[f32; 2]; 4]>,
}

impl Preview3dCuboid {
    fn new(min: [f32; 3], max: [f32; 3]) -> Self {
        Self {
            min,
            max,
            material_slot: None,
            face_material_slots: BTreeMap::new(),
            face_uvs: BTreeMap::new(),
        }
    }

    fn with_material_slots(
        mut self,
        material_slot: Option<Preview3dMaterialSlot>,
        face_material_slots: BTreeMap<BlockFace, Preview3dMaterialSlot>,
    ) -> Self {
        self.material_slot = material_slot;
        self.face_material_slots = face_material_slots;
        self
    }

    fn material_slot_for_normal(&self, normal: [i32; 3]) -> Option<Preview3dMaterialSlot> {
        let face = block_face_for_normal(normal);
        self.face_material_slots
            .get(&face)
            .or_else(|| {
                if matches!(
                    face,
                    BlockFace::North | BlockFace::South | BlockFace::East | BlockFace::West
                ) {
                    self.face_material_slots.get(&BlockFace::Side)
                } else {
                    None
                }
            })
            .cloned()
            .or_else(|| self.material_slot.clone())
    }

    fn with_face_material_slot(mut self, face: BlockFace, slot: &str) -> Self {
        if let Some(slot) = preview_3d_material_slot_from_value(slot) {
            self.face_material_slots.insert(face, slot);
        }
        self
    }

    fn face_uv_for_normal(&self, normal: [i32; 3]) -> Option<[[f32; 2]; 4]> {
        let face = block_face_for_normal(normal);
        self.face_uvs
            .get(&face)
            .or_else(|| {
                if matches!(
                    face,
                    BlockFace::North | BlockFace::South | BlockFace::East | BlockFace::West
                ) {
                    self.face_uvs.get(&BlockFace::Side)
                } else {
                    None
                }
            })
            .copied()
    }

    fn with_face_uv(mut self, face: BlockFace, uv: [[f32; 2]; 4]) -> Self {
        self.face_uvs.insert(face, uv);
        self
    }
}

fn preview_3d_detail_cuboid_with_local_uv(cuboid: Preview3dCuboid) -> Preview3dCuboid {
    let min = cuboid.min;
    let max = cuboid.max;
    cuboid
        .with_face_uv(
            BlockFace::Up,
            preview_3d_rect_uv(min[0], min[2], max[0], max[2]),
        )
        .with_face_uv(
            BlockFace::Down,
            preview_3d_rect_uv(min[0], min[2], max[0], max[2]),
        )
        .with_face_uv(
            BlockFace::North,
            preview_3d_rect_uv(min[0], min[1], max[0], max[1]),
        )
        .with_face_uv(
            BlockFace::South,
            preview_3d_rect_uv(min[0], min[1], max[0], max[1]),
        )
        .with_face_uv(
            BlockFace::West,
            preview_3d_rect_uv(min[2], min[1], max[2], max[1]),
        )
        .with_face_uv(
            BlockFace::East,
            preview_3d_rect_uv(min[2], min[1], max[2], max[1]),
        )
}

fn preview_3d_rect_uv(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    [[u0, v0], [u1, v0], [u1, v1], [u0, v1]]
}

fn preview_3d_uv16(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    preview_3d_rect_uv(u0 / 16.0, v0 / 16.0, u1 / 16.0, v1 / 16.0)
}

fn preview_3d_full_texture_uv() -> [[f32; 2]; 4] {
    preview_3d_uv16(0.0, 0.0, 16.0, 16.0)
}

#[derive(Clone, Debug)]
struct Preview3dPlane {
    corners: [[f32; 3]; 4],
    normal: [i32; 3],
    material_slot: Option<Preview3dMaterialSlot>,
    uv: Option<[[f32; 2]; 4]>,
}

fn preview_3d_detail_shape_from_model_shape(shape: ModelShape) -> Preview3dDetailShape {
    Preview3dDetailShape {
        cuboids: shape
            .cuboids
            .into_iter()
            .map(preview_3d_cuboid_from_model_cuboid)
            .collect(),
        planes: shape
            .planes
            .into_iter()
            .map(preview_3d_plane_from_model_plane)
            .collect(),
    }
}

fn preview_3d_cuboid_from_model_cuboid(cuboid: ModelCuboid) -> Preview3dCuboid {
    Preview3dCuboid {
        min: cuboid.min,
        max: cuboid.max,
        material_slot: cuboid
            .material_slot
            .and_then(|slot| preview_3d_material_slot_from_value(&slot)),
        face_material_slots: cuboid
            .face_material_slots
            .into_iter()
            .filter_map(|(face, slot)| {
                preview_3d_material_slot_from_value(&slot).map(|slot| (face, slot))
            })
            .collect(),
        face_uvs: cuboid.face_uvs,
    }
}

fn preview_3d_plane_from_model_plane(plane: ModelPlane) -> Preview3dPlane {
    Preview3dPlane {
        corners: plane.corners,
        normal: plane.normal,
        material_slot: plane
            .material_slot
            .and_then(|slot| preview_3d_material_slot_from_value(&slot)),
        uv: plane.uv,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Preview3dSelectionSignature {
    pub(super) bounds: SlimeChunkBounds,
}

#[derive(Clone, Debug)]
pub(super) enum Preview3dStatus {
    Idle,
    Loading(Preview3dBuildStatus),
    Ready,
    NoSurface(SharedString),
    Error(SharedString),
}

#[derive(Clone, Debug)]
pub(super) struct Preview3dBuildStatus {
    pub(super) phase: SharedString,
    pub(super) detail: SharedString,
}

impl Preview3dBuildStatus {
    pub(super) fn new(phase: impl Into<SharedString>, detail: impl Into<SharedString>) -> Self {
        Self {
            phase: phase.into(),
            detail: detail.into(),
        }
    }
}

impl Default for Preview3dStatus {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Clone)]
pub(super) struct Preview3dState {
    pub(super) source: Preview3dSource,
    pub(super) status: Preview3dStatus,
    pub(super) camera: Preview3dCamera,
    pub(super) model_rotation: Preview3dModelRotation,
    pub(super) mesh: Option<Arc<Preview3dMesh>>,
    pub(super) signature: Option<Preview3dSelectionSignature>,
    pub(super) generation: u64,
    pub(super) drag_origin: Option<Point<Pixels>>,
    pub(super) movement_input: Preview3dMovementInput,
    pub(super) last_motion_frame_at: Option<Instant>,
    pub(super) render_in_flight: bool,
    pub(super) cancel: Option<CancelFlag>,
}

impl Default for Preview3dState {
    fn default() -> Self {
        Self {
            source: Preview3dSource::Selection,
            status: Preview3dStatus::Idle,
            camera: Preview3dCamera::default(),
            model_rotation: Preview3dModelRotation::default(),
            mesh: None,
            signature: None,
            generation: 0,
            drag_origin: None,
            movement_input: Preview3dMovementInput::default(),
            last_motion_frame_at: None,
            render_in_flight: false,
            cancel: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Preview3dSource {
    #[default]
    Selection,
    ImportPreview,
}

impl Preview3dState {
    pub(super) fn clear_resources(&mut self, clear_pipeline: bool) {
        self.status = Preview3dStatus::Idle;
        self.reset_view_and_model();
        self.signature = None;
        self.mesh = None;
        let _ = clear_pipeline;
        self.render_in_flight = false;
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
    }

    pub(super) fn estimated_surface_bytes(&self) -> usize {
        0
    }

    pub(super) fn clear_surface(&mut self) {
        // Nova renders preview meshes through GPUI's retained 3D mesh primitive; there is no
        // per-preview surface cache to clear.
    }

    pub(super) fn clear_navigation_input(&mut self) {
        self.movement_input = Preview3dMovementInput::default();
        self.last_motion_frame_at = None;
    }

    pub(super) fn reset_view_and_model(&mut self) {
        self.camera = Preview3dCamera::default();
        self.model_rotation = Preview3dModelRotation::default();
        self.drag_origin = None;
        self.clear_navigation_input();
    }

    pub(super) fn tick_motion(&mut self, now: Instant, focused: bool) -> bool {
        if !focused {
            self.clear_navigation_input();
            return false;
        }
        if !self.movement_input.any_active() {
            self.last_motion_frame_at = None;
            return false;
        }

        let delta_seconds = self.last_motion_frame_at.map_or(1.0 / 60.0, |previous| {
            (now - previous).as_secs_f32().clamp(1.0 / 240.0, 0.05)
        });
        self.last_motion_frame_at = Some(now);
        self.camera
            .move_from_input(self.movement_input, delta_seconds);
        true
    }
}

pub(super) const fn preview_3d_bounds_width(bounds: SlimeChunkBounds) -> i32 {
    bounds.max_chunk_x.saturating_sub(bounds.min_chunk_x) + 1
}

pub(super) const fn preview_3d_bounds_depth(bounds: SlimeChunkBounds) -> i32 {
    bounds.max_chunk_z.saturating_sub(bounds.min_chunk_z) + 1
}

pub(super) fn load_preview_3d_mesh_blocking_incremental(
    world_path: &Path,
    bounds: SlimeChunkBounds,
    cancel: Option<CancelFlag>,
    mut update: impl FnMut(Arc<Preview3dMesh>, Preview3dBuildStatus) + Send + 'static,
) -> Result<Preview3dMesh, String> {
    bounds.validate().map_err(|error| error.to_string())?;
    check_preview_3d_cancelled(cancel.as_ref())?;
    let total_chunks = preview_3d_bounds_chunk_count(bounds);
    let world = BedrockWorld::open_blocking(world_path, bedrock_world::OpenOptions::default())
        .map_err(|error| error.to_string())?;
    check_preview_3d_cancelled(cancel.as_ref())?;
    let mut positions = preview_3d_chunk_positions(bounds);
    preview_3d_sort_positions_by_distance(
        &mut positions,
        bounds.min_chunk_x + preview_3d_bounds_width(bounds) / 2,
        bounds.min_chunk_z + preview_3d_bounds_depth(bounds) / 2,
    );
    let mut builder = Preview3dMeshBuilder::new(bounds, 0);
    let chunk_total = positions.len();
    let options = preview_3d_render_chunk_load_options(total_chunks, bounds, cancel.clone());
    for chunk_positions in positions.chunks(preview_3d_incremental_mesh_batch_size(chunk_total)) {
        check_preview_3d_cancelled(cancel.as_ref())?;
        let chunks = world
            .load_render_chunks_blocking(chunk_positions.to_vec(), options.clone())
            .map_err(|error| error.to_string())?;
        let processed_chunks = chunks
            .par_iter()
            .map(|chunk| preview_3d_collect_chunk_blocks_result(chunk))
            .collect::<Result<Vec<_>, _>>()?;
        for processed_chunk in processed_chunks {
            builder.push_processed_chunk(processed_chunk);
        }
        let completed_chunks = builder.processed_chunk_count.min(chunk_total);
        if preview_3d_should_emit_incremental_mesh(completed_chunks, chunk_total) {
            let status = Preview3dBuildStatus::new(
                "拼接模型",
                format!("{completed_chunks}/{chunk_total} chunks"),
            );
            emit_preview_3d_mesh_update(&mut builder, &mut update, status)?;
        }
    }

    builder.rebuild_combined_mesh()?;
    Ok(builder.build_mesh())
}

pub(super) fn load_preview_3d_mesh_blocking_incremental_with_block_models(
    world_path: &Path,
    bounds: SlimeChunkBounds,
    block_models: Option<Arc<BlockModelRepository>>,
    cancel: Option<CancelFlag>,
    mut update: impl FnMut(Arc<Preview3dMesh>, Preview3dBuildStatus) + Send + 'static,
) -> Result<Preview3dMesh, String> {
    bounds.validate().map_err(|error| error.to_string())?;
    check_preview_3d_cancelled(cancel.as_ref())?;
    let total_chunks = preview_3d_bounds_chunk_count(bounds);
    let world = BedrockWorld::open_blocking(world_path, bedrock_world::OpenOptions::default())
        .map_err(|error| error.to_string())?;
    check_preview_3d_cancelled(cancel.as_ref())?;
    let mut positions = preview_3d_chunk_positions(bounds);
    preview_3d_sort_positions_by_distance(
        &mut positions,
        bounds.min_chunk_x + preview_3d_bounds_width(bounds) / 2,
        bounds.min_chunk_z + preview_3d_bounds_depth(bounds) / 2,
    );
    let mut builder = Preview3dMeshBuilder::new(bounds, 0);
    let chunk_total = positions.len();
    let options = preview_3d_render_chunk_load_options(total_chunks, bounds, cancel.clone());
    for chunk_positions in positions.chunks(preview_3d_incremental_mesh_batch_size(chunk_total)) {
        check_preview_3d_cancelled(cancel.as_ref())?;
        let chunks = world
            .load_render_chunks_blocking(chunk_positions.to_vec(), options.clone())
            .map_err(|error| error.to_string())?;
        let processed_chunks = chunks
            .par_iter()
            .map(|chunk| {
                preview_3d_collect_chunk_blocks_result_with_block_models(
                    chunk,
                    block_models.as_deref(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        for processed_chunk in processed_chunks {
            builder.push_processed_chunk(processed_chunk);
        }
        let completed_chunks = builder.processed_chunk_count.min(chunk_total);
        if preview_3d_should_emit_incremental_mesh(completed_chunks, chunk_total) {
            let status = Preview3dBuildStatus::new(
                "拼接模型",
                format!("{completed_chunks}/{chunk_total} chunks"),
            );
            emit_preview_3d_mesh_update(&mut builder, &mut update, status)?;
        }
    }

    builder.rebuild_combined_mesh()?;
    Ok(builder.build_mesh())
}

pub(super) fn load_preview_3d_mesh_from_mcstructure_blocking(
    structure: &bedrock_world::McStructureFile,
    anchor_chunk: ChunkPos,
    origin_y: i32,
) -> Result<Preview3dMesh, String> {
    let max_block_x = structure.size.x.saturating_sub(1);
    let max_block_z = structure.size.z.saturating_sub(1);
    let bounds = SlimeChunkBounds {
        dimension: anchor_chunk.dimension,
        min_chunk_x: anchor_chunk.x,
        max_chunk_x: anchor_chunk.x.saturating_add(max_block_x.div_euclid(16)),
        min_chunk_z: anchor_chunk.z,
        max_chunk_z: anchor_chunk.z.saturating_add(max_block_z.div_euclid(16)),
    };
    bounds.validate().map_err(|error| error.to_string())?;

    let palette = structure
        .palette
        .iter()
        .map(|entry| BlockState {
            name: entry.name.clone(),
            states: entry.states.clone(),
            version: entry.version,
        })
        .collect::<Vec<_>>();
    let render_palette = RenderPalette::default();
    let blocks = structure
        .blocks()
        .map_err(|error| format!("结构方块索引无效：{error}"))?;
    let mut chunk_builders = HashMap::<ChunkKey, Preview3dStructureChunkBuilder>::new();
    let origin_x = anchor_chunk.x.saturating_mul(16);
    let origin_z = anchor_chunk.z.saturating_mul(16);

    for block in blocks {
        let key = BlockKey {
            x: origin_x.saturating_add(block.x),
            y: origin_y.saturating_add(block.y),
            z: origin_z.saturating_add(block.z),
        };
        let chunk_key = ChunkKey::from_block(key);
        let builder = chunk_builders.entry(chunk_key).or_default();
        preview_3d_push_structure_block(
            key,
            structure_palette_state(&palette, block.primary),
            structure_palette_state(&palette, block.secondary),
            &render_palette,
            builder,
        )?;
    }

    let mut builder = Preview3dMeshBuilder::new(bounds, 0);
    for chunk_position in preview_3d_chunk_positions(bounds) {
        let chunk_key = ChunkKey::from_pos(chunk_position);
        let mut blocks = chunk_builders
            .remove(&chunk_key)
            .map_or_else(Preview3dChunkBlocks::default, |chunk_builder| {
                chunk_builder.blocks
            });
        blocks.internally_culled_blocks = preview_3d_filter_internal_block_records(&mut blocks);
        builder.push_processed_chunk(Preview3dProcessedChunk {
            chunk_key,
            subchunk_count: preview_3d_structure_subchunk_count(&blocks),
            internally_culled_blocks: blocks.internally_culled_blocks,
            blocks: Some(blocks),
        });
    }

    builder.rebuild_combined_mesh()?;
    Ok(builder.build_mesh())
}

pub(super) fn load_preview_3d_mesh_from_copied_chunk_blocking(
    copied_chunk: &CopiedChunkData,
) -> Result<Preview3dMesh, String> {
    let bounds = copied_chunk_3d_bounds(copied_chunk)?;
    bounds.validate().map_err(|error| error.to_string())?;
    let mut builder = Preview3dMeshBuilder::new(bounds, 0);
    for snapshot in &copied_chunk.chunks {
        let chunk = render_chunk_from_copied_snapshot(snapshot);
        let processed_chunk = preview_3d_collect_chunk_blocks_result(&chunk)?;
        builder.push_processed_chunk(processed_chunk);
    }
    builder.rebuild_combined_mesh()?;
    Ok(builder.build_mesh())
}

fn preview_3d_render_chunk_load_options(
    total_chunks: usize,
    bounds: SlimeChunkBounds,
    cancel: Option<CancelFlag>,
) -> RenderChunkLoadOptions {
    RenderChunkLoadOptions {
        request: RenderChunkRequest::ExactSurface {
            subchunks: ExactSurfaceSubchunkPolicy::Full,
            biome: ExactSurfaceBiomeLoad::All,
            block_entities: false,
        },
        subchunk_decode: SubChunkDecodeMode::FullIndices,
        threading: preview_3d_world_threading(total_chunks),
        pipeline: WorldPipelineOptions {
            queue_depth: preview_3d_queue_depth(total_chunks),
            chunk_batch_size: preview_3d_chunk_batch_size(total_chunks),
            subchunk_decode_workers: preview_3d_subchunk_decode_workers(total_chunks),
            ..WorldPipelineOptions::default()
        },
        cancel,
        priority: RenderChunkPriority::DistanceFrom {
            chunk_x: bounds.min_chunk_x + preview_3d_bounds_width(bounds) / 2,
            chunk_z: bounds.min_chunk_z + preview_3d_bounds_depth(bounds) / 2,
        },
        ..RenderChunkLoadOptions::default()
    }
}

fn check_preview_3d_cancelled(cancel: Option<&CancelFlag>) -> Result<(), String> {
    if cancel.is_some_and(CancelFlag::is_cancelled) {
        return Err("3D 预览任务已取消".to_string());
    }
    Ok(())
}

fn preview_3d_chunk_positions(bounds: SlimeChunkBounds) -> Vec<ChunkPos> {
    (bounds.min_chunk_z..=bounds.max_chunk_z)
        .flat_map(|z| {
            (bounds.min_chunk_x..=bounds.max_chunk_x).map(move |x| ChunkPos {
                x,
                z,
                dimension: bounds.dimension,
            })
        })
        .collect()
}

fn preview_3d_sort_positions_by_distance(
    positions: &mut [ChunkPos],
    center_chunk_x: i32,
    center_chunk_z: i32,
) {
    positions.sort_by_key(|position| {
        let dx = i64::from(position.x) - i64::from(center_chunk_x);
        let dz = i64::from(position.z) - i64::from(center_chunk_z);
        (
            dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)),
            position.z,
            position.x,
            position.dimension,
        )
    });
}

fn preview_3d_bounds_chunk_count(bounds: SlimeChunkBounds) -> usize {
    usize::try_from(preview_3d_bounds_width(bounds))
        .unwrap_or(0)
        .saturating_mul(usize::try_from(preview_3d_bounds_depth(bounds)).unwrap_or(0))
}

fn preview_3d_queue_depth(total_chunks: usize) -> usize {
    total_chunks.clamp(16, 256)
}

fn preview_3d_chunk_batch_size(total_chunks: usize) -> usize {
    total_chunks.clamp(16, 128)
}

fn preview_3d_world_threading(total_chunks: usize) -> WorldThreadingOptions {
    if total_chunks <= 1 {
        WorldThreadingOptions::Single
    } else {
        WorldThreadingOptions::Auto
    }
}

fn preview_3d_subchunk_decode_workers(total_chunks: usize) -> usize {
    if total_chunks < 16 {
        0
    } else {
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .saturating_sub(1)
            .clamp(1, 8)
    }
}

fn preview_3d_incremental_mesh_batch_size(total_chunks: usize) -> usize {
    (total_chunks.saturating_add(PREVIEW_3D_INCREMENTAL_TARGET_UPDATES - 1)
        / PREVIEW_3D_INCREMENTAL_TARGET_UPDATES)
        .max(1)
}

fn preview_3d_should_emit_incremental_mesh(completed_chunks: usize, total_chunks: usize) -> bool {
    completed_chunks > 0 && completed_chunks <= total_chunks
}

fn emit_preview_3d_mesh_update(
    builder: &mut Preview3dMeshBuilder,
    update: &mut impl FnMut(Arc<Preview3dMesh>, Preview3dBuildStatus),
    status: Preview3dBuildStatus,
) -> Result<(), String> {
    builder.rebuild_combined_mesh()?;
    let mesh = Arc::new(builder.build_mesh());
    if mesh.surface_face_count() != 0 {
        update(mesh, status);
    }
    Ok(())
}

struct Preview3dMeshBuilder {
    bounds: SlimeChunkBounds,
    combined_meshes: Vec<Preview3dChunkMesh>,
    mesh_generation: u64,
    min_y: i16,
    max_y: i16,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    missing_chunks: usize,
    processed_chunk_count: usize,
    subchunk_count: usize,
    solid_block_count: usize,
    glass_block_count: usize,
    water_block_count: usize,
    lava_block_count: usize,
    face_count: usize,
    glass_face_count: usize,
    water_face_count: usize,
    lava_face_count: usize,
    culled_face_count: usize,
    omitted_face_count: usize,
    truncated_chunk_count: usize,
    block_chunks: HashMap<ChunkKey, Preview3dChunkBlocks>,
    processed_chunk_keys: HashSet<ChunkKey>,
}

struct Preview3dProcessedChunk {
    chunk_key: ChunkKey,
    subchunk_count: usize,
    internally_culled_blocks: usize,
    blocks: Option<Preview3dChunkBlocks>,
}

impl Preview3dMeshBuilder {
    fn new(bounds: SlimeChunkBounds, truncated_chunk_count: usize) -> Self {
        Self {
            bounds,
            combined_meshes: Vec::new(),
            mesh_generation: 0,
            min_y: i16::MAX,
            max_y: i16::MIN,
            min_x: i32::MAX,
            max_x: i32::MIN,
            min_z: i32::MAX,
            max_z: i32::MIN,
            missing_chunks: 0,
            processed_chunk_count: 0,
            subchunk_count: 0,
            solid_block_count: 0,
            glass_block_count: 0,
            water_block_count: 0,
            lava_block_count: 0,
            face_count: 0,
            glass_face_count: 0,
            water_face_count: 0,
            lava_face_count: 0,
            culled_face_count: 0,
            omitted_face_count: 0,
            truncated_chunk_count,
            block_chunks: HashMap::new(),
            processed_chunk_keys: HashSet::new(),
        }
    }

    fn push_processed_chunk(&mut self, processed_chunk: Preview3dProcessedChunk) {
        self.processed_chunk_count = self.processed_chunk_count.saturating_add(1);
        self.processed_chunk_keys.insert(processed_chunk.chunk_key);
        self.culled_face_count = self
            .culled_face_count
            .saturating_add(processed_chunk.internally_culled_blocks.saturating_mul(6));
        let Some(mut blocks) = processed_chunk.blocks else {
            self.missing_chunks = self.missing_chunks.saturating_add(1);
            return;
        };
        blocks.rebuild_detail_connectors();
        self.subchunk_count = self
            .subchunk_count
            .saturating_add(processed_chunk.subchunk_count);
        self.block_chunks.insert(processed_chunk.chunk_key, blocks);
    }

    fn build_mesh(&self) -> Preview3dMesh {
        Preview3dMesh {
            chunk_meshes: self.combined_meshes.clone(),
            min_y: if self.min_y == i16::MAX {
                0
            } else {
                self.min_y
            },
            max_y: if self.max_y == i16::MIN {
                0
            } else {
                self.max_y
            },
            min_x: if self.min_x == i32::MAX {
                0
            } else {
                self.min_x
            },
            max_x: if self.max_x == i32::MIN {
                0
            } else {
                self.max_x
            },
            min_z: if self.min_z == i32::MAX {
                0
            } else {
                self.min_z
            },
            max_z: if self.max_z == i32::MIN {
                0
            } else {
                self.max_z
            },
            missing_chunks: self.missing_chunks,
            chunk_count: preview_3d_bounds_chunk_count(self.bounds),
            processed_chunk_count: self.processed_chunk_count,
            subchunk_count: self.subchunk_count,
            solid_block_count: self.solid_block_count,
            glass_block_count: self.glass_block_count,
            water_block_count: self.water_block_count,
            lava_block_count: self.lava_block_count,
            face_count: self.face_count,
            glass_face_count: self.glass_face_count,
            water_face_count: self.water_face_count,
            lava_face_count: self.lava_face_count,
            culled_face_count: self.culled_face_count,
            omitted_face_count: self.omitted_face_count,
            truncated_chunk_count: self.truncated_chunk_count,
            vertex_budget: PREVIEW_3D_GPU_VERTEX_BUDGET,
        }
    }

    fn rebuild_combined_mesh(&mut self) -> Result<(), String> {
        self.recalculate_block_stats();
        self.mesh_generation = self.mesh_generation.saturating_add(1);
        let (build, omitted_face_count, water_face_count, lava_face_count) =
            self.build_combined_meshes(self.mesh_generation)?;
        self.combined_meshes = build;
        self.omitted_face_count = omitted_face_count;
        self.recalculate_mesh_stats();
        self.water_face_count = water_face_count;
        self.lava_face_count = lava_face_count;
        Ok(())
    }

    fn build_combined_meshes(
        &self,
        generation: u64,
    ) -> Result<(Vec<Preview3dChunkMesh>, usize, usize, usize), String> {
        let ((opaque_result, glass_result), (water_result, lava_result)) = rayon::join(
            || {
                rayon::join(
                    || self.collect_opaque_faces(),
                    || self.collect_glass_faces(),
                )
            },
            || rayon::join(|| self.collect_water_faces(), || self.collect_lava_faces()),
        );
        let (opaque_faces, opaque_omitted) = opaque_result;
        let (glass_faces, glass_omitted) = glass_result;
        let (water_faces, water_omitted) = water_result;
        let (lava_faces, lava_omitted) = lava_result;
        let omitted_face_count = opaque_omitted
            .saturating_add(glass_omitted)
            .saturating_add(water_omitted)
            .saturating_add(lava_omitted);

        if opaque_faces.is_empty()
            && glass_faces.is_empty()
            && water_faces.is_empty()
            && lava_faces.is_empty()
        {
            return Ok((Vec::new(), omitted_face_count, 0, 0));
        }
        let (center, horizontal_span, vertical_span) = self.mesh_frame();
        let meshes = build_preview_3d_gpu_meshes(
            &opaque_faces,
            &glass_faces,
            &water_faces,
            &lava_faces,
            center,
            horizontal_span,
            vertical_span,
            generation,
        )?;
        Ok((
            meshes,
            omitted_face_count,
            water_faces.len(),
            lava_faces.len(),
        ))
    }

    fn collect_opaque_faces(&self) -> (Vec<Preview3dFace>, usize) {
        let mut budget = Preview3dFaceBudget::unbounded();
        let mut faces = Vec::with_capacity(PREVIEW_3D_OPAQUE_FACE_BUDGET.min(64 * 1024));
        self.push_opaque_faces(&mut faces, &mut budget);
        (faces, budget.omitted_faces)
    }

    fn collect_glass_faces(&self) -> (Vec<Preview3dFace>, usize) {
        let mut budget = Preview3dFaceBudget::unbounded();
        let mut faces = Vec::with_capacity(PREVIEW_3D_GLASS_FACE_BUDGET.min(16 * 1024));
        self.push_glass_faces(&mut faces, &mut budget);
        (faces, budget.omitted_faces)
    }

    fn collect_water_faces(&self) -> (Vec<Preview3dFace>, usize) {
        let mut budget = Preview3dFaceBudget::unbounded();
        let mut faces = Vec::with_capacity(PREVIEW_3D_WATER_FACE_BUDGET.min(16 * 1024));
        self.push_water_faces(&mut faces, &mut budget);
        (faces, budget.omitted_faces)
    }

    fn collect_lava_faces(&self) -> (Vec<Preview3dFace>, usize) {
        let mut budget = Preview3dFaceBudget::unbounded();
        let mut faces = Vec::with_capacity((PREVIEW_3D_WATER_FACE_BUDGET / 2).min(8 * 1024));
        self.push_lava_faces(&mut faces, &mut budget);
        (faces, budget.omitted_faces)
    }

    fn push_opaque_faces(&self, faces: &mut Vec<Preview3dFace>, budget: &mut Preview3dFaceBudget) {
        let merger = self
            .block_chunks
            .par_iter()
            .map(|blocks| {
                let blocks = blocks.1;
                let mut merger = Preview3dFaceMerger::new();
                for block in &blocks.opaque_blocks {
                    for face in FACE_DEFINITIONS {
                        if self.is_preview_3d_opaque_neighbor(block.key, face) {
                            continue;
                        }
                        merger.push(
                            block.key,
                            face,
                            block.colors.color_for_normal(face.normal),
                            block.material.clone(),
                        );
                    }
                }
                merger
            })
            .reduce(Preview3dFaceMerger::new, Preview3dFaceMerger::merge);
        merger.emit_into(faces, budget);
        self.push_detail_faces(faces, budget, |blocks| &blocks.detail_blocks);
    }

    fn mesh_frame(&self) -> ([f32; 3], f32, f32) {
        if self.min_y == i16::MAX || self.max_y == i16::MIN {
            return preview_3d_selection_frame(self.bounds);
        }
        let min_x = self.min_x as f32;
        let max_x = self.max_x as f32 + 1.0;
        let min_z = self.min_z as f32;
        let max_z = self.max_z as f32 + 1.0;
        let min_y = f32::from(self.min_y);
        let max_y = f32::from(self.max_y).max(min_y + 1.0) + 1.0;
        let horizontal_span = (max_x - min_x).max(max_z - min_z).max(1.0);
        let vertical_span = (max_y - min_y).max(1.0);
        (
            [
                (min_x + max_x) * 0.5,
                (min_y + max_y) * 0.5,
                (min_z + max_z) * 0.5,
            ],
            horizontal_span,
            vertical_span,
        )
    }

    fn push_glass_faces(&self, faces: &mut Vec<Preview3dFace>, budget: &mut Preview3dFaceBudget) {
        let merger = self
            .block_chunks
            .par_iter()
            .map(|blocks| {
                let blocks = blocks.1;
                let mut merger = Preview3dFaceMerger::new();
                for block in &blocks.glass_blocks {
                    for face in FACE_DEFINITIONS {
                        if self.is_preview_3d_glass_neighbor(block.key, face) {
                            continue;
                        }
                        merger.push(
                            block.key,
                            face,
                            block.colors.color_for_normal(face.normal),
                            block.material.clone(),
                        );
                    }
                }
                merger
            })
            .reduce(Preview3dFaceMerger::new, Preview3dFaceMerger::merge);
        merger.emit_into(faces, budget);
        self.push_detail_faces(faces, budget, |blocks| &blocks.glass_detail_blocks);
    }

    fn push_detail_faces<'a>(
        &'a self,
        faces: &mut Vec<Preview3dFace>,
        budget: &mut Preview3dFaceBudget,
        blocks_for_chunk: impl Fn(&'a Preview3dChunkBlocks) -> &'a [Preview3dDetailBlock] + Copy + Sync,
    ) {
        let detail_faces = self
            .block_chunks
            .par_iter()
            .map(|blocks| {
                let mut faces = Vec::new();
                for block in blocks_for_chunk(blocks.1) {
                    let inferred_shape = self.preview_3d_inferred_detail_shape(block);
                    let shape = inferred_shape.as_ref().unwrap_or(&block.shape);
                    preview_3d_push_detail_block_faces(block, shape, &mut faces);
                }
                faces
            })
            .reduce(Vec::new, |mut left, mut right| {
                left.append(&mut right);
                left
            });
        for face in detail_faces {
            budget.push_or_omit(faces, face);
        }
    }

    fn push_water_faces(&self, faces: &mut Vec<Preview3dFace>, budget: &mut Preview3dFaceBudget) {
        let merger = self
            .block_chunks
            .par_iter()
            .map(|blocks| {
                let blocks = blocks.1;
                let mut merger = Preview3dFaceMerger::new();
                for block in &blocks.water_blocks {
                    for face in FACE_DEFINITIONS {
                        if block.surface_only && face.normal != [0, 1, 0] {
                            continue;
                        }
                        if self.is_preview_3d_water_neighbor(block.key, face) {
                            continue;
                        }
                        merger.push(block.key, face, block.color, block.material.clone());
                    }
                }
                merger
            })
            .reduce(Preview3dFaceMerger::new, Preview3dFaceMerger::merge);
        merger.emit_into(faces, budget);
    }

    fn push_lava_faces(&self, faces: &mut Vec<Preview3dFace>, budget: &mut Preview3dFaceBudget) {
        let merger = self
            .block_chunks
            .par_iter()
            .map(|blocks| {
                let blocks = blocks.1;
                let mut merger = Preview3dFaceMerger::new();
                for block in &blocks.lava_blocks {
                    for face in FACE_DEFINITIONS {
                        if block.surface_only && face.normal != [0, 1, 0] {
                            continue;
                        }
                        if self.is_preview_3d_lava_neighbor(block.key, face) {
                            continue;
                        }
                        merger.push(block.key, face, block.color, block.material.clone());
                    }
                }
                merger
            })
            .reduce(Preview3dFaceMerger::new, Preview3dFaceMerger::merge);
        merger.emit_into(faces, budget);
    }

    fn recalculate_block_stats(&mut self) {
        self.min_y = i16::MAX;
        self.max_y = i16::MIN;
        self.min_x = i32::MAX;
        self.max_x = i32::MIN;
        self.min_z = i32::MAX;
        self.max_z = i32::MIN;
        self.solid_block_count = 0;
        self.glass_block_count = 0;
        self.water_block_count = 0;
        self.lava_block_count = 0;
        for blocks in self.block_chunks.values() {
            self.solid_block_count = self
                .solid_block_count
                .saturating_add(blocks.opaque_blocks.len())
                .saturating_add(blocks.detail_blocks.len());
            self.glass_block_count = self
                .glass_block_count
                .saturating_add(blocks.glass_blocks.len())
                .saturating_add(blocks.glass_detail_blocks.len());
            self.water_block_count = self
                .water_block_count
                .saturating_add(blocks.water_blocks.len());
            self.lava_block_count = self
                .lava_block_count
                .saturating_add(blocks.lava_blocks.len());
            if blocks.min_y != i16::MAX {
                self.min_y = self.min_y.min(blocks.min_y);
                self.max_y = self.max_y.max(blocks.max_y);
                self.min_x = self.min_x.min(blocks.min_x);
                self.max_x = self.max_x.max(blocks.max_x);
                self.min_z = self.min_z.min(blocks.min_z);
                self.max_z = self.max_z.max(blocks.max_z);
            }
        }
    }

    fn recalculate_mesh_stats(&mut self) {
        self.face_count = 0;
        self.glass_face_count = 0;
        self.water_face_count = 0;
        self.lava_face_count = 0;
        for mesh in &self.combined_meshes {
            self.face_count = self
                .face_count
                .saturating_add(mesh.gpu_mesh.ranges.opaque.count as usize / 6);
            self.glass_face_count = self
                .glass_face_count
                .saturating_add(mesh.gpu_mesh.ranges.glass.count as usize / 6);
        }
    }

    fn is_preview_3d_opaque_neighbor(&self, block: BlockKey, face: FaceDefinition) -> bool {
        let neighbor = block.neighbor(face);
        self.block_class_at(neighbor) == Some(Preview3dBlockClass::Opaque)
    }

    fn is_preview_3d_glass_neighbor(&self, block: BlockKey, face: FaceDefinition) -> bool {
        let neighbor = block.neighbor(face);
        matches!(
            self.block_class_at(neighbor),
            Some(Preview3dBlockClass::Opaque | Preview3dBlockClass::TransparentGlass)
        )
    }

    fn is_preview_3d_water_neighbor(&self, block: BlockKey, face: FaceDefinition) -> bool {
        let neighbor = block.neighbor(face);
        matches!(
            self.block_class_at(neighbor),
            Some(Preview3dBlockClass::Opaque | Preview3dBlockClass::Water)
        )
    }

    fn is_preview_3d_lava_neighbor(&self, block: BlockKey, face: FaceDefinition) -> bool {
        let neighbor = block.neighbor(face);
        matches!(
            self.block_class_at(neighbor),
            Some(Preview3dBlockClass::Opaque | Preview3dBlockClass::Lava)
        )
    }

    fn block_class_at(&self, block: BlockKey) -> Option<Preview3dBlockClass> {
        if self.is_unprocessed_selected_neighbor(block) {
            return Some(Preview3dBlockClass::Opaque);
        }
        self.block_chunks
            .get(&ChunkKey::from_block(block))
            .and_then(|chunk| chunk.class_at(block))
    }

    fn preview_3d_inferred_detail_shape(
        &self,
        block: &Preview3dDetailBlock,
    ) -> Option<Preview3dDetailShape> {
        if !block.inferred_connections
            || !preview_3d_is_pane_like_block(block.normalized_name.as_ref())
        {
            return None;
        }
        let block_name = if block.normalized_name.starts_with("minecraft:") {
            block.normalized_name.to_string()
        } else {
            format!("minecraft:{}", block.normalized_name)
        };
        let mut query = BlockStateQuery::new(block_name);
        for direction in Preview3dCardinalDirection::ALL {
            if self.preview_3d_pane_neighbor_connects(block.key, direction) {
                query = query.with_state(direction.state_key(), true);
            }
        }
        model_shape_for_block_state(&query).map(preview_3d_detail_shape_from_model_shape)
    }

    fn preview_3d_pane_neighbor_connects(
        &self,
        block: BlockKey,
        direction: Preview3dCardinalDirection,
    ) -> bool {
        let neighbor = block.cardinal_neighbor(direction);
        if matches!(
            self.block_class_at(neighbor),
            Some(Preview3dBlockClass::Opaque | Preview3dBlockClass::TransparentGlass)
        ) {
            return true;
        }
        self.block_chunks
            .get(&ChunkKey::from_block(neighbor))
            .is_some_and(|chunk| chunk.detail_connector_at(neighbor))
    }

    fn is_unprocessed_selected_neighbor(&self, block: BlockKey) -> bool {
        let chunk_count = preview_3d_bounds_chunk_count(self.bounds);
        if self.processed_chunk_count == 0 || self.processed_chunk_count >= chunk_count {
            return false;
        }
        let chunk_key = ChunkKey::from_block(block);
        self.chunk_key_in_bounds(chunk_key) && !self.processed_chunk_keys.contains(&chunk_key)
    }

    fn chunk_key_in_bounds(&self, chunk_key: ChunkKey) -> bool {
        chunk_key.x >= self.bounds.min_chunk_x
            && chunk_key.x <= self.bounds.max_chunk_x
            && chunk_key.z >= self.bounds.min_chunk_z
            && chunk_key.z <= self.bounds.max_chunk_z
    }
}

fn preview_3d_selection_frame(bounds: SlimeChunkBounds) -> ([f32; 3], f32, f32) {
    let min_x = (bounds.min_chunk_x.saturating_mul(16)) as f32;
    let max_x = (bounds.max_chunk_x.saturating_add(1).saturating_mul(16)) as f32;
    let min_z = (bounds.min_chunk_z.saturating_mul(16)) as f32;
    let max_z = (bounds.max_chunk_z.saturating_add(1).saturating_mul(16)) as f32;
    let horizontal_span = (max_x - min_x).max(max_z - min_z).max(1.0);
    (
        [(min_x + max_x) * 0.5, 0.5, (min_z + max_z) * 0.5],
        horizontal_span,
        1.0,
    )
}

fn preview_3d_collect_chunk_blocks_result(
    chunk: &RenderChunkData,
) -> Result<Preview3dProcessedChunk, String> {
    preview_3d_collect_chunk_blocks_result_with_block_models(chunk, None)
}

fn preview_3d_collect_chunk_blocks_result_with_block_models(
    chunk: &RenderChunkData,
    block_models: Option<&BlockModelRepository>,
) -> Result<Preview3dProcessedChunk, String> {
    let chunk_key = ChunkKey::from_pos(chunk.pos);
    if !chunk.is_loaded {
        return Ok(Preview3dProcessedChunk {
            chunk_key,
            subchunk_count: 0,
            internally_culled_blocks: 0,
            blocks: None,
        });
    }
    let blocks = preview_3d_collect_chunk_blocks_with_block_models(chunk, block_models)?;
    let internally_culled_blocks = blocks.internally_culled_blocks;
    Ok(Preview3dProcessedChunk {
        chunk_key,
        subchunk_count: chunk.subchunks.len(),
        internally_culled_blocks,
        blocks: Some(blocks),
    })
}

fn render_chunk_from_copied_snapshot(snapshot: &CopiedChunkSnapshot) -> RenderChunkData {
    let parsed = bedrock_world::parsed::parse_chunk_records_with_options(
        snapshot.chunk,
        snapshot.records.clone(),
        copied_chunk_preview_3d_parse_options(),
    );
    let mut subchunks = BTreeMap::new();
    let mut legacy_terrain = None;
    let mut version = bedrock_world::ChunkVersion::New;
    for record in parsed.records {
        match record.value {
            ParsedChunkRecordValue::SubChunk(subchunk) => {
                subchunks.insert(subchunk.y, subchunk);
            }
            ParsedChunkRecordValue::LegacyTerrain(terrain) => {
                legacy_terrain = Some(terrain);
                version = bedrock_world::ChunkVersion::Old;
            }
            ParsedChunkRecordValue::Version(version_byte) => {
                if version_byte < 25 {
                    version = bedrock_world::ChunkVersion::Old;
                }
            }
            _ => {}
        }
    }
    RenderChunkData {
        pos: snapshot.chunk,
        is_loaded: !subchunks.is_empty() || legacy_terrain.is_some(),
        height_map: None,
        legacy_biomes: None,
        legacy_biome_colors: None,
        biome_data: BTreeMap::new(),
        subchunks,
        block_entities: Vec::new(),
        legacy_terrain,
        column_samples: None,
        version,
    }
}

fn copied_chunk_3d_bounds(copied_chunk: &CopiedChunkData) -> Result<SlimeChunkBounds, String> {
    let mut chunks = copied_chunk.chunks.iter().map(|chunk| chunk.chunk);
    let Some(first) = chunks.next() else {
        return Err("导入区域包没有可预览的 chunk".to_string());
    };
    let mut bounds = SlimeChunkBounds {
        dimension: first.dimension,
        min_chunk_x: first.x,
        max_chunk_x: first.x,
        min_chunk_z: first.z,
        max_chunk_z: first.z,
    };
    for chunk in chunks {
        bounds.min_chunk_x = bounds.min_chunk_x.min(chunk.x);
        bounds.max_chunk_x = bounds.max_chunk_x.max(chunk.x);
        bounds.min_chunk_z = bounds.min_chunk_z.min(chunk.z);
        bounds.max_chunk_z = bounds.max_chunk_z.max(chunk.z);
    }
    Ok(bounds)
}

fn copied_chunk_preview_3d_parse_options() -> bedrock_world::WorldParseOptions {
    bedrock_world::WorldParseOptions {
        categories: bedrock_world::WorldParseCategories {
            chunks: true,
            players: false,
            actors: false,
            maps: false,
            villages: false,
            globals: false,
        },
        retention: bedrock_world::RetentionMode::Structured,
        subchunk_decode_mode: SubChunkDecodeMode::FullIndices,
        actor_resolution: bedrock_world::ActorResolution::None,
    }
}

fn preview_3d_collect_chunk_blocks(
    chunk: &RenderChunkData,
) -> Result<Preview3dChunkBlocks, String> {
    preview_3d_collect_chunk_blocks_with_block_models(chunk, None)
}

fn preview_3d_collect_chunk_blocks_with_block_models(
    chunk: &RenderChunkData,
    block_models: Option<&BlockModelRepository>,
) -> Result<Preview3dChunkBlocks, String> {
    let palette = RenderPalette::default();
    let initial_block_capacity = chunk
        .subchunks
        .len()
        .saturating_mul(16 * 16 * 16)
        .min(PREVIEW_3D_BLOCK_RECORD_BUDGET);
    let mut occupied = HashSet::<BlockKey>::with_capacity(initial_block_capacity);
    let mut glass = HashSet::<BlockKey>::with_capacity(initial_block_capacity / 8);
    let mut water = HashSet::<BlockKey>::with_capacity(initial_block_capacity / 8);
    let mut lava = HashSet::<BlockKey>::with_capacity(initial_block_capacity / 16);
    let mut opaque_blocks = Vec::<Preview3dBlockRecord>::with_capacity(initial_block_capacity);
    let mut glass_blocks = Vec::<Preview3dBlockRecord>::with_capacity(initial_block_capacity / 8);
    let mut detail_blocks = Vec::<Preview3dDetailBlock>::with_capacity(initial_block_capacity / 8);
    let mut glass_detail_blocks =
        Vec::<Preview3dDetailBlock>::with_capacity(initial_block_capacity / 16);
    let mut water_blocks = Vec::<Preview3dFluidBlock>::with_capacity(initial_block_capacity / 8);
    let mut lava_blocks = Vec::<Preview3dFluidBlock>::with_capacity(initial_block_capacity / 16);
    let mut block_budget = Preview3dBlockBudget::new(initial_block_capacity);
    let mut min_y = i16::MAX;
    let mut max_y = i16::MIN;
    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_z = i32::MAX;
    let mut max_z = i32::MIN;

    for (subchunk_y, subchunk) in &chunk.subchunks {
        let base_y = i32::from(*subchunk_y) * 16;
        for local_y in 0u8..16 {
            let y = base_y + i32::from(local_y);
            for local_z in 0u8..16 {
                for local_x in 0u8..16 {
                    let key = BlockKey {
                        x: chunk.pos.x.saturating_mul(16) + i32::from(local_x),
                        y,
                        z: chunk.pos.z.saturating_mul(16) + i32::from(local_z),
                    };
                    let biome = preview_3d_biome_at_or_top(chunk, local_x, local_z, y);
                    let primary_state = subchunk.block_state_at(local_x, local_y, local_z);
                    let primary_class =
                        primary_state.map(|state| preview_3d_block_class(&state.name));
                    if let Some(state) = primary_state {
                        let block_class = preview_3d_block_class(&state.name);
                        if preview_3d_block_class_is_renderable(block_class) {
                            preview_3d_push_collected_block(
                                key,
                                state,
                                block_class,
                                false,
                                biome,
                                block_models,
                                &palette,
                                &mut block_budget,
                                &mut occupied,
                                &mut glass,
                                &mut water,
                                &mut lava,
                                &mut opaque_blocks,
                                &mut glass_blocks,
                                &mut detail_blocks,
                                &mut glass_detail_blocks,
                                &mut water_blocks,
                                &mut lava_blocks,
                                &mut min_x,
                                &mut max_x,
                                &mut min_y,
                                &mut max_y,
                                &mut min_z,
                                &mut max_z,
                            )?;
                        }
                    }

                    let fluid_state = subchunk
                        .visible_block_states_at(local_x, local_y, local_z)
                        .find(|state| {
                            matches!(
                                preview_3d_block_class(&state.name),
                                Preview3dBlockClass::Water | Preview3dBlockClass::Lava
                            )
                        });
                    if let Some(state) = fluid_state {
                        let block_class = preview_3d_block_class(&state.name);
                        if primary_class != Some(block_class) {
                            let surface_only = matches!(
                                primary_class,
                                Some(
                                    Preview3dBlockClass::Opaque
                                        | Preview3dBlockClass::TransparentGlass
                                )
                            );
                            preview_3d_push_collected_block(
                                key,
                                state,
                                block_class,
                                surface_only,
                                biome,
                                block_models,
                                &palette,
                                &mut block_budget,
                                &mut occupied,
                                &mut glass,
                                &mut water,
                                &mut lava,
                                &mut opaque_blocks,
                                &mut glass_blocks,
                                &mut detail_blocks,
                                &mut glass_detail_blocks,
                                &mut water_blocks,
                                &mut lava_blocks,
                                &mut min_x,
                                &mut max_x,
                                &mut min_y,
                                &mut max_y,
                                &mut min_z,
                                &mut max_z,
                            )?;
                        }
                    }

                    if primary_class.is_none_or(|block_class| {
                        !preview_3d_block_class_is_renderable(block_class)
                    }) && fluid_state.is_none()
                    {
                        if let Some(state) = preview_3d_visible_renderable_block_state_at(
                            subchunk, local_x, local_y, local_z,
                        ) {
                            let block_class = preview_3d_block_class(&state.name);
                            preview_3d_push_collected_block(
                                key,
                                state,
                                block_class,
                                false,
                                biome,
                                block_models,
                                &palette,
                                &mut block_budget,
                                &mut occupied,
                                &mut glass,
                                &mut water,
                                &mut lava,
                                &mut opaque_blocks,
                                &mut glass_blocks,
                                &mut detail_blocks,
                                &mut glass_detail_blocks,
                                &mut water_blocks,
                                &mut lava_blocks,
                                &mut min_x,
                                &mut max_x,
                                &mut min_y,
                                &mut max_y,
                                &mut min_z,
                                &mut max_z,
                            )?;
                        }
                    }
                }
            }
        }
    }

    let mut blocks = Preview3dChunkBlocks {
        occupied,
        glass,
        water,
        lava,
        detail_connectors: HashSet::new(),
        opaque_blocks,
        glass_blocks,
        detail_blocks,
        glass_detail_blocks,
        water_blocks,
        lava_blocks,
        min_y,
        max_y,
        min_x,
        max_x,
        min_z,
        max_z,
        internally_culled_blocks: 0,
    };
    blocks.rebuild_detail_connectors();
    blocks.internally_culled_blocks = preview_3d_filter_internal_block_records(&mut blocks);
    Ok(blocks)
}

#[allow(clippy::too_many_arguments)]
fn preview_3d_push_collected_block(
    key: BlockKey,
    state: &BlockState,
    block_class: Preview3dBlockClass,
    surface_only: bool,
    biome: Option<Preview3dBiomeSample>,
    block_models: Option<&BlockModelRepository>,
    palette: &RenderPalette,
    block_budget: &mut Preview3dBlockBudget,
    occupied: &mut HashSet<BlockKey>,
    glass: &mut HashSet<BlockKey>,
    water: &mut HashSet<BlockKey>,
    lava: &mut HashSet<BlockKey>,
    opaque_blocks: &mut Vec<Preview3dBlockRecord>,
    glass_blocks: &mut Vec<Preview3dBlockRecord>,
    detail_blocks: &mut Vec<Preview3dDetailBlock>,
    glass_detail_blocks: &mut Vec<Preview3dDetailBlock>,
    water_blocks: &mut Vec<Preview3dFluidBlock>,
    lava_blocks: &mut Vec<Preview3dFluidBlock>,
    min_x: &mut i32,
    max_x: &mut i32,
    min_y: &mut i16,
    max_y: &mut i16,
    min_z: &mut i32,
    max_z: &mut i32,
) -> Result<(), String> {
    if !block_budget.try_take() {
        return Err("3D 预览单区块方块记录超出预算".to_string());
    }
    preview_3d_include_block_bounds(key, min_x, max_x, min_y, max_y, min_z, max_z);
    let material_source = preview_3d_material_block_name_for_state(state, block_class);
    let material = preview_3d_material_name_for_block(material_source.as_ref(), block_class);
    let normalized_name = Arc::<str>::from(preview_3d_normalized_block_name(&state.name));
    let inferred_connections = preview_3d_should_infer_detail_connections(state);
    let resolved_shape = block_models
        .and_then(|models| preview_3d_resolved_detail_shape_for_block(models, state, block_class));
    match block_class {
        Preview3dBlockClass::Opaque => {
            if let Some(shape) = resolved_shape.or_else(|| preview_3d_detail_shape_for_block(state))
            {
                if !shape.is_empty() {
                    detail_blocks.push(Preview3dDetailBlock {
                        key,
                        normalized_name: normalized_name.clone(),
                        inferred_connections,
                        shape,
                        color: preview_3d_color_for_block(palette, state, biome),
                        material,
                    });
                    return Ok(());
                }
            }
            occupied.insert(key);
            opaque_blocks.push(Preview3dBlockRecord::new(
                key,
                preview_3d_face_colors_for_block(palette, state, biome),
                material,
            ));
        }
        Preview3dBlockClass::TransparentGlass => {
            if let Some(shape) = resolved_shape.or_else(|| preview_3d_detail_shape_for_block(state))
            {
                if !shape.is_empty() {
                    glass_detail_blocks.push(Preview3dDetailBlock {
                        key,
                        normalized_name: normalized_name.clone(),
                        inferred_connections,
                        shape,
                        color: preview_3d_transparent_color_for_block(palette, state, biome),
                        material,
                    });
                    return Ok(());
                }
            }
            glass.insert(key);
            glass_blocks.push(Preview3dBlockRecord::uniform(
                key,
                preview_3d_transparent_color_for_block(palette, state, biome),
                material,
            ));
        }
        Preview3dBlockClass::Water => {
            water.insert(key);
            water_blocks.push(Preview3dFluidBlock {
                key,
                color: preview_3d_water_color_for_block(palette, state, biome),
                material,
                surface_only,
            });
        }
        Preview3dBlockClass::Lava => {
            lava.insert(key);
            lava_blocks.push(Preview3dFluidBlock {
                key,
                color: preview_3d_lava_color_for_block(palette, state, biome),
                material,
                surface_only,
            });
        }
        Preview3dBlockClass::DetailOpaque | Preview3dBlockClass::DetailGlass => {
            let Some(shape) = resolved_shape.or_else(|| preview_3d_detail_shape_for_block(state))
            else {
                return Ok(());
            };
            if shape.is_empty() {
                return Ok(());
            }
            let color = if block_class == Preview3dBlockClass::DetailGlass {
                preview_3d_transparent_color_for_block(palette, state, biome)
            } else {
                preview_3d_color_for_block(palette, state, biome)
            };
            let block = Preview3dDetailBlock {
                key,
                normalized_name,
                inferred_connections,
                shape,
                color,
                material,
            };
            if block_class == Preview3dBlockClass::DetailGlass {
                glass_detail_blocks.push(block);
            } else {
                detail_blocks.push(block);
            }
        }
        Preview3dBlockClass::Air | Preview3dBlockClass::SkipTransparent => {}
    }
    Ok(())
}

fn preview_3d_visible_renderable_block_state_at(
    subchunk: &bedrock_world::SubChunk,
    local_x: u8,
    local_y: u8,
    local_z: u8,
) -> Option<&BlockState> {
    subchunk
        .visible_block_states_at(local_x, local_y, local_z)
        .find(|state| preview_3d_block_class_is_renderable(preview_3d_block_class(&state.name)))
}

fn preview_3d_filter_internal_block_records(blocks: &mut Preview3dChunkBlocks) -> usize {
    let opaque_before = blocks.opaque_blocks.len();
    let glass_before = blocks.glass_blocks.len();
    let water_before = blocks.water_blocks.len();
    let lava_before = blocks.lava_blocks.len();

    let occupied = &blocks.occupied;
    blocks
        .opaque_blocks
        .retain(|block| !preview_3d_opaque_block_is_fully_hidden(block.key, occupied));

    let glass = &blocks.glass;
    let water = &blocks.water;
    let lava = &blocks.lava;
    blocks.glass_blocks.retain(|block| {
        !preview_3d_transparent_block_is_fully_hidden(block.key, occupied, glass, water, lava)
    });

    blocks.water_blocks.retain(|block| {
        if block.surface_only {
            !preview_3d_surface_fluid_block_is_hidden(block.key, occupied, water)
        } else {
            !preview_3d_transparent_block_is_fully_hidden(block.key, occupied, water, glass, lava)
        }
    });

    blocks.lava_blocks.retain(|block| {
        if block.surface_only {
            !preview_3d_surface_fluid_block_is_hidden(block.key, occupied, lava)
        } else {
            !preview_3d_transparent_block_is_fully_hidden(block.key, occupied, lava, glass, water)
        }
    });

    opaque_before
        .saturating_sub(blocks.opaque_blocks.len())
        .saturating_add(glass_before.saturating_sub(blocks.glass_blocks.len()))
        .saturating_add(water_before.saturating_sub(blocks.water_blocks.len()))
        .saturating_add(lava_before.saturating_sub(blocks.lava_blocks.len()))
}

fn preview_3d_opaque_block_is_fully_hidden(block: BlockKey, occupied: &HashSet<BlockKey>) -> bool {
    FACE_DEFINITIONS
        .iter()
        .all(|face| occupied.contains(&block.neighbor(*face)))
}

fn preview_3d_transparent_block_is_fully_hidden(
    block: BlockKey,
    occupied: &HashSet<BlockKey>,
    same_class: &HashSet<BlockKey>,
    other_transparent_a: &HashSet<BlockKey>,
    other_transparent_b: &HashSet<BlockKey>,
) -> bool {
    FACE_DEFINITIONS.iter().all(|face| {
        let neighbor = block.neighbor(*face);
        same_class.contains(&neighbor)
            || (occupied.contains(&neighbor)
                && !other_transparent_a.contains(&neighbor)
                && !other_transparent_b.contains(&neighbor))
    })
}

fn preview_3d_surface_fluid_block_is_hidden(
    block: BlockKey,
    occupied: &HashSet<BlockKey>,
    same_class: &HashSet<BlockKey>,
) -> bool {
    let above = block.neighbor(FACE_DEFINITIONS[0]);
    occupied.contains(&above) || same_class.contains(&above)
}

fn preview_3d_include_block_bounds(
    block: BlockKey,
    min_x: &mut i32,
    max_x: &mut i32,
    min_y: &mut i16,
    max_y: &mut i16,
    min_z: &mut i32,
    max_z: &mut i32,
) {
    *min_x = (*min_x).min(block.x);
    *max_x = (*max_x).max(block.x);
    if let Ok(y) = i16::try_from(block.y) {
        *min_y = (*min_y).min(y);
        *max_y = (*max_y).max(y);
    }
    *min_z = (*min_z).min(block.z);
    *max_z = (*max_z).max(block.z);
}

struct Preview3dBlockBudget {
    max_records: usize,
    used_records: usize,
}

impl Preview3dBlockBudget {
    const fn new(max_records: usize) -> Self {
        Self {
            max_records,
            used_records: 0,
        }
    }

    const fn is_full(&self) -> bool {
        self.used_records >= self.max_records
    }

    fn try_take(&mut self) -> bool {
        if self.is_full() {
            return false;
        }
        self.used_records = self.used_records.saturating_add(1);
        true
    }
}

struct Preview3dFaceBudget {
    max_faces: usize,
    emitted_faces: usize,
    omitted_faces: usize,
}

impl Preview3dFaceBudget {
    #[cfg(test)]
    const fn new(max_faces: usize) -> Self {
        Self {
            max_faces,
            emitted_faces: 0,
            omitted_faces: 0,
        }
    }

    const fn unbounded() -> Self {
        Self {
            max_faces: usize::MAX,
            emitted_faces: 0,
            omitted_faces: 0,
        }
    }

    fn push_or_omit(&mut self, faces: &mut Vec<Preview3dFace>, face: Preview3dFace) {
        if self.emitted_faces < self.max_faces {
            faces.push(face);
            self.emitted_faces = self.emitted_faces.saturating_add(1);
        } else {
            self.omitted_faces = self.omitted_faces.saturating_add(1);
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct Preview3dFacePlaneKey {
    axis: u8,
    normal_positive: bool,
    plane: i32,
    color: [u32; 4],
    shade_bits: u32,
    material: Preview3dMaterialName,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct Preview3dFaceCell {
    u: i32,
    v: i32,
}

#[derive(Default)]
struct Preview3dFaceMerger {
    planes: BTreeMap<Preview3dFacePlaneKey, BTreeSet<Preview3dFaceCell>>,
}

impl Preview3dFaceMerger {
    fn new() -> Self {
        Self::default()
    }

    fn merge(mut self, other: Self) -> Self {
        for (key, cells) in other.planes {
            self.planes.entry(key).or_default().extend(cells);
        }
        self
    }

    fn push(
        &mut self,
        block: BlockKey,
        face: FaceDefinition,
        color: [f32; 4],
        material: Preview3dMaterialName,
    ) {
        let Some(axis) = face.axis() else {
            return;
        };
        let normal_positive = face.normal[axis] > 0;
        let plane = match axis {
            0 => block.x + i32::from(normal_positive),
            1 => block.y + i32::from(normal_positive),
            2 => block.z + i32::from(normal_positive),
            _ => return,
        };
        let (u, v) = match axis {
            0 => (block.z, block.y),
            1 => (block.x, block.z),
            2 => (block.x, block.y),
            _ => return,
        };
        let material = preview_3d_material_name_for_face(material, face.normal);
        let key = Preview3dFacePlaneKey {
            axis: axis as u8,
            normal_positive,
            plane,
            color: color.map(f32::to_bits),
            shade_bits: face.shade.to_bits(),
            material,
        };
        self.planes
            .entry(key)
            .or_default()
            .insert(Preview3dFaceCell { u, v });
    }

    fn emit_into(self, faces: &mut Vec<Preview3dFace>, budget: &mut Preview3dFaceBudget) {
        for (key, cells) in self.planes {
            for rectangle in preview_3d_merge_face_cells(cells) {
                budget.push_or_omit(faces, rectangle.into_face(key.clone()));
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Preview3dMergedFaceRect {
    u0: i32,
    v0: i32,
    u1: i32,
    v1: i32,
}

impl Preview3dMergedFaceRect {
    fn into_face(self, key: Preview3dFacePlaneKey) -> Preview3dFace {
        let color = key.color.map(f32::from_bits);
        let shade = f32::from_bits(key.shade_bits);
        let p = key.plane as f32;
        let u0 = self.u0 as f32;
        let u1 = self.u1 as f32;
        let v0 = self.v0 as f32;
        let v1 = self.v1 as f32;
        let corners = match (key.axis, key.normal_positive) {
            (0, true) => [[p, v0, u0], [p, v0, u1], [p, v1, u1], [p, v1, u0]],
            (0, false) => [[p, v0, u1], [p, v0, u0], [p, v1, u0], [p, v1, u1]],
            (1, true) => [[u0, p, v0], [u1, p, v0], [u1, p, v1], [u0, p, v1]],
            (1, false) => [[u0, p, v1], [u1, p, v1], [u1, p, v0], [u0, p, v0]],
            (2, true) => [[u1, v0, p], [u0, v0, p], [u0, v1, p], [u1, v1, p]],
            (2, false) => [[u0, v0, p], [u1, v0, p], [u1, v1, p], [u0, v1, p]],
            _ => [[0.0; 3]; 4],
        };
        Preview3dFace {
            corners,
            color,
            shade,
            material: key.material,
            normal: match (key.axis, key.normal_positive) {
                (0, true) => [1, 0, 0],
                (0, false) => [-1, 0, 0],
                (1, true) => [0, 1, 0],
                (1, false) => [0, -1, 0],
                (2, true) => [0, 0, 1],
                (2, false) => [0, 0, -1],
                _ => [0, 0, 0],
            },
            uv: None,
        }
    }
}

fn preview_3d_merge_face_cells(
    mut cells: BTreeSet<Preview3dFaceCell>,
) -> Vec<Preview3dMergedFaceRect> {
    let mut rectangles = Vec::new();
    while let Some(start) = cells.iter().next().copied() {
        let mut u1 = start.u + 1;
        while cells.contains(&Preview3dFaceCell { u: u1, v: start.v }) {
            u1 += 1;
        }

        let mut v1 = start.v + 1;
        'rows: loop {
            for u in start.u..u1 {
                if !cells.contains(&Preview3dFaceCell { u, v: v1 }) {
                    break 'rows;
                }
            }
            v1 += 1;
        }

        for v in start.v..v1 {
            for u in start.u..u1 {
                cells.remove(&Preview3dFaceCell { u, v });
            }
        }
        rectangles.push(Preview3dMergedFaceRect {
            u0: start.u,
            v0: start.v,
            u1,
            v1,
        });
    }
    rectangles
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ChunkKey {
    x: i32,
    z: i32,
}

impl ChunkKey {
    const fn from_pos(pos: ChunkPos) -> Self {
        Self { x: pos.x, z: pos.z }
    }

    fn from_block(block: BlockKey) -> Self {
        Self {
            x: block.x.div_euclid(16),
            z: block.z.div_euclid(16),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct BlockKey {
    x: i32,
    y: i32,
    z: i32,
}

impl BlockKey {
    const fn neighbor(self, face: FaceDefinition) -> Self {
        Self {
            x: self.x + face.normal[0],
            y: self.y + face.normal[1],
            z: self.z + face.normal[2],
        }
    }

    const fn cardinal_neighbor(self, direction: Preview3dCardinalDirection) -> Self {
        let [x, y, z] = direction.normal();
        Self {
            x: self.x + x,
            y: self.y + y,
            z: self.z + z,
        }
    }
}

struct Preview3dChunkBlocks {
    occupied: HashSet<BlockKey>,
    glass: HashSet<BlockKey>,
    water: HashSet<BlockKey>,
    lava: HashSet<BlockKey>,
    detail_connectors: HashSet<BlockKey>,
    opaque_blocks: Vec<Preview3dBlockRecord>,
    glass_blocks: Vec<Preview3dBlockRecord>,
    detail_blocks: Vec<Preview3dDetailBlock>,
    glass_detail_blocks: Vec<Preview3dDetailBlock>,
    water_blocks: Vec<Preview3dFluidBlock>,
    lava_blocks: Vec<Preview3dFluidBlock>,
    min_y: i16,
    max_y: i16,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    internally_culled_blocks: usize,
}

#[derive(Clone, Debug)]
struct Preview3dFluidBlock {
    key: BlockKey,
    color: [f32; 4],
    material: Preview3dMaterialName,
    surface_only: bool,
}

impl Default for Preview3dChunkBlocks {
    fn default() -> Self {
        Self {
            occupied: HashSet::new(),
            glass: HashSet::new(),
            water: HashSet::new(),
            lava: HashSet::new(),
            detail_connectors: HashSet::new(),
            opaque_blocks: Vec::new(),
            glass_blocks: Vec::new(),
            detail_blocks: Vec::new(),
            glass_detail_blocks: Vec::new(),
            water_blocks: Vec::new(),
            lava_blocks: Vec::new(),
            min_y: i16::MAX,
            max_y: i16::MIN,
            min_x: i32::MAX,
            max_x: i32::MIN,
            min_z: i32::MAX,
            max_z: i32::MIN,
            internally_culled_blocks: 0,
        }
    }
}

impl Preview3dChunkBlocks {
    fn rebuild_detail_connectors(&mut self) {
        self.detail_connectors.clear();
        for block in self
            .detail_blocks
            .iter()
            .chain(self.glass_detail_blocks.iter())
        {
            if preview_3d_detail_block_connects_to_panes(block.normalized_name.as_ref()) {
                self.detail_connectors.insert(block.key);
            }
        }
    }

    fn class_at(&self, block: BlockKey) -> Option<Preview3dBlockClass> {
        if self.occupied.contains(&block) {
            return Some(Preview3dBlockClass::Opaque);
        }
        if self.glass.contains(&block) {
            return Some(Preview3dBlockClass::TransparentGlass);
        }
        if self.water.contains(&block) {
            return Some(Preview3dBlockClass::Water);
        }
        if self.lava.contains(&block) {
            return Some(Preview3dBlockClass::Lava);
        }
        None
    }

    fn detail_connector_at(&self, block: BlockKey) -> bool {
        self.detail_connectors.contains(&block)
    }
}

struct Preview3dStructureChunkBuilder {
    blocks: Preview3dChunkBlocks,
    block_budget: Preview3dBlockBudget,
}

impl Default for Preview3dStructureChunkBuilder {
    fn default() -> Self {
        Self {
            blocks: Preview3dChunkBlocks::default(),
            block_budget: Preview3dBlockBudget::new(PREVIEW_3D_BLOCK_RECORD_BUDGET),
        }
    }
}

fn structure_palette_state(palette: &[BlockState], index: i32) -> Option<&BlockState> {
    let index = usize::try_from(index).ok()?;
    palette.get(index)
}

fn preview_3d_structure_subchunk_count(blocks: &Preview3dChunkBlocks) -> usize {
    let mut subchunks = HashSet::new();
    for block in &blocks.opaque_blocks {
        subchunks.insert(block.key.y.div_euclid(16));
    }
    for block in &blocks.glass_blocks {
        subchunks.insert(block.key.y.div_euclid(16));
    }
    for block in &blocks.detail_blocks {
        subchunks.insert(block.key.y.div_euclid(16));
    }
    for block in &blocks.glass_detail_blocks {
        subchunks.insert(block.key.y.div_euclid(16));
    }
    for block in &blocks.water_blocks {
        subchunks.insert(block.key.y.div_euclid(16));
    }
    for block in &blocks.lava_blocks {
        subchunks.insert(block.key.y.div_euclid(16));
    }
    subchunks.len()
}

fn preview_3d_push_structure_block(
    key: BlockKey,
    primary: Option<&BlockState>,
    secondary: Option<&BlockState>,
    render_palette: &RenderPalette,
    builder: &mut Preview3dStructureChunkBuilder,
) -> Result<(), String> {
    let primary_class = primary.map(|state| preview_3d_block_class(&state.name));
    if let Some((state, block_class)) = primary.zip(primary_class) {
        if preview_3d_block_class_is_renderable(block_class) {
            preview_3d_push_structure_state(
                key,
                state,
                block_class,
                false,
                render_palette,
                builder,
            )?;
        }
    }

    let secondary_class = secondary.map(|state| preview_3d_block_class(&state.name));
    let mut pushed_secondary = false;
    if let Some((state, block_class)) = secondary.zip(secondary_class) {
        if matches!(
            block_class,
            Preview3dBlockClass::Water | Preview3dBlockClass::Lava
        ) && primary_class != Some(block_class)
        {
            let surface_only = matches!(
                primary_class,
                Some(Preview3dBlockClass::Opaque | Preview3dBlockClass::TransparentGlass)
            );
            preview_3d_push_structure_state(
                key,
                state,
                block_class,
                surface_only,
                render_palette,
                builder,
            )?;
            pushed_secondary = true;
        }
    }

    if primary_class.is_none_or(|block_class| !preview_3d_block_class_is_renderable(block_class))
        && !pushed_secondary
    {
        if let Some((state, block_class)) = secondary.zip(secondary_class) {
            if preview_3d_block_class_is_renderable(block_class) {
                preview_3d_push_structure_state(
                    key,
                    state,
                    block_class,
                    false,
                    render_palette,
                    builder,
                )?;
            }
        }
    }
    Ok(())
}

fn preview_3d_push_structure_state(
    key: BlockKey,
    state: &BlockState,
    block_class: Preview3dBlockClass,
    surface_only: bool,
    render_palette: &RenderPalette,
    builder: &mut Preview3dStructureChunkBuilder,
) -> Result<(), String> {
    preview_3d_push_collected_block(
        key,
        state,
        block_class,
        surface_only,
        None,
        None,
        render_palette,
        &mut builder.block_budget,
        &mut builder.blocks.occupied,
        &mut builder.blocks.glass,
        &mut builder.blocks.water,
        &mut builder.blocks.lava,
        &mut builder.blocks.opaque_blocks,
        &mut builder.blocks.glass_blocks,
        &mut builder.blocks.detail_blocks,
        &mut builder.blocks.glass_detail_blocks,
        &mut builder.blocks.water_blocks,
        &mut builder.blocks.lava_blocks,
        &mut builder.blocks.min_x,
        &mut builder.blocks.max_x,
        &mut builder.blocks.min_y,
        &mut builder.blocks.max_y,
        &mut builder.blocks.min_z,
        &mut builder.blocks.max_z,
    )
}

#[derive(Clone, Copy)]
struct FaceDefinition {
    normal: [i32; 3],
    #[cfg(test)]
    corners: [[f32; 3]; 4],
    shade: f32,
}

impl FaceDefinition {
    fn axis(self) -> Option<usize> {
        self.normal.iter().position(|normal| *normal != 0)
    }
}

const FACE_DEFINITIONS: [FaceDefinition; 6] = [
    FaceDefinition {
        normal: [0, 1, 0],
        #[cfg(test)]
        corners: [
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ],
        shade: 1.0,
    },
    FaceDefinition {
        normal: [0, -1, 0],
        #[cfg(test)]
        corners: [
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0],
        ],
        shade: 0.42,
    },
    FaceDefinition {
        normal: [1, 0, 0],
        #[cfg(test)]
        corners: [
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
        ],
        shade: 0.70,
    },
    FaceDefinition {
        normal: [-1, 0, 0],
        #[cfg(test)]
        corners: [
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 1.0],
        ],
        shade: 0.62,
    },
    FaceDefinition {
        normal: [0, 0, 1],
        #[cfg(test)]
        corners: [
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
        ],
        shade: 0.56,
    },
    FaceDefinition {
        normal: [0, 0, -1],
        #[cfg(test)]
        corners: [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        shade: 0.78,
    },
];

#[cfg(test)]
fn block_face(block: BlockKey, face: FaceDefinition, color: [f32; 4]) -> Preview3dFace {
    Preview3dFace {
        corners: face.corners.map(|corner| {
            [
                block.x as f32 + corner[0],
                block.y as f32 + corner[1],
                block.z as f32 + corner[2],
            ]
        }),
        color,
        shade: face.shade,
        normal: face.normal,
        material: Arc::from("minecraft_test"),
        uv: None,
    }
}

fn preview_3d_push_detail_block_faces(
    block: &Preview3dDetailBlock,
    shape: &Preview3dDetailShape,
    faces: &mut Vec<Preview3dFace>,
) {
    for cuboid in &shape.cuboids {
        preview_3d_push_cuboid_faces(
            block.key,
            cuboid.clone(),
            block.color,
            block.material.clone(),
            faces,
        );
    }
    for plane in &shape.planes {
        preview_3d_push_plane_face(
            block.key,
            plane.clone(),
            block.color,
            block.material.clone(),
            faces,
        );
        preview_3d_push_plane_face(
            block.key,
            Preview3dPlane {
                corners: [
                    plane.corners[3],
                    plane.corners[2],
                    plane.corners[1],
                    plane.corners[0],
                ],
                normal: [-plane.normal[0], -plane.normal[1], -plane.normal[2]],
                material_slot: plane.material_slot.clone(),
                uv: plane.uv.map(|[a, b, c, d]| [d, c, b, a]),
            },
            block.color,
            block.material.clone(),
            faces,
        );
    }
}

fn preview_3d_push_cuboid_faces(
    block: BlockKey,
    cuboid: Preview3dCuboid,
    color: [f32; 4],
    material: Preview3dMaterialName,
    faces: &mut Vec<Preview3dFace>,
) {
    if cuboid.min[0] >= cuboid.max[0]
        || cuboid.min[1] >= cuboid.max[1]
        || cuboid.min[2] >= cuboid.max[2]
    {
        return;
    }
    let [x0, y0, z0] = cuboid.min;
    let [x1, y1, z1] = cuboid.max;
    let planes = [
        Preview3dPlane {
            corners: [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]],
            normal: [0, 1, 0],
            material_slot: cuboid.material_slot_for_normal([0, 1, 0]),
            uv: cuboid.face_uv_for_normal([0, 1, 0]),
        },
        Preview3dPlane {
            corners: [[x0, y0, z1], [x1, y0, z1], [x1, y0, z0], [x0, y0, z0]],
            normal: [0, -1, 0],
            material_slot: cuboid.material_slot_for_normal([0, -1, 0]),
            uv: cuboid.face_uv_for_normal([0, -1, 0]),
        },
        Preview3dPlane {
            corners: [[x1, y0, z0], [x1, y0, z1], [x1, y1, z1], [x1, y1, z0]],
            normal: [1, 0, 0],
            material_slot: cuboid.material_slot_for_normal([1, 0, 0]),
            uv: cuboid.face_uv_for_normal([1, 0, 0]),
        },
        Preview3dPlane {
            corners: [[x0, y0, z1], [x0, y0, z0], [x0, y1, z0], [x0, y1, z1]],
            normal: [-1, 0, 0],
            material_slot: cuboid.material_slot_for_normal([-1, 0, 0]),
            uv: cuboid.face_uv_for_normal([-1, 0, 0]),
        },
        Preview3dPlane {
            corners: [[x1, y0, z1], [x0, y0, z1], [x0, y1, z1], [x1, y1, z1]],
            normal: [0, 0, 1],
            material_slot: cuboid.material_slot_for_normal([0, 0, 1]),
            uv: cuboid.face_uv_for_normal([0, 0, 1]),
        },
        Preview3dPlane {
            corners: [[x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0]],
            normal: [0, 0, -1],
            material_slot: cuboid.material_slot_for_normal([0, 0, -1]),
            uv: cuboid.face_uv_for_normal([0, 0, -1]),
        },
    ];
    for plane in planes {
        preview_3d_push_plane_face(block, plane, color, material.clone(), faces);
    }
}

fn preview_3d_push_plane_face(
    block: BlockKey,
    plane: Preview3dPlane,
    color: [f32; 4],
    material: Preview3dMaterialName,
    faces: &mut Vec<Preview3dFace>,
) {
    faces.push(Preview3dFace {
        corners: plane.corners.map(|corner| {
            [
                block.x as f32 + corner[0],
                block.y as f32 + corner[1],
                block.z as f32 + corner[2],
            ]
        }),
        color,
        shade: preview_3d_shade_for_normal(plane.normal),
        normal: plane.normal,
        material: preview_3d_material_name_for_plane(material, &plane),
        uv: plane.uv,
    });
}

fn preview_3d_shade_for_normal(normal: [i32; 3]) -> f32 {
    FACE_DEFINITIONS
        .iter()
        .find(|face| face.normal == normal)
        .map_or(0.72, |face| face.shade)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Preview3dBlockClass {
    Air,
    Opaque,
    TransparentGlass,
    DetailOpaque,
    DetailGlass,
    Water,
    Lava,
    SkipTransparent,
}

fn preview_3d_block_class(name: &str) -> Preview3dBlockClass {
    let normalized = name.strip_prefix("minecraft:").unwrap_or(name);
    if matches!(normalized, "air" | "cave_air" | "void_air") {
        return Preview3dBlockClass::Air;
    }
    if matches!(normalized, "water" | "flowing_water") {
        return Preview3dBlockClass::Water;
    }
    if matches!(normalized, "lava" | "flowing_lava") {
        return Preview3dBlockClass::Lava;
    }
    if preview_3d_is_full_grass_block(normalized) {
        return Preview3dBlockClass::Opaque;
    }
    if preview_3d_is_glass_block(normalized) {
        if preview_3d_is_detail_shape_block(normalized) {
            return Preview3dBlockClass::DetailGlass;
        }
        return Preview3dBlockClass::TransparentGlass;
    }
    if preview_3d_is_foliage_block(normalized) {
        return Preview3dBlockClass::Opaque;
    }
    if preview_3d_is_detail_shape_block(normalized) {
        return Preview3dBlockClass::DetailOpaque;
    }
    if preview_3d_is_transparent_detail_block(normalized) {
        return Preview3dBlockClass::SkipTransparent;
    }
    Preview3dBlockClass::Opaque
}

fn preview_3d_block_class_is_renderable(block_class: Preview3dBlockClass) -> bool {
    !matches!(
        block_class,
        Preview3dBlockClass::Air | Preview3dBlockClass::SkipTransparent
    )
}

#[cfg(test)]
fn preview_3d_is_solid_block(name: &str) -> bool {
    preview_3d_block_class(name) == Preview3dBlockClass::Opaque
}

fn preview_3d_is_transparent_detail_block(normalized: &str) -> bool {
    let transparent_exact = [
        "deadbush",
        "kelp",
        "kelp_plant",
        "seagrass",
        "tall_seagrass",
        "snow_layer",
        "tripwire",
    ];
    if transparent_exact.contains(&normalized) {
        return true;
    }

    let transparent_suffixes = ["mushroom", "banner", "coral"];

    transparent_suffixes
        .iter()
        .any(|suffix| normalized == *suffix || normalized.ends_with(&format!("_{suffix}")))
}

fn preview_3d_is_detail_shape_block(normalized: &str) -> bool {
    model_family_has_detail_shape(normalized)
}

fn preview_3d_is_foliage_block(normalized: &str) -> bool {
    if normalized.contains("leaf_litter") {
        return false;
    }
    normalized == "leaves"
        || normalized.ends_with("_leaves")
        || normalized.contains("leaves")
        || normalized.ends_with("_leaf")
        || normalized.contains("foliage")
}

fn preview_3d_is_glass_block(normalized: &str) -> bool {
    normalized == "glass"
        || normalized == "glass_pane"
        || normalized.ends_with("_glass")
        || normalized.ends_with("_glass_pane")
        || normalized.contains("stained_glass")
}

fn preview_3d_detail_shape_for_block(state: &BlockState) -> Option<Preview3dDetailShape> {
    model_shape_for_block_state(&preview_3d_block_state_query(state))
        .map(preview_3d_detail_shape_from_model_shape)
}

fn preview_3d_resolved_detail_shape_for_block(
    repository: &BlockModelRepository,
    state: &BlockState,
    block_class: Preview3dBlockClass,
) -> Option<Preview3dDetailShape> {
    if matches!(
        block_class,
        Preview3dBlockClass::Water | Preview3dBlockClass::Lava
    ) {
        return None;
    }
    let normalized = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    if normalized == "shulker_box" || normalized.ends_with("_shulker_box") {
        if let Some(shape) =
            preview_3d_shape_from_named_geometry(repository, "geometry.shulker.v1.8")
                .filter(|shape| !shape.is_empty())
        {
            return Some(shape);
        }
    }
    let query = preview_3d_block_state_query(state);
    let resolved = repository.resolve_block(&query);
    if resolved
        .warnings
        .iter()
        .any(|warning| matches!(warning, ModelWarning::MissingBlockDefinition(_)))
    {
        return None;
    }
    let geometry = resolved.geometry.as_ref()?;
    preview_3d_shape_from_block_geometry(geometry)
        .filter(|shape| !shape.is_empty())
        .filter(|shape| !preview_3d_shape_is_full_cube(shape))
}

fn preview_3d_block_state_query(state: &BlockState) -> BlockStateQuery {
    let mut query = BlockStateQuery::new(state.name.clone());
    for (key, value) in &state.states {
        if let Some(value) = preview_3d_block_state_value(value) {
            query = query.with_state(key.clone(), value);
        }
    }
    let canonical_name = canonical_block_name_for_state(&query);
    query.name = canonical_name;
    query
}

fn preview_3d_block_state_value(value: &NbtTag) -> Option<BlockStateValue> {
    match value {
        NbtTag::Byte(value) => Some(BlockStateValue::Int(i64::from(*value))),
        NbtTag::Short(value) => Some(BlockStateValue::Int(i64::from(*value))),
        NbtTag::Int(value) => Some(BlockStateValue::Int(i64::from(*value))),
        NbtTag::Long(value) => Some(BlockStateValue::Int(*value)),
        NbtTag::String(value) => Some(BlockStateValue::String(value.clone())),
        _ => None,
    }
}

fn preview_3d_shape_from_block_geometry(geometry: &BlockGeometry) -> Option<Preview3dDetailShape> {
    let mut shape = Preview3dDetailShape::default();
    for bone in &geometry.bones {
        preview_3d_push_bone_geometry(bone, &mut shape);
    }
    Some(shape).filter(|shape| !shape.is_empty())
}

fn preview_3d_shape_from_named_geometry(
    repository: &BlockModelRepository,
    identifier: &str,
) -> Option<Preview3dDetailShape> {
    let geometry = repository.geometries.get(identifier)?;
    let mut shape = preview_3d_shape_from_block_geometry(geometry)?;
    if identifier == "geometry.shulker.v1.8" {
        preview_3d_normalize_shulker_geometry_shape(&mut shape);
    }
    Some(shape).filter(|shape| !shape.is_empty())
}

fn preview_3d_normalize_shulker_geometry_shape(shape: &mut Preview3dDetailShape) {
    shape.cuboids.retain(|cuboid| {
        let width = cuboid.max[0] - cuboid.min[0];
        let height = cuboid.max[1] - cuboid.min[1];
        let depth = cuboid.max[2] - cuboid.min[2];
        !(width <= 0.4 && height <= 0.4 && depth <= 0.4)
    });
    for cuboid in &mut shape.cuboids {
        *cuboid = preview_3d_detail_cuboid_with_local_uv(cuboid.clone())
            .with_face_material_slot(BlockFace::Down, "down")
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Side, "side");
    }
    shape.planes.clear();
}

fn preview_3d_push_bone_geometry(bone: &GeometryBone, shape: &mut Preview3dDetailShape) {
    for cube in &bone.cubes {
        preview_3d_push_geometry_cube(bone, cube, shape);
    }
}

fn preview_3d_push_geometry_cube(
    bone: &GeometryBone,
    cube: &GeometryCube,
    shape: &mut Preview3dDetailShape,
) {
    let Some(origin) = cube.origin else {
        return;
    };
    let Some(size) = cube.size else {
        return;
    };
    let raw_min = preview_3d_geometry_point_to_block(origin);
    let raw_max = preview_3d_geometry_point_to_block([
        origin[0] + size[0],
        origin[1] + size[1],
        origin[2] + size[2],
    ]);
    let min = [
        raw_min[0].min(raw_max[0]),
        raw_min[1].min(raw_max[1]),
        raw_min[2].min(raw_max[2]),
    ];
    let max = [
        raw_min[0].max(raw_max[0]),
        raw_min[1].max(raw_max[1]),
        raw_min[2].max(raw_max[2]),
    ];
    let mut cuboid = Preview3dCuboid::new(min, max).with_material_slots(
        preview_3d_geometry_material_slot(cube.material_instance.as_deref()),
        preview_3d_geometry_face_material_slots(cube),
    );
    if let Some(face_uvs) = preview_3d_geometry_cube_face_uvs(cube, size) {
        cuboid.face_uvs = face_uvs;
    }
    let rotation = cube.rotation.or(bone.rotation).unwrap_or([0.0, 0.0, 0.0]);
    if preview_3d_rotation_is_zero(rotation) {
        shape.cuboids.push(cuboid);
        return;
    }
    let pivot = cube.pivot.or(bone.pivot).unwrap_or([0.0, 8.0, 0.0]);
    shape.planes.extend(preview_3d_rotated_cuboid_planes(
        cuboid,
        preview_3d_geometry_point_to_block(pivot),
        rotation,
    ));
}

fn preview_3d_geometry_point_to_block(point: [f32; 3]) -> [f32; 3] {
    [
        (point[0] + 8.0) / 16.0,
        point[1] / 16.0,
        (point[2] + 8.0) / 16.0,
    ]
}

fn preview_3d_rotation_is_zero(rotation: [f32; 3]) -> bool {
    rotation.iter().all(|value| value.abs() < 0.001)
}

fn preview_3d_rotated_cuboid_planes(
    cuboid: Preview3dCuboid,
    pivot: [f32; 3],
    rotation_degrees: [f32; 3],
) -> Vec<Preview3dPlane> {
    let [x0, y0, z0] = cuboid.min;
    let [x1, y1, z1] = cuboid.max;
    let points = [
        [x0, y0, z0],
        [x1, y0, z0],
        [x1, y1, z0],
        [x0, y1, z0],
        [x0, y0, z1],
        [x1, y0, z1],
        [x1, y1, z1],
        [x0, y1, z1],
    ]
    .map(|point| preview_3d_rotate_point(point, pivot, rotation_degrees));
    let plane_indices = [
        ([0, 1, 2, 3], [0, 0, -1]),
        ([5, 4, 7, 6], [0, 0, 1]),
        ([4, 0, 3, 7], [-1, 0, 0]),
        ([1, 5, 6, 2], [1, 0, 0]),
        ([3, 2, 6, 7], [0, 1, 0]),
        ([4, 5, 1, 0], [0, -1, 0]),
    ];
    plane_indices
        .into_iter()
        .map(|(indices, normal)| Preview3dPlane {
            corners: indices.map(|index| points[index]),
            normal: preview_3d_rotated_axis_normal(normal, rotation_degrees),
            material_slot: cuboid.material_slot_for_normal(normal),
            uv: cuboid.face_uv_for_normal(normal),
        })
        .collect()
}

fn preview_3d_geometry_material_slot(slot: Option<&str>) -> Option<Preview3dMaterialSlot> {
    slot.and_then(preview_3d_material_slot_from_value)
}

fn preview_3d_geometry_face_material_slots(
    cube: &GeometryCube,
) -> BTreeMap<BlockFace, Preview3dMaterialSlot> {
    cube.face_material_instances
        .iter()
        .filter_map(|(face, slot)| {
            preview_3d_material_slot_from_value(slot).map(|slot| (*face, slot))
        })
        .collect()
}

fn preview_3d_geometry_cube_face_uvs(
    cube: &GeometryCube,
    size: [f32; 3],
) -> Option<BTreeMap<BlockFace, [[f32; 2]; 4]>> {
    let uv = cube.uv.as_ref()?;
    let raw = uv.raw.as_array()?;
    let [u, v] = [raw.first()?.as_f64()? as f32, raw.get(1)?.as_f64()? as f32];
    let width = size[0].abs();
    let height = size[1].abs();
    let depth = size[2].abs();
    let texture_span = 64.0_f32;

    let mut face_uvs = BTreeMap::new();
    face_uvs.insert(
        BlockFace::Up,
        preview_3d_uv_pixels(texture_span, u + depth, v, u + depth + width, v + depth),
    );
    face_uvs.insert(
        BlockFace::Down,
        preview_3d_uv_pixels(
            texture_span,
            u + depth + width,
            v,
            u + depth + width + width,
            v + depth,
        ),
    );
    face_uvs.insert(
        BlockFace::North,
        preview_3d_uv_pixels(
            texture_span,
            u + depth,
            v + depth,
            u + depth + width,
            v + depth + height,
        ),
    );
    face_uvs.insert(
        BlockFace::South,
        preview_3d_uv_pixels(
            texture_span,
            u + depth + width + depth,
            v + depth,
            u + depth + width + depth + width,
            v + depth + height,
        ),
    );
    face_uvs.insert(
        BlockFace::West,
        preview_3d_uv_pixels(texture_span, u, v + depth, u + depth, v + depth + height),
    );
    face_uvs.insert(
        BlockFace::East,
        preview_3d_uv_pixels(
            texture_span,
            u + depth + width,
            v + depth,
            u + depth + width + depth,
            v + depth + height,
        ),
    );
    Some(face_uvs)
}

fn preview_3d_uv_pixels(texture_span: f32, u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    preview_3d_rect_uv(
        u0 / texture_span,
        v0 / texture_span,
        u1 / texture_span,
        v1 / texture_span,
    )
}

fn preview_3d_material_slot_from_value(value: &str) -> Option<Preview3dMaterialSlot> {
    let value = value.trim();
    (!value.is_empty()).then(|| Arc::from(value))
}

fn preview_3d_rotate_point(
    point: [f32; 3],
    pivot: [f32; 3],
    rotation_degrees: [f32; 3],
) -> [f32; 3] {
    let mut point = vec3_sub(point, pivot);
    for (axis, degrees) in rotation_degrees.into_iter().enumerate() {
        point = preview_3d_rotate_point_axis(point, axis, degrees.to_radians());
    }
    vec3_add(point, pivot)
}

fn preview_3d_rotate_point_axis(point: [f32; 3], axis: usize, angle: f32) -> [f32; 3] {
    if angle.abs() < 0.0001 {
        return point;
    }
    let (sin, cos) = angle.sin_cos();
    match axis {
        0 => [
            point[0],
            point[1] * cos - point[2] * sin,
            point[1] * sin + point[2] * cos,
        ],
        1 => [
            point[0] * cos + point[2] * sin,
            point[1],
            -point[0] * sin + point[2] * cos,
        ],
        2 => [
            point[0] * cos - point[1] * sin,
            point[0] * sin + point[1] * cos,
            point[2],
        ],
        _ => point,
    }
}

fn preview_3d_rotated_axis_normal(normal: [i32; 3], rotation_degrees: [f32; 3]) -> [i32; 3] {
    let rotated = preview_3d_rotate_point(
        [normal[0] as f32, normal[1] as f32, normal[2] as f32],
        [0.0, 0.0, 0.0],
        rotation_degrees,
    );
    preview_3d_nearest_axis_normal(rotated)
}

fn preview_3d_nearest_axis_normal(normal: [f32; 3]) -> [i32; 3] {
    let axis = (0..3)
        .max_by(|left, right| {
            normal[*left]
                .abs()
                .partial_cmp(&normal[*right].abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(1);
    let mut result = [0, 0, 0];
    result[axis] = if normal[axis].is_sign_negative() {
        -1
    } else {
        1
    };
    result
}

fn preview_3d_shape_is_full_cube(shape: &Preview3dDetailShape) -> bool {
    shape.planes.is_empty()
        && shape.cuboids.len() == 1
        && shape.cuboids.first().is_some_and(|cuboid| {
            preview_3d_nearly_eq(cuboid.min, [0.0, 0.0, 0.0])
                && preview_3d_nearly_eq(cuboid.max, [1.0, 1.0, 1.0])
        })
}

fn preview_3d_nearly_eq(left: [f32; 3], right: [f32; 3]) -> bool {
    left.iter()
        .zip(right)
        .all(|(left, right)| (*left - right).abs() < 0.001)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Preview3dCardinalDirection {
    North,
    South,
    East,
    West,
}

impl Preview3dCardinalDirection {
    const ALL: [Self; 4] = [Self::North, Self::South, Self::East, Self::West];

    const fn state_key(self) -> &'static str {
        match self {
            Self::North => "north",
            Self::South => "south",
            Self::East => "east",
            Self::West => "west",
        }
    }

    const fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::South => Self::North,
            Self::East => Self::West,
            Self::West => Self::East,
        }
    }

    const fn normal(self) -> [i32; 3] {
        match self {
            Self::North => [0, 0, -1],
            Self::South => [0, 0, 1],
            Self::East => [1, 0, 0],
            Self::West => [-1, 0, 0],
        }
    }
}

fn preview_3d_cardinal_direction(state: &BlockState) -> Option<Preview3dCardinalDirection> {
    preview_3d_state_string(state, "minecraft:cardinal_direction")
        .and_then(preview_3d_cardinal_direction_from_string)
        .or_else(|| {
            preview_3d_state_string(state, "facing")
                .and_then(preview_3d_cardinal_direction_from_string)
        })
        .or_else(|| {
            preview_3d_state_string(state, "facing_direction")
                .and_then(preview_3d_cardinal_direction_from_string)
        })
        .or_else(|| {
            preview_3d_block_face(state).and_then(preview_3d_cardinal_direction_from_string)
        })
        .or_else(|| {
            preview_3d_state_i32(state, "facing_direction")
                .and_then(preview_3d_facing_direction_from_int)
        })
        .or_else(|| {
            preview_3d_state_i32(state, "weirdo_direction")
                .and_then(preview_3d_cardinal_direction_from_int)
        })
        .or_else(|| {
            preview_3d_state_i32(state, "direction")
                .and_then(preview_3d_cardinal_direction_from_int)
        })
}

fn preview_3d_cardinal_direction_from_string(value: &str) -> Option<Preview3dCardinalDirection> {
    match value {
        "north" => Some(Preview3dCardinalDirection::North),
        "south" => Some(Preview3dCardinalDirection::South),
        "east" => Some(Preview3dCardinalDirection::East),
        "west" => Some(Preview3dCardinalDirection::West),
        _ => None,
    }
}

fn preview_3d_block_face<'a>(state: &'a BlockState) -> Option<&'a str> {
    preview_3d_state_string(state, "minecraft:block_face")
        .or_else(|| preview_3d_state_string(state, "block_face"))
        .or_else(|| preview_3d_state_string(state, "torch_facing_direction"))
}

fn preview_3d_should_infer_detail_connections(state: &BlockState) -> bool {
    let normalized = preview_3d_normalized_block_name(&state.name);
    preview_3d_is_pane_like_block(&normalized) && !preview_3d_has_direction_connection_state(state)
}

fn preview_3d_has_direction_connection_state(state: &BlockState) -> bool {
    Preview3dCardinalDirection::ALL
        .into_iter()
        .any(|direction| preview_3d_has_direction_connection_state_for(state, direction))
}

fn preview_3d_has_direction_connection_state_for(
    state: &BlockState,
    direction: Preview3dCardinalDirection,
) -> bool {
    let key = direction.state_key();
    preview_3d_state_tag(state, &format!("{key}_connection_type")).is_some()
        || preview_3d_state_tag(state, &format!("wall_connection_type_{key}")).is_some()
        || preview_3d_state_tag(state, key).is_some()
        || preview_3d_state_tag(state, &format!("{key}_bit")).is_some()
        || preview_3d_state_tag(state, &format!("connected_{key}")).is_some()
        || preview_3d_state_tag(state, &format!("{key}_connection_bit")).is_some()
        || preview_3d_state_tag(state, &format!("{key}_wall_bit")).is_some()
}

fn preview_3d_normalized_block_name(name: &str) -> String {
    name.strip_prefix("minecraft:").unwrap_or(name).to_string()
}

fn preview_3d_is_pane_like_block(normalized: &str) -> bool {
    normalized == "iron_bars"
        || normalized.ends_with("_pane")
        || normalized == "pane"
        || normalized.ends_with("_glass_pane")
}

fn preview_3d_detail_block_connects_to_panes(normalized: &str) -> bool {
    preview_3d_is_pane_like_block(normalized) || preview_3d_is_glass_block(normalized)
}

fn preview_3d_cardinal_direction_from_int(value: i32) -> Option<Preview3dCardinalDirection> {
    match value.rem_euclid(4) {
        0 => Some(Preview3dCardinalDirection::South),
        1 => Some(Preview3dCardinalDirection::West),
        2 => Some(Preview3dCardinalDirection::North),
        3 => Some(Preview3dCardinalDirection::East),
        _ => None,
    }
}

fn preview_3d_facing_direction_from_int(value: i32) -> Option<Preview3dCardinalDirection> {
    match value {
        2 => Some(Preview3dCardinalDirection::North),
        3 => Some(Preview3dCardinalDirection::South),
        4 => Some(Preview3dCardinalDirection::West),
        5 => Some(Preview3dCardinalDirection::East),
        _ => None,
    }
}

fn preview_3d_state_tag<'a>(state: &'a BlockState, key: &str) -> Option<&'a NbtTag> {
    state
        .states
        .get(key)
        .or_else(|| state.states.get(&format!("minecraft:{key}")))
}

fn preview_3d_state_string<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    match preview_3d_state_tag(state, key)? {
        NbtTag::String(value) => Some(value),
        _ => None,
    }
}

fn preview_3d_state_i32(state: &BlockState, key: &str) -> Option<i32> {
    match preview_3d_state_tag(state, key)? {
        NbtTag::Byte(value) => Some(i32::from(*value)),
        NbtTag::Short(value) => Some(i32::from(*value)),
        NbtTag::Int(value) => Some(*value),
        NbtTag::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn preview_3d_state_bool(state: &BlockState, key: &str) -> Option<bool> {
    match preview_3d_state_tag(state, key)? {
        NbtTag::Byte(value) => Some(*value != 0),
        NbtTag::Short(value) => Some(*value != 0),
        NbtTag::Int(value) => Some(*value != 0),
        NbtTag::Long(value) => Some(*value != 0),
        NbtTag::String(value) => match value.as_str() {
            "true" | "top" | "upper" | "up" => Some(true),
            "false" | "bottom" | "lower" | "down" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct Preview3dBiomeSample {
    biome_id: Option<u32>,
    legacy_color: Option<RgbaColor>,
}

fn preview_3d_color_for_block(
    palette: &RenderPalette,
    state: &BlockState,
    biome: Option<Preview3dBiomeSample>,
) -> [f32; 4] {
    if state.name.strip_prefix("minecraft:").unwrap_or(&state.name) == "redstone_wire" {
        return preview_3d_redstone_wire_color(state);
    }
    let color = preview_3d_surface_color_for_block(palette, state.name.as_str(), biome).to_array();
    [
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        1.0,
    ]
}

fn preview_3d_redstone_wire_color(state: &BlockState) -> [f32; 4] {
    let power = preview_3d_state_i32(state, "redstone_signal")
        .or_else(|| preview_3d_state_i32(state, "power"))
        .unwrap_or(0)
        .clamp(0, 15);
    let strength = power as f32 / 15.0;
    let red = if power == 0 {
        0.30
    } else {
        strength.mul_add(0.60, 0.40)
    };
    let green = (strength * strength * 0.70 - 0.50).max(0.0);
    let blue = (strength * strength * 0.60 - 0.70).max(0.0);
    [red, green, blue, 1.0]
}

fn preview_3d_face_colors_for_block(
    palette: &RenderPalette,
    state: &BlockState,
    biome: Option<Preview3dBiomeSample>,
) -> Preview3dBlockFaceColors {
    let normalized = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    if preview_3d_is_full_grass_block(normalized) {
        let up = preview_3d_color_for_named_block(palette, "minecraft:grass_block", biome, true);
        let down = preview_3d_color_for_named_block(palette, "minecraft:dirt", biome, false);
        let side = preview_3d_color_for_named_block(palette, "minecraft:grass", biome, true);
        return Preview3dBlockFaceColors { up, down, side };
    }
    Preview3dBlockFaceColors::uniform(preview_3d_color_for_block(palette, state, biome))
}

fn preview_3d_color_for_named_block(
    palette: &RenderPalette,
    name: &str,
    biome: Option<Preview3dBiomeSample>,
    biome_tint: bool,
) -> [f32; 4] {
    let biome_id = biome.and_then(|biome| biome.biome_id);
    let mut color = if biome_tint {
        if let Some(legacy_color) = biome.and_then(|biome| biome.legacy_color) {
            preview_3d_multiply_with_biome_tint(palette.block_color(name), legacy_color)
        } else {
            palette.surface_block_color(name, biome_id, true)
        }
    } else {
        palette.block_color(name)
    };
    if biome_tint {
        color = preview_3d_correct_washed_out_biome_tint(palette, name, biome_id, color);
    }
    let color = color.to_array();
    [
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        1.0,
    ]
}

fn preview_3d_surface_color_for_block(
    palette: &RenderPalette,
    name: &str,
    biome: Option<Preview3dBiomeSample>,
) -> RgbaColor {
    let normalized = name.strip_prefix("minecraft:").unwrap_or(name);
    if let Some(legacy_color) = biome.and_then(|biome| biome.legacy_color) {
        if preview_3d_is_full_grass_block(normalized) {
            return preview_3d_apply_surface_grass_tint(
                palette.block_color("minecraft:grass_block"),
                legacy_color,
            );
        }
        if preview_3d_is_foliage_block(normalized) {
            return preview_3d_multiply_with_biome_tint(palette.block_color(name), legacy_color);
        }
    }
    let biome_id = biome.and_then(|biome| biome.biome_id);
    let color_name = if preview_3d_is_full_grass_block(normalized) {
        "minecraft:grass_block"
    } else {
        name
    };
    let color = palette.surface_block_color(color_name, biome_id, true);
    preview_3d_correct_washed_out_biome_tint(palette, color_name, biome_id, color)
}

fn preview_3d_is_full_grass_block(normalized: &str) -> bool {
    normalized == "grass" || normalized == "grass_block" || normalized.ends_with("_grass_block")
}

fn preview_3d_is_grass_tinted_detail_block(normalized: &str) -> bool {
    matches!(
        normalized,
        "short_grass"
            | "tall_grass"
            | "tallgrass"
            | "fern"
            | "large_fern"
            | "vine"
            | "twisting_vines"
            | "weeping_vines"
            | "seagrass"
            | "tall_seagrass"
            | "kelp"
            | "kelp_plant"
    ) || normalized.contains("grass")
        || normalized.contains("fern")
        || normalized.contains("vine")
}

fn preview_3d_correct_washed_out_biome_tint(
    palette: &RenderPalette,
    name: &str,
    biome_id: Option<u32>,
    color: RgbaColor,
) -> RgbaColor {
    let normalized = name.strip_prefix("minecraft:").unwrap_or(name);
    if !preview_3d_color_is_washed_out(color) {
        return color;
    }
    if preview_3d_is_full_grass_block(normalized)
        || preview_3d_is_grass_tinted_detail_block(normalized)
    {
        return palette.surface_block_color("minecraft:grass_block", biome_id, true);
    }
    if preview_3d_is_foliage_block(normalized) {
        return palette.surface_block_color("minecraft:oak_leaves", biome_id, true);
    }
    color
}

fn preview_3d_color_is_washed_out(color: RgbaColor) -> bool {
    let color = color.to_array();
    let red = f32::from(color[0]) / 255.0;
    let green = f32::from(color[1]) / 255.0;
    let blue = f32::from(color[2]) / 255.0;
    let maximum = red.max(green).max(blue);
    let minimum = red.min(green).min(blue);
    let luminance = red * 0.2126 + green * 0.7152 + blue * 0.0722;
    luminance > 0.78 || (maximum > 0.72 && maximum - minimum < 0.18)
}

fn preview_3d_transparent_color_for_block(
    palette: &RenderPalette,
    state: &BlockState,
    biome: Option<Preview3dBiomeSample>,
) -> [f32; 4] {
    let mut color = preview_3d_color_for_block(palette, state, biome);
    color[3] = PREVIEW_3D_GLASS_ALPHA;
    color
}

fn preview_3d_water_color_for_block(
    palette: &RenderPalette,
    state: &BlockState,
    biome: Option<Preview3dBiomeSample>,
) -> [f32; 4] {
    let biome_id = biome.and_then(|biome| biome.biome_id);
    let mut color = preview_3d_color_from_rgba(
        palette.surface_block_color(state.name.as_str(), biome_id, true),
        PREVIEW_3D_WATER_ALPHA,
    );
    if preview_3d_water_color_is_washed_out(color) {
        color = [
            PREVIEW_3D_DEFAULT_WATER_RGB[0],
            PREVIEW_3D_DEFAULT_WATER_RGB[1],
            PREVIEW_3D_DEFAULT_WATER_RGB[2],
            PREVIEW_3D_WATER_ALPHA,
        ];
    }
    color[3] = PREVIEW_3D_WATER_ALPHA;
    color
}

fn preview_3d_color_from_rgba(color: RgbaColor, alpha: f32) -> [f32; 4] {
    let color = color.to_array();
    [
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        alpha,
    ]
}

fn preview_3d_water_color_is_washed_out(color: [f32; 4]) -> bool {
    let maximum = color[0].max(color[1]).max(color[2]);
    let minimum = color[0].min(color[1]).min(color[2]);
    let luminance = preview_3d_luminance(color);
    luminance > 0.66 || (maximum > 0.72 && maximum - minimum < 0.24)
}

fn preview_3d_luminance(color: [f32; 4]) -> f32 {
    color[0] * 0.2126 + color[1] * 0.7152 + color[2] * 0.0722
}

fn preview_3d_lava_color_for_block(
    palette: &RenderPalette,
    state: &BlockState,
    biome: Option<Preview3dBiomeSample>,
) -> [f32; 4] {
    let mut color = preview_3d_color_for_block(palette, state, biome);
    color[3] = PREVIEW_3D_LAVA_ALPHA;
    color
}

fn preview_3d_material_block_name_for_state<'a>(
    state: &'a BlockState,
    block_class: Preview3dBlockClass,
) -> Cow<'a, str> {
    match block_class {
        Preview3dBlockClass::Water => Cow::Borrowed("minecraft:water"),
        Preview3dBlockClass::Lava => Cow::Borrowed("minecraft:lava"),
        _ => detail_material_block_name_for_state(&preview_3d_block_state_query(state))
            .unwrap_or_else(|| Cow::Borrowed(&state.name)),
    }
}

fn preview_3d_biome_at_or_top(
    chunk: &RenderChunkData,
    local_x: u8,
    local_z: u8,
    y: i32,
) -> Option<Preview3dBiomeSample> {
    if let Some(biome) = chunk
        .column_sample_at(local_x, local_z)
        .and_then(|sample| sample.biome)
    {
        return Some(preview_3d_terrain_biome_sample(biome));
    }
    if let Some(samples) = chunk.legacy_biomes.as_ref() {
        if let Some(sample) = samples[usize::from(local_z)][usize::from(local_x)] {
            return Some(Preview3dBiomeSample {
                biome_id: Some(u32::from(sample.biome_id)),
                legacy_color: Some(RgbaColor::new(sample.red, sample.green, sample.blue, 255)),
            });
        }
    }
    preview_3d_biome_id_at_or_top(chunk, local_x, local_z, y).map(|biome_id| Preview3dBiomeSample {
        biome_id: Some(biome_id),
        legacy_color: None,
    })
}

fn preview_3d_terrain_biome_sample(biome: TerrainColumnBiome) -> Preview3dBiomeSample {
    match biome {
        TerrainColumnBiome::Id(biome_id) => Preview3dBiomeSample {
            biome_id: Some(biome_id),
            legacy_color: None,
        },
        TerrainColumnBiome::Legacy(sample) => Preview3dBiomeSample {
            biome_id: Some(u32::from(sample.biome_id)),
            legacy_color: Some(RgbaColor::new(sample.red, sample.green, sample.blue, 255)),
        },
    }
}

fn preview_3d_biome_id_at_or_top(
    chunk: &RenderChunkData,
    local_x: u8,
    local_z: u8,
    y: i32,
) -> Option<u32> {
    preview_3d_biome_id_at(chunk, local_x, local_z, y).or_else(|| {
        chunk.biome_data.values().rev().find_map(|storage| {
            if storage.y.is_none() {
                return preview_3d_non_empty_biome_id(storage.biome_id_at(local_x, 0, local_z));
            }
            (0..16_u8).rev().find_map(|local_y| {
                preview_3d_non_empty_biome_id(storage.biome_id_at(local_x, local_y, local_z))
            })
        })
    })
}

fn preview_3d_biome_id_at(
    chunk: &RenderChunkData,
    local_x: u8,
    local_z: u8,
    y: i32,
) -> Option<u32> {
    chunk
        .biome_data
        .get(&preview_3d_biome_storage_bucket_y(y))
        .or_else(|| chunk.biome_data.values().next())
        .and_then(|storage| {
            preview_3d_non_empty_biome_id(preview_3d_biome_id_from_storage(
                storage, local_x, local_z, y,
            ))
        })
}

fn preview_3d_biome_id_from_storage(
    storage: &ParsedBiomeStorage,
    local_x: u8,
    local_z: u8,
    y: i32,
) -> Option<u32> {
    let local_y = if let Some(start_y) = storage.y {
        u8::try_from(y - start_y).ok()?
    } else {
        0
    };
    storage.biome_id_at(local_x, local_y, local_z)
}

fn preview_3d_biome_storage_bucket_y(y: i32) -> i32 {
    y.div_euclid(16) * 16
}

fn preview_3d_non_empty_biome_id(id: Option<u32>) -> Option<u32> {
    id.filter(|id| *id != 0 && *id != u32::MAX)
}

fn preview_3d_apply_surface_grass_tint(base: RgbaColor, tint: RgbaColor) -> RgbaColor {
    let multiplied = preview_3d_multiply_with_biome_tint(base, tint);
    preview_3d_blend_toward_color(multiplied, tint, 96)
}

fn preview_3d_multiply_with_biome_tint(base: RgbaColor, tint: RgbaColor) -> RgbaColor {
    RgbaColor::new(
        preview_3d_multiply_channel(base.red, tint.red),
        preview_3d_multiply_channel(base.green, tint.green),
        preview_3d_multiply_channel(base.blue, tint.blue),
        255,
    )
}

fn preview_3d_multiply_channel(base: u8, tint: u8) -> u8 {
    let value = u16::from(base) * u16::from(tint) / 255;
    u8::try_from(value).unwrap_or(u8::MAX)
}

fn preview_3d_blend_toward_color(
    base: RgbaColor,
    target: RgbaColor,
    target_weight: u16,
) -> RgbaColor {
    let target_weight = target_weight.min(255);
    let base_weight = 255_u16.saturating_sub(target_weight);
    RgbaColor::new(
        preview_3d_weighted_channel(base.red, target.red, base_weight, target_weight),
        preview_3d_weighted_channel(base.green, target.green, base_weight, target_weight),
        preview_3d_weighted_channel(base.blue, target.blue, base_weight, target_weight),
        255,
    )
}

fn preview_3d_weighted_channel(base: u8, target: u8, base_weight: u16, target_weight: u16) -> u8 {
    let value = (u16::from(base) * base_weight + u16::from(target) * target_weight + 127) / 255;
    u8::try_from(value).unwrap_or(u8::MAX)
}

fn preview_3d_material_name_for_block(
    name: &str,
    block_class: Preview3dBlockClass,
) -> Preview3dMaterialName {
    let block = match block_class {
        Preview3dBlockClass::Water => "minecraft:water",
        Preview3dBlockClass::Lava => "minecraft:lava",
        _ => name,
    };
    Arc::from(block_export_material_name_for_block(block))
}

fn preview_3d_material_name_for_face(
    base: Preview3dMaterialName,
    normal: [i32; 3],
) -> Preview3dMaterialName {
    Arc::from(block_export_material_name_for_face(base.as_ref(), normal))
}

fn preview_3d_material_name_for_plane(
    base: Preview3dMaterialName,
    plane: &Preview3dPlane,
) -> Preview3dMaterialName {
    Arc::from(block_export_material_name_for_plane(
        base.as_ref(),
        plane.normal,
        plane.material_slot.as_deref(),
    ))
}

fn preview_3d_fit_scale(horizontal_span: f32, vertical_span: f32) -> f32 {
    let fitted_span = horizontal_span.max(vertical_span * PREVIEW_3D_VERTICAL_SCALE * 1.25);
    1.48 / fitted_span.max(1.0)
}

fn build_preview_3d_gpu_meshes(
    opaque_faces: &[Preview3dFace],
    glass_faces: &[Preview3dFace],
    water_faces: &[Preview3dFace],
    lava_faces: &[Preview3dFace],
    center: [f32; 3],
    horizontal_span: f32,
    vertical_span: f32,
    generation: u64,
) -> Result<Vec<Preview3dChunkMesh>, String> {
    let fit_scale = preview_3d_fit_scale(horizontal_span, vertical_span);
    let mut meshes = Vec::new();
    for slice in preview_3d_face_slices(opaque_faces, glass_faces, water_faces, lava_faces) {
        meshes.push(build_preview_3d_gpu_mesh_from_slices(
            slice.opaque,
            slice.glass,
            slice.water,
            slice.lava,
            center,
            fit_scale,
            generation,
        )?);
    }
    Ok(meshes)
}

fn preview_3d_face_slices<'a>(
    opaque_faces: &'a [Preview3dFace],
    glass_faces: &'a [Preview3dFace],
    water_faces: &'a [Preview3dFace],
    lava_faces: &'a [Preview3dFace],
) -> Vec<Preview3dFaceSlice<'a>> {
    let max_faces = PREVIEW_3D_FACE_BUDGET.max(1);
    let mut slices = Vec::new();
    let mut opaque_start = 0;
    let mut glass_start = 0;
    let mut water_start = 0;
    let mut lava_start = 0;
    while opaque_start < opaque_faces.len()
        || glass_start < glass_faces.len()
        || water_start < water_faces.len()
        || lava_start < lava_faces.len()
    {
        let opaque_remaining = opaque_faces.len() - opaque_start;
        let glass_remaining = glass_faces.len() - glass_start;
        let water_remaining = water_faces.len() - water_start;
        let lava_remaining = lava_faces.len() - lava_start;
        let mut opaque_count = opaque_remaining.min(PREVIEW_3D_OPAQUE_FACE_BUDGET.max(1));
        let mut glass_count = glass_remaining.min(PREVIEW_3D_GLASS_FACE_BUDGET.max(1));
        let fluid_budget = PREVIEW_3D_WATER_FACE_BUDGET.max(2);
        let mut water_count = water_remaining.min(fluid_budget / 2);
        let mut lava_count = lava_remaining.min(fluid_budget - water_count);
        let used_faces = opaque_count
            .saturating_add(glass_count)
            .saturating_add(water_count)
            .saturating_add(lava_count);
        let mut remaining_faces = max_faces.saturating_sub(used_faces);
        if remaining_faces > 0 {
            let extra = (opaque_remaining - opaque_count).min(remaining_faces);
            opaque_count += extra;
            remaining_faces -= extra;
        }
        if remaining_faces > 0 {
            let extra = (glass_remaining - glass_count).min(remaining_faces);
            glass_count += extra;
            remaining_faces -= extra;
        }
        if remaining_faces > 0 {
            let extra = (water_remaining - water_count).min(remaining_faces);
            water_count += extra;
            remaining_faces -= extra;
        }
        if remaining_faces > 0 {
            let extra = (lava_remaining - lava_count).min(remaining_faces);
            lava_count += extra;
        }

        slices.push(Preview3dFaceSlice {
            opaque: &opaque_faces[opaque_start..opaque_start + opaque_count],
            glass: &glass_faces[glass_start..glass_start + glass_count],
            water: &water_faces[water_start..water_start + water_count],
            lava: &lava_faces[lava_start..lava_start + lava_count],
        });
        opaque_start += opaque_count;
        glass_start += glass_count;
        water_start += water_count;
        lava_start += lava_count;
    }
    slices
}

struct Preview3dFaceSlice<'a> {
    opaque: &'a [Preview3dFace],
    glass: &'a [Preview3dFace],
    water: &'a [Preview3dFace],
    lava: &'a [Preview3dFace],
}

#[cfg(test)]
fn build_preview_3d_gpu_mesh(
    opaque_faces: &[Preview3dFace],
    glass_faces: &[Preview3dFace],
    water_faces: &[Preview3dFace],
    center: [f32; 3],
    horizontal_span: f32,
    vertical_span: f32,
    generation: u64,
) -> Result<GpuMesh3d, String> {
    let fit_scale = preview_3d_fit_scale(horizontal_span, vertical_span);
    Ok(build_preview_3d_gpu_mesh_from_slices(
        opaque_faces,
        glass_faces,
        water_faces,
        &[],
        center,
        fit_scale,
        generation,
    )
    .map(|chunk_mesh| chunk_mesh.gpu_mesh.as_ref().clone())?)
}

fn build_preview_3d_gpu_mesh_from_slices(
    opaque_faces: &[Preview3dFace],
    glass_faces: &[Preview3dFace],
    water_faces: &[Preview3dFace],
    lava_faces: &[Preview3dFace],
    center: [f32; 3],
    fit_scale: f32,
    generation: u64,
) -> Result<Preview3dChunkMesh, String> {
    let vertex_count = opaque_faces
        .len()
        .saturating_add(glass_faces.len())
        .saturating_add(water_faces.len())
        .saturating_add(lava_faces.len())
        .saturating_mul(6);
    if vertex_count > PREVIEW_3D_GPU_VERTEX_BUDGET {
        return Err(format!(
            "3D 预览网格过大: 顶点 {vertex_count}，预算 {PREVIEW_3D_GPU_VERTEX_BUDGET}"
        ));
    }
    let mut vertices = Vec::with_capacity(vertex_count);
    let mut face_materials = Vec::with_capacity(vertex_count / 6);
    let mut face_uvs = Vec::with_capacity(vertex_count / 6);
    let opaque = push_preview_gpu_faces(
        &mut vertices,
        &mut face_materials,
        &mut face_uvs,
        opaque_faces,
    );
    let glass = push_preview_gpu_faces(
        &mut vertices,
        &mut face_materials,
        &mut face_uvs,
        glass_faces,
    );
    let water = push_preview_gpu_fluid_faces(
        &mut vertices,
        &mut face_materials,
        &mut face_uvs,
        water_faces,
        lava_faces,
    );
    let gpu_mesh = GpuMesh3d::new(
        vertices,
        GpuMesh3dDrawRanges {
            opaque,
            glass,
            water,
        },
        center,
        fit_scale,
        PREVIEW_3D_VERTICAL_SCALE,
    )
    .with_generation(generation);
    Ok(Preview3dChunkMesh {
        gpu_mesh: Arc::new(gpu_mesh),
        face_materials: Arc::from(face_materials.into_boxed_slice()),
        face_uvs: Arc::from(face_uvs.into_boxed_slice()),
    })
}

fn push_preview_gpu_faces(
    vertices: &mut Vec<GpuMesh3dVertex>,
    face_materials: &mut Vec<Preview3dMaterialName>,
    face_uvs: &mut Vec<[[f32; 2]; 4]>,
    faces: &[Preview3dFace],
) -> GpuMesh3dRange {
    let start = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
    for face in faces {
        push_preview_gpu_face(vertices, face);
        face_materials.push(face.material.clone());
        face_uvs.push(
            face.uv
                .unwrap_or_else(|| default_block_face_uvs_from_corners(&face.corners)),
        );
    }
    let count = u32::try_from(vertices.len().saturating_sub(start as usize)).unwrap_or(u32::MAX);
    GpuMesh3dRange { start, count }
}

fn push_preview_gpu_fluid_faces(
    vertices: &mut Vec<GpuMesh3dVertex>,
    face_materials: &mut Vec<Preview3dMaterialName>,
    face_uvs: &mut Vec<[[f32; 2]; 4]>,
    water_faces: &[Preview3dFace],
    lava_faces: &[Preview3dFace],
) -> GpuMesh3dRange {
    let start = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
    push_preview_gpu_faces_matching(vertices, face_materials, face_uvs, water_faces, |face| {
        face.normal != [0, 1, 0]
    });
    push_preview_gpu_faces_matching(vertices, face_materials, face_uvs, lava_faces, |face| {
        face.normal != [0, 1, 0]
    });
    push_preview_gpu_faces_matching(vertices, face_materials, face_uvs, water_faces, |face| {
        face.normal == [0, 1, 0]
    });
    push_preview_gpu_faces_matching(vertices, face_materials, face_uvs, lava_faces, |face| {
        face.normal == [0, 1, 0]
    });
    let count = u32::try_from(vertices.len().saturating_sub(start as usize)).unwrap_or(u32::MAX);
    GpuMesh3dRange { start, count }
}

fn push_preview_gpu_faces_matching(
    vertices: &mut Vec<GpuMesh3dVertex>,
    face_materials: &mut Vec<Preview3dMaterialName>,
    face_uvs: &mut Vec<[[f32; 2]; 4]>,
    faces: &[Preview3dFace],
    mut predicate: impl FnMut(&Preview3dFace) -> bool,
) {
    for face in faces {
        if predicate(face) {
            push_preview_gpu_face(vertices, face);
            face_materials.push(face.material.clone());
            face_uvs.push(
                face.uv
                    .unwrap_or_else(|| default_block_face_uvs_from_corners(&face.corners)),
            );
        }
    }
}

fn push_preview_gpu_face(vertices: &mut Vec<GpuMesh3dVertex>, face: &Preview3dFace) {
    let color = shade_preview_color(face.color, face.shade);
    let vertex = |index: usize| GpuMesh3dVertex {
        position: face.corners[index],
        color,
    };
    vertices.extend([
        vertex(0),
        vertex(1),
        vertex(2),
        vertex(0),
        vertex(2),
        vertex(3),
    ]);
}

#[cfg(test)]
fn project_preview_point(
    point: [f32; 3],
    center: [f32; 3],
    scale: f32,
    camera: Preview3dCamera,
) -> (f32, f32, f32) {
    let view_proj_model = preview_3d_test_view_proj_model(center, scale, camera);
    let projected = mat4_mul_vec4(view_proj_model, [point[0], point[1], point[2], 1.0]);
    let reciprocal_w = 1.0 / projected[3].max(0.0001);
    (
        projected[0] * reciprocal_w,
        projected[1] * reciprocal_w,
        projected[2] * reciprocal_w,
    )
}

fn preview_3d_view_proj_model(
    aspect: f32,
    center: [f32; 3],
    scale: f32,
    camera: Preview3dCamera,
    model_rotation: Preview3dModelRotation,
) -> [[f32; 4]; 4] {
    let pitch = wrap_preview_3d_pitch(camera.pitch);
    let eye = camera.position;
    let look_at = vec3_add(eye, camera.forward());
    let view = mat4_look_at(eye, look_at, preview_3d_camera_up(pitch));
    let proj = mat4_perspective(
        aspect.max(0.1),
        PREVIEW_3D_BASE_FOV_Y_RADIANS,
        PREVIEW_3D_NEAR_PLANE,
        PREVIEW_3D_FAR_PLANE,
    );
    let zoom = camera.zoom.max(PREVIEW_3D_MIN_ZOOM);
    let rotation = mat4_mul(
        mat4_rotation_y(model_rotation.yaw),
        mat4_rotation_x(model_rotation.pitch),
    );
    let model = mat4_mul(
        mat4_scale([
            if model_rotation.mirror_x {
                -scale
            } else {
                scale
            } * zoom,
            scale * PREVIEW_3D_VERTICAL_SCALE * zoom,
            if model_rotation.mirror_z {
                -scale
            } else {
                scale
            } * zoom,
        ]),
        mat4_mul(
            rotation,
            mat4_translation([-center[0], -center[1], -center[2]]),
        ),
    );

    mat4_mul(mat4_mul(proj, view), model)
}

#[cfg(test)]
fn preview_3d_test_view_proj_model(
    center: [f32; 3],
    scale: f32,
    camera: Preview3dCamera,
) -> [[f32; 4]; 4] {
    preview_3d_view_proj_model(
        1.0,
        center,
        scale,
        camera,
        Preview3dModelRotation::default(),
    )
}

fn mat4_identity() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn mat4_translation(offset: [f32; 3]) -> [[f32; 4]; 4] {
    let mut matrix = mat4_identity();
    matrix[3][0] = offset[0];
    matrix[3][1] = offset[1];
    matrix[3][2] = offset[2];
    matrix
}

fn mat4_scale(scale: [f32; 3]) -> [[f32; 4]; 4] {
    [
        [scale[0], 0.0, 0.0, 0.0],
        [0.0, scale[1], 0.0, 0.0],
        [0.0, 0.0, scale[2], 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn mat4_rotation_x(angle: f32) -> [[f32; 4]; 4] {
    let (sin, cos) = angle.sin_cos();
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, cos, sin, 0.0],
        [0.0, -sin, cos, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn mat4_rotation_y(angle: f32) -> [[f32; 4]; 4] {
    let (sin, cos) = angle.sin_cos();
    [
        [cos, 0.0, -sin, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [sin, 0.0, cos, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn mat4_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            out[col][row] = a[0][row] * b[col][0]
                + a[1][row] * b[col][1]
                + a[2][row] * b[col][2]
                + a[3][row] * b[col][3];
        }
    }
    out
}

#[cfg(test)]
fn mat4_mul_vec4(matrix: [[f32; 4]; 4], value: [f32; 4]) -> [f32; 4] {
    [
        matrix[0][0] * value[0]
            + matrix[1][0] * value[1]
            + matrix[2][0] * value[2]
            + matrix[3][0] * value[3],
        matrix[0][1] * value[0]
            + matrix[1][1] * value[1]
            + matrix[2][1] * value[2]
            + matrix[3][1] * value[3],
        matrix[0][2] * value[0]
            + matrix[1][2] * value[1]
            + matrix[2][2] * value[2]
            + matrix[3][2] * value[3],
        matrix[0][3] * value[0]
            + matrix[1][3] * value[1]
            + matrix[2][3] * value[2]
            + matrix[3][3] * value[3],
    ]
}

fn mat4_perspective(aspect: f32, vertical_fov: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let focal_length = 1.0 / (vertical_fov * 0.5).tan().max(0.0001);
    let depth = near - far;
    [
        [focal_length / aspect.max(0.1), 0.0, 0.0, 0.0],
        [0.0, focal_length, 0.0, 0.0],
        [0.0, 0.0, (far + near) / depth, -1.0],
        [0.0, 0.0, (2.0 * far * near) / depth, 0.0],
    ]
}

fn mat4_look_at(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> [[f32; 4]; 4] {
    let forward = vec3_normalize(vec3_sub(center, eye));
    let side = vec3_normalize(vec3_cross(forward, up));
    let up = vec3_cross(side, forward);

    [
        [side[0], up[0], -forward[0], 0.0],
        [side[1], up[1], -forward[1], 0.0],
        [side[2], up[2], -forward[2], 0.0],
        [
            -vec3_dot(side, eye),
            -vec3_dot(up, eye),
            vec3_dot(forward, eye),
            1.0,
        ],
    ]
}

fn preview_3d_camera_up(pitch: f32) -> [f32; 3] {
    if pitch.cos().is_sign_negative() {
        [0.0, -1.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    }
}

fn preview_3d_camera_forward(yaw: f32, pitch: f32) -> [f32; 3] {
    let pitch = wrap_preview_3d_pitch(pitch);
    let horizontal_distance = pitch.cos();
    vec3_normalize([
        -yaw.sin() * horizontal_distance,
        -pitch.sin(),
        -yaw.cos() * horizontal_distance,
    ])
}

fn preview_3d_camera_right(yaw: f32) -> [f32; 3] {
    vec3_normalize([yaw.cos(), 0.0, -yaw.sin()])
}

fn preview_3d_default_camera_position(yaw: f32, pitch: f32) -> [f32; 3] {
    let pitch = wrap_preview_3d_pitch(pitch);
    let horizontal_distance = PREVIEW_3D_DEFAULT_DISTANCE * pitch.cos();
    [
        horizontal_distance * yaw.sin(),
        PREVIEW_3D_DEFAULT_DISTANCE * pitch.sin(),
        horizontal_distance * yaw.cos(),
    ]
}

fn vec3_sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn vec3_add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn vec3_scale(value: [f32; 3], factor: f32) -> [f32; 3] {
    [value[0] * factor, value[1] * factor, value[2] * factor]
}

fn vec3_cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn vec3_dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn vec3_length_squared(value: [f32; 3]) -> f32 {
    vec3_dot(value, value)
}

fn vec3_normalize(value: [f32; 3]) -> [f32; 3] {
    let length = vec3_length_squared(value).sqrt().max(0.0001);
    [value[0] / length, value[1] / length, value[2] / length]
}

fn shade_preview_color(mut color: [f32; 4], factor: f32) -> [f32; 4] {
    color[0] = (color[0] * factor).clamp(0.0, 1.0);
    color[1] = (color[1] * factor).clamp(0.0, 1.0);
    color[2] = (color[2] * factor).clamp(0.0, 1.0);
    color
}

fn preview_3d_block_face_from_direction(direction: Preview3dCardinalDirection) -> BlockFace {
    match direction {
        Preview3dCardinalDirection::North => BlockFace::North,
        Preview3dCardinalDirection::South => BlockFace::South,
        Preview3dCardinalDirection::East => BlockFace::East,
        Preview3dCardinalDirection::West => BlockFace::West,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_material() -> Preview3dMaterialName {
        Arc::from("minecraft_test")
    }

    #[test]
    fn resolver_geometry_turns_resource_pack_block_into_non_full_shape() {
        let mut repository = BlockModelRepository::new();
        repository.merge_blocks_value(&serde_json::json!({
            "minecraft:sunflower": {
                "geometry": "geometry.test.sunflower"
            }
        }));
        repository.geometries.merge_value(&serde_json::json!({
            "minecraft:geometry": [{
                "description": { "identifier": "geometry.test.sunflower" },
                "bones": [{
                    "name": "root",
                    "cubes": [{
                        "origin": [-8, 0, -0.5],
                        "size": [16, 16, 1]
                    }]
                }]
            }]
        }));
        let state = test_block_state_with_tag(
            "minecraft:double_plant",
            "double_plant_type",
            NbtTag::String("sunflower".to_string()),
        );

        let shape = preview_3d_resolved_detail_shape_for_block(
            &repository,
            &state,
            Preview3dBlockClass::DetailOpaque,
        )
        .unwrap_or_else(|| panic!("sunflower geometry should resolve"));

        assert_eq!(
            canonical_block_name_for_state(&preview_3d_block_state_query(&state)),
            "minecraft:sunflower"
        );
        assert_eq!(shape.cuboids.len(), 1);
        assert!(shape.cuboids[0].max[2] - shape.cuboids[0].min[2] <= 0.07);
        assert!(!preview_3d_shape_is_full_cube(&shape));
    }

    #[test]
    fn resolver_uses_shulker_geometry_for_shulker_box_blocks() {
        let mut repository = BlockModelRepository::new();
        repository.geometries.merge_value(&serde_json::json!({
            "geometry.shulker.v1.8": {
                "texturewidth": 64,
                "textureheight": 64,
                "bones": [{
                    "name": "base",
                    "cubes": [{
                        "origin": [-8.0, 0.0, -8.0],
                        "size": [16.0, 8.0, 16.0],
                        "uv": [0, 28]
                    }]
                }, {
                    "name": "lid",
                    "cubes": [{
                        "origin": [-8.0, 4.0, -8.0],
                        "size": [16.0, 12.0, 16.0],
                        "uv": [0, 0]
                    }]
                }, {
                    "name": "head",
                    "cubes": [{
                        "origin": [-3.0, 6.0, -3.0],
                        "size": [6.0, 6.0, 6.0],
                        "uv": [0, 52]
                    }]
                }]
            }
        }));

        let shape = preview_3d_resolved_detail_shape_for_block(
            &repository,
            &test_block_state("minecraft:blue_shulker_box"),
            Preview3dBlockClass::DetailOpaque,
        )
        .unwrap_or_else(|| panic!("shulker geometry should resolve"));

        assert_eq!(shape.cuboids.len(), 2);
        assert!(
            shape
                .cuboids
                .iter()
                .any(|cuboid| (cuboid.max[1] - cuboid.min[1] - 0.75).abs() < 0.001)
        );
        assert!(
            shape
                .cuboids
                .iter()
                .all(|cuboid| (cuboid.max[0] - cuboid.min[0]) > 0.4)
        );
        assert!(
            shape
                .cuboids
                .iter()
                .all(|cuboid| cuboid.face_material_slots.values().all(|slot| {
                    let slot = slot.as_ref();
                    slot == "up" || slot == "down" || slot == "side"
                }))
        );
    }

    #[test]
    fn legacy_variant_block_states_resolve_to_canonical_block_materials() {
        let red_flower = test_block_state_with_tag(
            "minecraft:red_flower",
            "flower_type",
            NbtTag::String("blue_orchid".to_string()),
        );
        let carpet = test_block_state_with_tag(
            "minecraft:carpet",
            "color",
            NbtTag::String("red".to_string()),
        );
        let numeric_carpet =
            test_block_state_with_tag("minecraft:carpet", "color", NbtTag::Byte(11));
        let spruce_fence = test_block_state_with_tag(
            "minecraft:fence",
            "wood_type",
            NbtTag::String("spruce".to_string()),
        );
        let brick_wall = test_block_state_with_tag(
            "minecraft:cobblestone_wall",
            "wall_block_type",
            NbtTag::String("brick".to_string()),
        );
        let end_brick_wall = test_block_state_with_tag(
            "minecraft:cobblestone_wall",
            "wall_block_type",
            NbtTag::String("end_brick".to_string()),
        );
        let blue_shulker =
            test_block_state_with_tag("minecraft:shulker_box", "color", NbtTag::Byte(11));

        assert_eq!(
            canonical_block_name_for_state(&preview_3d_block_state_query(&red_flower)),
            "minecraft:blue_orchid"
        );
        assert_eq!(
            canonical_block_name_for_state(&preview_3d_block_state_query(&carpet)),
            "minecraft:red_carpet"
        );
        assert_eq!(
            preview_3d_material_block_name_for_state(&carpet, Preview3dBlockClass::DetailOpaque)
                .as_ref(),
            "minecraft:red_wool"
        );
        assert_eq!(
            canonical_block_name_for_state(&preview_3d_block_state_query(&numeric_carpet)),
            "minecraft:blue_carpet"
        );
        assert_eq!(
            preview_3d_material_block_name_for_state(
                &numeric_carpet,
                Preview3dBlockClass::DetailOpaque
            )
            .as_ref(),
            "minecraft:blue_wool"
        );
        assert_eq!(
            canonical_block_name_for_state(&preview_3d_block_state_query(&spruce_fence)),
            "minecraft:spruce_fence"
        );
        assert_eq!(
            preview_3d_material_block_name_for_state(
                &spruce_fence,
                Preview3dBlockClass::DetailOpaque
            )
            .as_ref(),
            "minecraft:spruce_planks"
        );
        assert_eq!(
            canonical_block_name_for_state(&preview_3d_block_state_query(&brick_wall)),
            "minecraft:brick_wall"
        );
        assert_eq!(
            preview_3d_material_block_name_for_state(
                &brick_wall,
                Preview3dBlockClass::DetailOpaque
            )
            .as_ref(),
            "minecraft:bricks"
        );
        assert_eq!(
            canonical_block_name_for_state(&preview_3d_block_state_query(&end_brick_wall)),
            "minecraft:end_brick_wall"
        );
        assert_eq!(
            preview_3d_material_block_name_for_state(
                &end_brick_wall,
                Preview3dBlockClass::DetailOpaque
            )
            .as_ref(),
            "minecraft:end_bricks"
        );
        assert_eq!(
            canonical_block_name_for_state(&preview_3d_block_state_query(&blue_shulker)),
            "minecraft:blue_shulker_box"
        );
        assert_eq!(
            preview_3d_block_class("minecraft:double_plant"),
            Preview3dBlockClass::DetailOpaque
        );
    }

    fn test_mesh_with_faces(faces: Vec<Preview3dFace>, bounds: SlimeChunkBounds) -> Preview3dMesh {
        test_mesh_with_layers(faces, Vec::new(), Vec::new(), bounds)
    }

    fn test_mesh_with_layers(
        opaque_faces: Vec<Preview3dFace>,
        glass_faces: Vec<Preview3dFace>,
        water_faces: Vec<Preview3dFace>,
        bounds: SlimeChunkBounds,
    ) -> Preview3dMesh {
        let has_faces =
            !opaque_faces.is_empty() || !glass_faces.is_empty() || !water_faces.is_empty();
        let (min_y, max_y) = if !has_faces {
            (0, 16)
        } else {
            let mut min_y = f32::INFINITY;
            let mut max_y = f32::NEG_INFINITY;
            for corner in opaque_faces
                .iter()
                .chain(glass_faces.iter())
                .chain(water_faces.iter())
                .flat_map(|face| face.corners)
            {
                min_y = min_y.min(corner[1]);
                max_y = max_y.max(corner[1]);
            }
            (min_y.floor() as i16, max_y.ceil() as i16)
        };
        let min_x = (bounds.min_chunk_x.saturating_mul(16)) as f32;
        let max_x = (bounds.max_chunk_x.saturating_add(1).saturating_mul(16)) as f32;
        let min_z = (bounds.min_chunk_z.saturating_mul(16)) as f32;
        let max_z = (bounds.max_chunk_z.saturating_add(1).saturating_mul(16)) as f32;
        let min_y_f32 = f32::from(min_y);
        let max_y_f32 = f32::from(max_y).max(min_y_f32 + 1.0);
        let center = [
            (min_x + max_x) * 0.5,
            (min_y_f32 + max_y_f32) * 0.5,
            (min_z + max_z) * 0.5,
        ];
        let horizontal_span = (max_x - min_x).max(max_z - min_z).max(1.0);
        let vertical_span = (max_y_f32 - min_y_f32).max(1.0);
        let face_count = opaque_faces.len();
        let glass_face_count = glass_faces.len();
        let water_face_count = water_faces.len();
        let chunk_meshes = build_preview_3d_gpu_meshes(
            &opaque_faces,
            &glass_faces,
            &water_faces,
            &[],
            center,
            horizontal_span,
            vertical_span,
            7,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        Preview3dMesh {
            face_count,
            glass_face_count,
            water_face_count,
            lava_face_count: 0,
            chunk_meshes,
            min_y,
            max_y,
            min_x: bounds.min_chunk_x.saturating_mul(16),
            max_x: bounds
                .max_chunk_x
                .saturating_add(1)
                .saturating_mul(16)
                .saturating_sub(1),
            min_z: bounds.min_chunk_z.saturating_mul(16),
            max_z: bounds
                .max_chunk_z
                .saturating_add(1)
                .saturating_mul(16)
                .saturating_sub(1),
            missing_chunks: 0,
            chunk_count: 1,
            processed_chunk_count: 1,
            subchunk_count: 1,
            solid_block_count: usize::from(face_count > 0),
            glass_block_count: usize::from(glass_face_count > 0),
            water_block_count: usize::from(water_face_count > 0),
            lava_block_count: 0,
            culled_face_count: 0,
            omitted_face_count: 0,
            truncated_chunk_count: 0,
            vertex_budget: PREVIEW_3D_GPU_VERTEX_BUDGET,
        }
    }

    fn test_block_state(name: &str) -> BlockState {
        BlockState {
            name: name.to_string(),
            states: Default::default(),
            version: None,
        }
    }

    fn test_block_state_with_tag(name: &str, key: &str, value: NbtTag) -> BlockState {
        let mut states = BTreeMap::new();
        states.insert(key.to_string(), value);
        BlockState {
            name: name.to_string(),
            states,
            version: None,
        }
    }

    fn test_block_record(
        key: BlockKey,
        color: [f32; 4],
        material: Preview3dMaterialName,
    ) -> Preview3dBlockRecord {
        Preview3dBlockRecord::uniform(key, color, material)
    }

    fn test_water_color() -> [f32; 4] {
        [0.18, 0.48, 0.78, PREVIEW_3D_WATER_ALPHA]
    }

    fn test_render_chunk_with_subchunks(
        subchunks: BTreeMap<i8, bedrock_world::SubChunk>,
    ) -> RenderChunkData {
        RenderChunkData {
            pos: ChunkPos {
                x: 0,
                z: 0,
                dimension: bedrock_render::Dimension::Overworld,
            },
            is_loaded: true,
            height_map: None,
            legacy_biomes: None,
            legacy_biome_colors: None,
            biome_data: BTreeMap::new(),
            subchunks,
            block_entities: Vec::new(),
            legacy_terrain: None,
            column_samples: None,
            version: bedrock_world::ChunkVersion::New,
        }
    }

    fn test_gpu_mesh(mesh: &Preview3dMesh) -> &GpuMesh3d {
        mesh.chunk_meshes
            .first()
            .map(|chunk_mesh| chunk_mesh.gpu_mesh.as_ref())
            .unwrap_or_else(|| panic!("test mesh should contain a GPU mesh"))
    }

    #[test]
    fn map_viewer_preview_3d_chunk_positions_cover_full_selection_without_span_limit() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: -8,
            max_chunk_x: 7,
            min_chunk_z: -6,
            max_chunk_z: 9,
        };

        let positions = preview_3d_chunk_positions(bounds);

        assert_eq!(preview_3d_bounds_width(bounds), 16);
        assert_eq!(preview_3d_bounds_depth(bounds), 16);
        assert_eq!(positions.len(), 256);
        assert_eq!(
            positions.first().map(|position| (position.x, position.z)),
            Some((-8, -6))
        );
        assert_eq!(
            positions.last().map(|position| (position.x, position.z)),
            Some((7, 9))
        );
    }

    #[test]
    fn map_viewer_preview_3d_sorted_positions_cover_large_selection() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: -40,
            max_chunk_x: 39,
            min_chunk_z: -32,
            max_chunk_z: 31,
        };
        let mut positions = preview_3d_chunk_positions(bounds);

        preview_3d_sort_positions_by_distance(&mut positions, 0, 0);

        assert_eq!(positions.len(), preview_3d_bounds_chunk_count(bounds));
        assert_eq!(
            positions.first().map(|position| (position.x, position.z)),
            Some((0, 0))
        );
        assert!(positions.iter().any(|position| {
            position.x == bounds.min_chunk_x && position.z == bounds.min_chunk_z
        }));
        assert!(positions.iter().any(|position| {
            position.x == bounds.max_chunk_x && position.z == bounds.max_chunk_z
        }));
    }

    #[test]
    fn map_viewer_preview_3d_hides_faces_against_unprocessed_selected_neighbors() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 1,
            min_chunk_z: 0,
            max_chunk_z: 0,
        };
        let mut builder = Preview3dMeshBuilder::new(bounds, 0);
        let block = BlockKey { x: 15, y: 64, z: 8 };
        builder.push_processed_chunk(Preview3dProcessedChunk {
            chunk_key: ChunkKey { x: 0, z: 0 },
            subchunk_count: 1,
            internally_culled_blocks: 0,
            blocks: Some(Preview3dChunkBlocks {
                occupied: HashSet::from([block]),
                opaque_blocks: vec![test_block_record(
                    block,
                    [0.4, 0.5, 0.6, 1.0],
                    test_material(),
                )],
                min_y: 64,
                max_y: 64,
                min_x: 15,
                max_x: 15,
                min_z: 8,
                max_z: 8,
                ..Preview3dChunkBlocks::default()
            }),
        });

        builder
            .rebuild_combined_mesh()
            .unwrap_or_else(|error| panic!("{error}"));

        let mesh = builder.build_mesh();
        assert_eq!(mesh.face_count, 5);
    }

    #[test]
    fn map_viewer_preview_3d_restores_boundary_faces_after_selection_finishes() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 1,
            min_chunk_z: 0,
            max_chunk_z: 0,
        };
        let mut builder = Preview3dMeshBuilder::new(bounds, 0);
        let block = BlockKey { x: 15, y: 64, z: 8 };
        builder.push_processed_chunk(Preview3dProcessedChunk {
            chunk_key: ChunkKey { x: 0, z: 0 },
            subchunk_count: 1,
            internally_culled_blocks: 0,
            blocks: Some(Preview3dChunkBlocks {
                occupied: HashSet::from([block]),
                opaque_blocks: vec![test_block_record(
                    block,
                    [0.4, 0.5, 0.6, 1.0],
                    test_material(),
                )],
                min_y: 64,
                max_y: 64,
                min_x: 15,
                max_x: 15,
                min_z: 8,
                max_z: 8,
                ..Preview3dChunkBlocks::default()
            }),
        });
        builder.push_processed_chunk(Preview3dProcessedChunk {
            chunk_key: ChunkKey { x: 1, z: 0 },
            subchunk_count: 0,
            internally_culled_blocks: 0,
            blocks: None,
        });

        builder
            .rebuild_combined_mesh()
            .unwrap_or_else(|error| panic!("{error}"));
        let mesh = builder.build_mesh();

        assert_eq!(mesh.face_count, 6);
    }

    #[test]
    fn map_viewer_preview_3d_merges_exposed_faces_across_chunk_boundaries() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 1,
            min_chunk_z: 0,
            max_chunk_z: 0,
        };
        let mut merger = Preview3dFaceMerger::new();
        for x in 0..32 {
            merger.push(
                BlockKey { x, y: 64, z: 0 },
                FACE_DEFINITIONS[0],
                [0.4, 0.5, 0.6, 1.0],
                test_material(),
            );
        }
        let mut faces = Vec::new();
        let mut budget = Preview3dFaceBudget::new(PREVIEW_3D_FACE_BUDGET);
        merger.emit_into(&mut faces, &mut budget);
        let mesh = test_mesh_with_faces(faces, bounds);

        assert_eq!(mesh.face_count, 1);
        assert_eq!(test_gpu_mesh(&mesh).vertices.len(), 6);
    }

    #[test]
    fn map_viewer_preview_3d_merges_opaque_faces_after_cross_chunk_culling() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 1,
            min_chunk_z: 0,
            max_chunk_z: 0,
        };
        let mut builder = Preview3dMeshBuilder::new(bounds, 0);
        for chunk_x in 0..=1 {
            let blocks = (chunk_x * 16..chunk_x * 16 + 16)
                .map(|x| (BlockKey { x, y: 64, z: 0 }, [0.4, 0.5, 0.6, 1.0]))
                .map(|(block, color)| test_block_record(block, color, test_material()))
                .collect::<Vec<_>>();
            builder.block_chunks.insert(
                ChunkKey { x: chunk_x, z: 0 },
                Preview3dChunkBlocks {
                    occupied: blocks.iter().map(|block| block.key).collect(),
                    opaque_blocks: blocks,
                    min_y: 64,
                    max_y: 64,
                    min_x: chunk_x * 16,
                    max_x: chunk_x * 16 + 15,
                    min_z: 0,
                    max_z: 0,
                    ..Preview3dChunkBlocks::default()
                },
            );
        }

        builder
            .rebuild_combined_mesh()
            .unwrap_or_else(|error| panic!("{error}"));
        let mesh = builder.build_mesh();

        assert_eq!(mesh.face_count, 6);
    }

    #[test]
    fn map_viewer_preview_3d_surface_only_water_keeps_top_face_only() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        };
        let mut builder = Preview3dMeshBuilder::new(bounds, 0);
        let block = BlockKey { x: 8, y: 64, z: 8 };
        builder.push_processed_chunk(Preview3dProcessedChunk {
            chunk_key: ChunkKey { x: 0, z: 0 },
            subchunk_count: 1,
            internally_culled_blocks: 0,
            blocks: Some(Preview3dChunkBlocks {
                water: HashSet::from([block]),
                water_blocks: vec![Preview3dFluidBlock {
                    key: block,
                    color: test_water_color(),
                    material: test_material(),
                    surface_only: true,
                }],
                min_y: 64,
                max_y: 64,
                min_x: 8,
                max_x: 8,
                min_z: 8,
                max_z: 8,
                ..Preview3dChunkBlocks::default()
            }),
        });

        builder
            .rebuild_combined_mesh()
            .unwrap_or_else(|error| panic!("{error}"));
        let mesh = builder.build_mesh();

        assert_eq!(mesh.water_face_count, 1);
        assert_eq!(test_gpu_mesh(&mesh).ranges.water.count, 6);
    }

    #[test]
    fn map_viewer_preview_3d_filters_fully_hidden_internal_blocks() {
        let mut opaque_blocks = Vec::new();
        let mut occupied = HashSet::new();
        for y in 0..3 {
            for z in 0..3 {
                for x in 0..3 {
                    let block = BlockKey { x, y, z };
                    occupied.insert(block);
                    opaque_blocks.push(test_block_record(
                        block,
                        [0.4, 0.5, 0.6, 1.0],
                        test_material(),
                    ));
                }
            }
        }
        let mut blocks = Preview3dChunkBlocks {
            occupied,
            opaque_blocks,
            min_y: 0,
            max_y: 2,
            min_x: 0,
            max_x: 2,
            min_z: 0,
            max_z: 2,
            ..Preview3dChunkBlocks::default()
        };

        let culled = preview_3d_filter_internal_block_records(&mut blocks);

        assert_eq!(culled, 1);
        assert_eq!(blocks.opaque_blocks.len(), 26);
        assert!(
            !blocks
                .opaque_blocks
                .iter()
                .any(|block| block.key == BlockKey { x: 1, y: 1, z: 1 })
        );
    }

    #[test]
    fn map_viewer_preview_3d_keeps_full_grass_blocks_solid() {
        assert_eq!(
            preview_3d_block_class("minecraft:grass"),
            Preview3dBlockClass::Opaque
        );
        assert_eq!(
            preview_3d_block_class("minecraft:grass_block"),
            Preview3dBlockClass::Opaque
        );
        assert_eq!(
            preview_3d_block_class("minecraft:short_grass"),
            Preview3dBlockClass::DetailOpaque
        );
    }

    #[test]
    fn map_viewer_preview_3d_camera_wraps_rotation_and_clamps_zoom() {
        let mut camera = Preview3dCamera::default();
        let original_zoom = camera.zoom;

        camera.rotate_orbit(48.0, 24.0);
        assert_ne!(camera.yaw, Preview3dCamera::default().yaw);
        assert_ne!(camera.pitch, Preview3dCamera::default().pitch);
        assert_eq!(camera.zoom, original_zoom);
        camera.rotate_orbit(0.0, -240.0);
        assert!(camera.pitch > std::f32::consts::FRAC_PI_2);
        camera.rotate_orbit(0.0, 10_000.0);
        assert!(camera.pitch >= PREVIEW_3D_MIN_PITCH);
        assert!(camera.pitch <= PREVIEW_3D_MAX_PITCH);
        let pitch = camera.pitch;
        let yaw = camera.yaw;
        camera.zoom_by(0.001);
        assert_eq!(camera.pitch, pitch);
        assert_eq!(camera.yaw, yaw);
        assert_eq!(camera.zoom, PREVIEW_3D_MIN_ZOOM);
        camera.zoom_by(10_000.0);
        assert_eq!(camera.zoom, PREVIEW_3D_MAX_ZOOM);
    }

    #[test]
    fn map_viewer_preview_3d_model_rotation_keeps_center_and_moves_off_axis_point() {
        let camera = Preview3dCamera::default();
        let model_rotation = Preview3dModelRotation {
            yaw: 0.6,
            pitch: 0.0,
            mirror_x: false,
            mirror_z: false,
        };
        let center = [0.0, 64.0, 0.0];
        let off_axis_point = [16.0, 64.0, 0.0];
        let center_before = project_preview_point(center, center, 0.01, camera);
        let center_after = {
            let view_proj_model =
                preview_3d_view_proj_model(1.0, center, 0.01, camera, model_rotation);
            let projected = mat4_mul_vec4(view_proj_model, [center[0], center[1], center[2], 1.0]);
            let reciprocal_w = 1.0 / projected[3].max(0.0001);
            (
                projected[0] * reciprocal_w,
                projected[1] * reciprocal_w,
                projected[2] * reciprocal_w,
            )
        };
        let point_before = project_preview_point(off_axis_point, center, 0.01, camera);
        let point_after = {
            let view_proj_model =
                preview_3d_view_proj_model(1.0, center, 0.01, camera, model_rotation);
            let projected = mat4_mul_vec4(
                view_proj_model,
                [off_axis_point[0], off_axis_point[1], off_axis_point[2], 1.0],
            );
            let reciprocal_w = 1.0 / projected[3].max(0.0001);
            (
                projected[0] * reciprocal_w,
                projected[1] * reciprocal_w,
                projected[2] * reciprocal_w,
            )
        };

        assert!(
            center_before.0.abs() < 0.05,
            "model center should start near screen center: {center_before:?}"
        );
        assert!(
            center_after.0.abs() < 0.05,
            "model rotation should keep model center near screen center: {center_after:?}"
        );
        assert!(
            (point_after.0 - point_before.0).abs() > 0.1,
            "dragging model should move off-axis point on screen: before={point_before:?} after={point_after:?}"
        );
    }

    #[test]
    fn map_viewer_preview_3d_observer_vertical_controls_move_camera_height() {
        let mut camera = Preview3dCamera::default();
        let initial_y = camera.position[1];

        camera.move_from_input(
            Preview3dMovementInput {
                ascend: true,
                ..Preview3dMovementInput::default()
            },
            0.5,
        );
        assert!(camera.position[1] > initial_y);

        let raised_y = camera.position[1];
        camera.move_from_input(
            Preview3dMovementInput {
                descend: true,
                ..Preview3dMovementInput::default()
            },
            0.5,
        );
        assert!(camera.position[1] < raised_y);
    }

    #[test]
    fn map_viewer_preview_3d_drag_down_lowers_elevation_view() {
        let mut camera = Preview3dCamera::default();
        let initial_pitch = camera.pitch;

        camera.rotate_orbit(0.0, 80.0);

        assert!(camera.pitch < initial_pitch);
        assert!(camera.pitch >= PREVIEW_3D_MIN_PITCH);
    }

    #[test]
    fn map_viewer_preview_3d_pitch_midrange_projection_stays_continuous() {
        let center = [0.0, 64.0, 0.0];
        let scale = 0.01;
        let points = [
            [-8.0, 0.0, -8.0],
            [8.0, 0.0, -8.0],
            [-8.0, 128.0, 8.0],
            [8.0, 128.0, 8.0],
        ];
        let camera_at_44 = Preview3dCamera::new(-0.65, 44.8_f32.to_radians(), 1.0);
        let camera_at_50 = Preview3dCamera::new(-0.65, 50.0_f32.to_radians(), 1.0);

        for point in points {
            let before = project_preview_point(point, center, scale, camera_at_44);
            let after = project_preview_point(point, center, scale, camera_at_50);

            assert!(before.0.is_finite() && before.1.is_finite() && before.2.is_finite());
            assert!(after.0.is_finite() && after.1.is_finite() && after.2.is_finite());
            assert!(
                before.0.abs() < 0.9 && before.1.abs() < 0.9,
                "44.8deg projection should stay in view: {before:?}"
            );
            assert!(
                after.0.abs() < 0.9 && after.1.abs() < 0.9,
                "50deg projection should stay in view: {after:?}"
            );
            assert!(
                (after.0 - before.0).abs() < 0.25 && (after.1 - before.1).abs() < 0.25,
                "pitch 44.8deg to 50deg should be continuous: before={before:?} after={after:?}"
            );
        }
    }

    #[test]
    fn map_viewer_preview_3d_single_block_has_six_faces() {
        let block = BlockKey { x: 0, y: 0, z: 0 };
        let mut faces = Vec::new();
        for face in FACE_DEFINITIONS {
            faces.push(block_face(block, face, [0.4, 0.5, 0.6, 1.0]));
        }
        assert_eq!(faces.len(), 6);
    }

    #[test]
    fn map_viewer_preview_3d_adjacent_blocks_cull_shared_face() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        };
        let occupied =
            HashSet::from([BlockKey { x: 0, y: 0, z: 0 }, BlockKey { x: 1, y: 0, z: 0 }]);
        let mut faces = Vec::new();
        let mut culled = 0usize;
        for block in occupied.iter().copied() {
            for face in FACE_DEFINITIONS {
                let neighbor = BlockKey {
                    x: block.x + face.normal[0],
                    y: block.y + face.normal[1],
                    z: block.z + face.normal[2],
                };
                if occupied.contains(&neighbor) {
                    culled = culled.saturating_add(1);
                } else {
                    faces.push(block_face(block, face, [0.5, 0.5, 0.5, 1.0]));
                }
            }
        }
        let mesh = test_mesh_with_faces(faces, bounds);

        assert_eq!(culled, 2);
        assert_eq!(mesh.face_count, 10);
    }

    #[test]
    fn map_viewer_preview_3d_uses_full_subchunks_when_column_samples_exist() {
        let mut indices = vec![0_u16; 16 * 16 * 16];
        indices[bedrock_world::block_storage_index(0, 0, 0)] = 1;
        indices[bedrock_world::block_storage_index(0, 1, 0)] = 1;
        let mut subchunks = BTreeMap::new();
        subchunks.insert(
            0,
            bedrock_world::SubChunk {
                y: 0,
                format: bedrock_world::SubChunkFormat::Paletted {
                    version: 8,
                    storages: vec![bedrock_world::BlockPalette {
                        states: vec![
                            test_block_state("minecraft:air"),
                            test_block_state("minecraft:sand"),
                        ],
                        indices: Some(indices),
                        counts: vec![4094, 2],
                    }],
                },
            },
        );
        let mut column_samples = bedrock_world::TerrainColumnSamples::new();
        column_samples.set(
            0,
            0,
            bedrock_world::TerrainColumnSample {
                surface_y: 1,
                surface_block_state: test_block_state("minecraft:sand"),
                relief_y: 1,
                relief_block_state: test_block_state("minecraft:sand"),
                overlay: None,
                water: None,
                biome: None,
                source: bedrock_world::TerrainSampleSource::Subchunk,
            },
        );
        let chunk = RenderChunkData {
            pos: ChunkPos {
                x: 0,
                z: 0,
                dimension: bedrock_render::Dimension::Overworld,
            },
            is_loaded: true,
            height_map: None,
            legacy_biomes: None,
            legacy_biome_colors: None,
            biome_data: BTreeMap::new(),
            subchunks,
            block_entities: Vec::new(),
            legacy_terrain: None,
            column_samples: Some(column_samples),
            version: bedrock_world::ChunkVersion::New,
        };

        let blocks =
            preview_3d_collect_chunk_blocks(&chunk).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(blocks.opaque_blocks.len(), 2);
        assert!(blocks.occupied.contains(&BlockKey { x: 0, y: 0, z: 0 }));
        assert!(blocks.occupied.contains(&BlockKey { x: 0, y: 1, z: 0 }));
    }

    #[test]
    fn map_viewer_preview_3d_uses_primary_water_when_visible_layer_is_detail() {
        let mut water_indices = vec![0_u16; 16 * 16 * 16];
        let mut detail_indices = vec![0_u16; 16 * 16 * 16];
        let block_index = bedrock_world::block_storage_index(0, 0, 0);
        water_indices[block_index] = 1;
        detail_indices[block_index] = 1;
        let mut subchunks = BTreeMap::new();
        subchunks.insert(
            0,
            bedrock_world::SubChunk {
                y: 0,
                format: bedrock_world::SubChunkFormat::Paletted {
                    version: 9,
                    storages: vec![
                        bedrock_world::BlockPalette {
                            states: vec![
                                test_block_state("minecraft:air"),
                                test_block_state("minecraft:water"),
                            ],
                            indices: Some(water_indices),
                            counts: vec![4095, 1],
                        },
                        bedrock_world::BlockPalette {
                            states: vec![
                                test_block_state("minecraft:air"),
                                test_block_state("minecraft:seagrass"),
                            ],
                            indices: Some(detail_indices),
                            counts: vec![4095, 1],
                        },
                    ],
                },
            },
        );
        let chunk = test_render_chunk_with_subchunks(subchunks);

        let blocks =
            preview_3d_collect_chunk_blocks(&chunk).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(blocks.water_blocks.len(), 1);
        assert!(blocks.water.contains(&BlockKey { x: 0, y: 0, z: 0 }));
        assert!(blocks.opaque_blocks.is_empty());
    }

    #[test]
    fn map_viewer_preview_3d_transparent_blocks_are_classified() {
        for name in ["minecraft:air", "minecraft:water", "minecraft:glass"] {
            assert!(!preview_3d_is_solid_block(name), "{name}");
        }
        assert!(preview_3d_is_solid_block("minecraft:stone"));
        assert!(preview_3d_is_solid_block("minecraft:oak_leaves"));
        assert_eq!(
            preview_3d_block_class("minecraft:short_grass"),
            Preview3dBlockClass::DetailOpaque
        );
        assert_eq!(
            preview_3d_block_class("minecraft:vine"),
            Preview3dBlockClass::DetailOpaque
        );
        assert_eq!(
            preview_3d_block_class("minecraft:rail"),
            Preview3dBlockClass::DetailOpaque
        );
        assert_eq!(
            preview_3d_block_class("minecraft:standing_sign"),
            Preview3dBlockClass::DetailOpaque
        );
        assert_eq!(
            preview_3d_block_class("minecraft:water"),
            Preview3dBlockClass::Water
        );
        assert_eq!(
            preview_3d_block_class("minecraft:flowing_water"),
            Preview3dBlockClass::Water
        );
        assert_eq!(
            preview_3d_block_class("minecraft:lava"),
            Preview3dBlockClass::Lava
        );
        assert_eq!(
            preview_3d_block_class("minecraft:flowing_lava"),
            Preview3dBlockClass::Lava
        );
        for name in ["minecraft:glass", "minecraft:white_stained_glass"] {
            assert_eq!(
                preview_3d_block_class(name),
                Preview3dBlockClass::TransparentGlass,
                "{name}"
            );
        }
        for name in ["minecraft:glass_pane", "minecraft:blue_stained_glass_pane"] {
            assert_eq!(
                preview_3d_block_class(name),
                Preview3dBlockClass::DetailGlass,
                "{name}"
            );
        }
    }

    #[test]
    fn map_viewer_preview_3d_collects_common_detail_blocks_as_geometry() {
        let palette = RenderPalette::default();
        let block = BlockKey { x: 0, y: 64, z: 0 };
        let mut builder = Preview3dStructureChunkBuilder::default();

        for (offset, state) in [
            (
                0,
                test_block_state_with_tag("minecraft:stone_slab", "top_slot_bit", NbtTag::Byte(0)),
            ),
            (
                1,
                test_block_state_with_tag(
                    "minecraft:ladder",
                    "minecraft:cardinal_direction",
                    NbtTag::String("north".to_string()),
                ),
            ),
            (2, test_block_state("minecraft:oak_fence")),
            (3, test_block_state("minecraft:short_grass")),
        ]
        .iter()
        {
            preview_3d_push_structure_block(
                BlockKey {
                    x: block.x + *offset,
                    y: block.y,
                    z: block.z,
                },
                Some(state),
                None,
                &palette,
                &mut builder,
            )
            .unwrap_or_else(|error| panic!("{error}"));
        }

        assert_eq!(builder.blocks.detail_blocks.len(), 4);
        assert!(builder.blocks.occupied.is_empty());
    }

    #[test]
    fn map_viewer_preview_3d_collects_variant_detail_blocks_as_non_full_geometry() {
        let palette = RenderPalette::default();
        let mut builder = Preview3dStructureChunkBuilder::default();

        for (offset, state) in [
            (0, test_block_state("minecraft:iron_bars")),
            (1, test_block_state("minecraft:stonecutter")),
            (2, test_block_state("minecraft:anvil")),
            (3, test_block_state("minecraft:decorated_pot")),
            (4, test_block_state("minecraft:blue_shulker_box")),
            (5, test_block_state("minecraft:chain")),
            (6, test_block_state("minecraft:lantern")),
            (7, test_block_state("minecraft:chest")),
            (8, test_block_state("minecraft:hopper")),
        ] {
            preview_3d_push_structure_block(
                BlockKey {
                    x: offset,
                    y: 64,
                    z: 0,
                },
                Some(&state),
                None,
                &palette,
                &mut builder,
            )
            .unwrap_or_else(|error| panic!("{error}"));
        }

        assert_eq!(builder.blocks.detail_blocks.len(), 9);
        assert!(builder.blocks.occupied.is_empty());
    }

    #[test]
    fn map_viewer_preview_3d_detail_blocks_generate_non_full_cube_faces() {
        let palette = RenderPalette::default();
        let block = BlockKey { x: 0, y: 64, z: 0 };
        let mut builder = Preview3dMeshBuilder::new(
            SlimeChunkBounds {
                dimension: bedrock_render::Dimension::Overworld,
                min_chunk_x: 0,
                max_chunk_x: 0,
                min_chunk_z: 0,
                max_chunk_z: 0,
            },
            0,
        );
        let mut chunk_builder = Preview3dStructureChunkBuilder::default();
        let state =
            test_block_state_with_tag("minecraft:stone_slab", "top_slot_bit", NbtTag::Byte(0));
        preview_3d_push_structure_block(block, Some(&state), None, &palette, &mut chunk_builder)
            .unwrap_or_else(|error| panic!("{error}"));
        builder.push_processed_chunk(Preview3dProcessedChunk {
            chunk_key: ChunkKey { x: 0, z: 0 },
            subchunk_count: 1,
            internally_culled_blocks: 0,
            blocks: Some(chunk_builder.blocks),
        });

        builder
            .rebuild_combined_mesh()
            .unwrap_or_else(|error| panic!("{error}"));
        let mesh = builder.build_mesh();
        let gpu_mesh = test_gpu_mesh(&mesh);
        let highest_y = gpu_mesh
            .vertices
            .iter()
            .map(|vertex| vertex.position[1])
            .fold(f32::NEG_INFINITY, f32::max);

        assert_eq!(mesh.face_count, 6);
        assert!((highest_y - 64.5).abs() < 0.001);
    }

    #[test]
    fn map_viewer_preview_3d_fence_and_wall_shapes_follow_connection_state() {
        let fence = test_block_state_with_tag("minecraft:oak_fence", "north", NbtTag::Byte(1));
        let fence_shape = preview_3d_detail_shape_for_block(&fence)
            .unwrap_or_else(|| panic!("fence should have detail shape"));
        assert_eq!(fence_shape.cuboids.len(), 3);
        assert!(
            fence_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min[2] == 0.0 && cuboid.max[2] == 0.5)
        );

        let mut wall_states = BTreeMap::new();
        wall_states.insert(
            "wall_connection_type_north".to_string(),
            NbtTag::String("tall".to_string()),
        );
        wall_states.insert(
            "wall_connection_type_south".to_string(),
            NbtTag::String("none".to_string()),
        );
        wall_states.insert("wall_post_bit".to_string(), NbtTag::Byte(1));
        let wall = BlockState {
            name: "minecraft:brick_wall".to_string(),
            states: wall_states,
            version: None,
        };
        let wall_shape = preview_3d_detail_shape_for_block(&wall)
            .unwrap_or_else(|| panic!("wall should have detail shape"));
        assert_eq!(wall_shape.cuboids.len(), 2);
        assert!(
            wall_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min[2] == 0.0 && (cuboid.max[1] - 1.0).abs() < 0.001)
        );
    }

    #[test]
    fn map_viewer_preview_3d_stairs_use_inner_and_outer_shape_states() {
        let mut inner_states = BTreeMap::new();
        inner_states.insert(
            "minecraft:cardinal_direction".to_string(),
            NbtTag::String("north".to_string()),
        );
        inner_states.insert(
            "shape".to_string(),
            NbtTag::String("inner_left".to_string()),
        );
        let inner = BlockState {
            name: "minecraft:oak_stairs".to_string(),
            states: inner_states,
            version: None,
        };
        let inner_shape = preview_3d_detail_shape_for_block(&inner)
            .unwrap_or_else(|| panic!("inner stairs should have detail shape"));

        let mut outer_states = BTreeMap::new();
        outer_states.insert(
            "minecraft:cardinal_direction".to_string(),
            NbtTag::String("north".to_string()),
        );
        outer_states.insert(
            "shape".to_string(),
            NbtTag::String("outer_right".to_string()),
        );
        let outer = BlockState {
            name: "minecraft:oak_stairs".to_string(),
            states: outer_states,
            version: None,
        };
        let outer_shape = preview_3d_detail_shape_for_block(&outer)
            .unwrap_or_else(|| panic!("outer stairs should have detail shape"));

        assert_eq!(inner_shape.cuboids.len(), 3);
        assert_eq!(outer_shape.cuboids.len(), 2);
        assert!(
            outer_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min[0] >= 0.5 && cuboid.max[2] <= 0.5)
        );
    }

    #[test]
    fn map_viewer_preview_3d_shulker_box_shape_is_closed_shell_without_inner_head() {
        let shape =
            preview_3d_detail_shape_for_block(&test_block_state("minecraft:blue_shulker_box"))
                .unwrap_or_else(|| panic!("shulker box should have detail shape"));

        assert_eq!(shape.cuboids.len(), 2);
        assert!(
            shape
                .cuboids
                .iter()
                .all(|cuboid| cuboid.max[0] - cuboid.min[0] >= 1.0
                    || cuboid.max[1] - cuboid.min[1] >= 0.5)
        );
    }

    #[test]
    fn map_viewer_preview_3d_panes_iron_bars_and_plants_use_cutout_detail_shapes() {
        let iron_bars = test_block_state_with_tag("minecraft:iron_bars", "north", NbtTag::Byte(1));
        let bars_shape = preview_3d_detail_shape_for_block(&iron_bars)
            .unwrap_or_else(|| panic!("iron bars should have detail shape"));
        assert_eq!(bars_shape.cuboids.len(), 2);
        assert!(
            bars_shape
                .cuboids
                .iter()
                .all(|cuboid| cuboid.max[0] - cuboid.min[0] <= 0.25)
        );

        let isolated_bars_shape =
            preview_3d_detail_shape_for_block(&test_block_state("minecraft:iron_bars"))
                .unwrap_or_else(|| panic!("isolated iron bars should have detail shape"));
        assert_eq!(isolated_bars_shape.cuboids.len(), 3);
        assert!(
            isolated_bars_shape
                .cuboids
                .iter()
                .filter(|cuboid| cuboid.max[0] - cuboid.min[0] >= 1.0
                    || cuboid.max[2] - cuboid.min[2] >= 1.0)
                .count()
                == 2
        );

        let plant_shape = preview_3d_detail_shape_for_block(&test_block_state("minecraft:poppy"))
            .unwrap_or_else(|| panic!("plant should have detail shape"));
        assert!(plant_shape.cuboids.is_empty());
        assert_eq!(plant_shape.planes.len(), 2);

        let web_shape = preview_3d_detail_shape_for_block(&test_block_state("minecraft:web"))
            .unwrap_or_else(|| panic!("web should have detail shape"));
        assert!(web_shape.cuboids.is_empty());
        assert_eq!(web_shape.planes.len(), 2);
    }

    #[test]
    fn map_viewer_preview_3d_redstone_wire_uses_flat_cutout_planes() {
        let mut states = BTreeMap::new();
        states.insert("power".to_string(), NbtTag::Int(15));
        states.insert("north".to_string(), NbtTag::String("side".to_string()));
        states.insert("south".to_string(), NbtTag::String("side".to_string()));
        states.insert("east".to_string(), NbtTag::String("none".to_string()));
        states.insert("west".to_string(), NbtTag::String("none".to_string()));
        let redstone = BlockState {
            name: "minecraft:redstone_wire".to_string(),
            states,
            version: None,
        };
        let shape = preview_3d_detail_shape_for_block(&redstone)
            .unwrap_or_else(|| panic!("redstone wire should have detail shape"));

        assert!(shape.cuboids.is_empty());
        assert_eq!(shape.planes.len(), 1);
        assert_eq!(shape.planes[0].material_slot.as_deref(), Some("down"));
        assert!(
            shape.planes[0]
                .corners
                .iter()
                .all(|corner| (corner[1] - 0.01).abs() < 0.001)
        );
        let color = preview_3d_redstone_wire_color(&redstone);
        assert!((color[0] - 1.0).abs() < 0.001);
        assert!((color[1] - 0.2).abs() < 0.001);
        assert!((color[2] - 0.0).abs() < 0.001);
        assert!((color[3] - 1.0).abs() < 0.001);
    }

    #[test]
    fn map_viewer_preview_3d_panes_infer_missing_connections_from_neighbors() {
        let palette = RenderPalette::default();
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        };
        let mut chunk_builder = Preview3dStructureChunkBuilder::default();
        preview_3d_push_structure_block(
            BlockKey { x: 0, y: 64, z: 0 },
            Some(&test_block_state("minecraft:iron_bars")),
            None,
            &palette,
            &mut chunk_builder,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        preview_3d_push_structure_block(
            BlockKey { x: 1, y: 64, z: 0 },
            Some(&test_block_state("minecraft:stone")),
            None,
            &palette,
            &mut chunk_builder,
        )
        .unwrap_or_else(|error| panic!("{error}"));

        let mut builder = Preview3dMeshBuilder::new(bounds, 0);
        builder.push_processed_chunk(Preview3dProcessedChunk {
            chunk_key: ChunkKey { x: 0, z: 0 },
            subchunk_count: 1,
            internally_culled_blocks: 0,
            blocks: Some(chunk_builder.blocks),
        });
        builder
            .rebuild_combined_mesh()
            .unwrap_or_else(|error| panic!("{error}"));
        let mesh = builder.build_mesh();
        let gpu_mesh = test_gpu_mesh(&mesh);

        assert!(
            gpu_mesh.vertices.iter().any(|vertex| {
                (vertex.position[0] - 1.0).abs() < 0.001
                    && vertex.position[1] >= 64.0
                    && vertex.position[1] <= 65.0
                    && vertex.position[2] >= 0.4375
                    && vertex.position[2] <= 0.5625
            }),
            "iron bars without explicit state should connect to the east neighbor"
        );
    }

    #[test]
    fn map_viewer_preview_3d_chest_faces_use_full_inventory_textures() {
        let shape = preview_3d_detail_shape_for_block(&test_block_state("minecraft:chest"))
            .unwrap_or_else(|| panic!("chest should have detail shape"));
        let body = shape
            .cuboids
            .first()
            .unwrap_or_else(|| panic!("chest body should exist"));
        let side_uv = body
            .face_uvs
            .get(&BlockFace::North)
            .copied()
            .unwrap_or_else(|| panic!("chest side should have explicit uv"));

        assert_eq!(side_uv, preview_3d_full_texture_uv());
    }

    #[test]
    fn map_viewer_preview_3d_double_slabs_are_full_blocks_with_slab_material() {
        assert_eq!(
            preview_3d_block_class("minecraft:double_stone_slab"),
            Preview3dBlockClass::Opaque
        );
        let state = test_block_state_with_tag(
            "minecraft:double_stone_slab",
            "stone_slab_type",
            NbtTag::String("brick".to_string()),
        );
        assert_eq!(
            preview_3d_material_block_name_for_state(&state, Preview3dBlockClass::Opaque).as_ref(),
            "minecraft:bricks"
        );
    }

    #[test]
    fn map_viewer_preview_3d_trapdoor_torch_and_portal_use_state_geometry() {
        let mut trapdoor_states = BTreeMap::new();
        trapdoor_states.insert("open".to_string(), NbtTag::Byte(1));
        trapdoor_states.insert("facing_direction".to_string(), NbtTag::Byte(4));
        let trapdoor = BlockState {
            name: "minecraft:oak_trapdoor".to_string(),
            states: trapdoor_states,
            version: None,
        };
        let trapdoor_shape = preview_3d_detail_shape_for_block(&trapdoor)
            .unwrap_or_else(|| panic!("trapdoor should have detail shape"));
        assert!(
            trapdoor_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.max[1] - cuboid.min[1] > 0.9
                    && cuboid.min[0] > 0.8
                    && cuboid.max[0] <= 1.0)
        );

        let torch = test_block_state_with_tag(
            "minecraft:torch",
            "minecraft:block_face",
            NbtTag::String("floor".to_string()),
        );
        let torch_shape = preview_3d_detail_shape_for_block(&torch)
            .unwrap_or_else(|| panic!("torch should have detail shape"));
        assert!(torch_shape.cuboids[0].max[0] - torch_shape.cuboids[0].min[0] < 0.25);
        assert!(torch_shape.cuboids[0].max[1] <= 0.625);

        let portal = test_block_state_with_tag(
            "minecraft:portal",
            "portal_axis",
            NbtTag::String("x".to_string()),
        );
        let portal_shape = preview_3d_detail_shape_for_block(&portal)
            .unwrap_or_else(|| panic!("portal should have detail shape"));
        assert!(portal_shape.cuboids[0].max[2] - portal_shape.cuboids[0].min[2] < 0.08);
    }

    #[test]
    fn map_viewer_preview_3d_glass_panes_use_detail_glass_layer() {
        let palette = RenderPalette::default();
        let block = BlockKey { x: 0, y: 64, z: 0 };
        let mut builder = Preview3dMeshBuilder::new(
            SlimeChunkBounds {
                dimension: bedrock_render::Dimension::Overworld,
                min_chunk_x: 0,
                max_chunk_x: 0,
                min_chunk_z: 0,
                max_chunk_z: 0,
            },
            0,
        );
        let mut chunk_builder = Preview3dStructureChunkBuilder::default();
        let state = test_block_state("minecraft:glass_pane");
        preview_3d_push_structure_block(block, Some(&state), None, &palette, &mut chunk_builder)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(chunk_builder.blocks.glass_detail_blocks.len(), 1);
        builder.push_processed_chunk(Preview3dProcessedChunk {
            chunk_key: ChunkKey { x: 0, z: 0 },
            subchunk_count: 1,
            internally_culled_blocks: 0,
            blocks: Some(chunk_builder.blocks),
        });

        builder
            .rebuild_combined_mesh()
            .unwrap_or_else(|error| panic!("{error}"));
        let mesh = builder.build_mesh();
        let gpu_mesh = test_gpu_mesh(&mesh);

        assert_eq!(mesh.face_count, 0);
        assert!(mesh.glass_face_count > 6);
        assert_eq!(gpu_mesh.ranges.opaque.count, 0);
        assert!(gpu_mesh.ranges.glass.count > 6);
    }

    #[test]
    fn map_viewer_preview_3d_grass_faces_and_water_use_face_specific_colors() {
        let palette = RenderPalette::default()
            .with_block_color(
                "minecraft:grass_block",
                bedrock_render::RgbaColor::new(20, 210, 30, 255),
            )
            .with_block_color(
                "minecraft:grass",
                bedrock_render::RgbaColor::new(80, 150, 50, 255),
            )
            .with_block_color(
                "minecraft:dirt",
                bedrock_render::RgbaColor::new(120, 80, 45, 255),
            )
            .with_block_color(
                "minecraft:water",
                bedrock_render::RgbaColor::new(10, 90, 220, 255),
            );
        let grass_state = test_block_state("minecraft:grass_block");
        let water_state = test_block_state("minecraft:water");

        let grass_colors = preview_3d_face_colors_for_block(&palette, &grass_state, None);
        let water_color = preview_3d_water_color_for_block(&palette, &water_state, None);

        assert!(grass_colors.up[1] > grass_colors.side[1]);
        assert!(grass_colors.down[0] > grass_colors.up[0]);
        assert!(water_color[2] > water_color[1]);
        assert!((water_color[3] - PREVIEW_3D_WATER_ALPHA).abs() < 0.001);
    }

    #[test]
    fn map_viewer_preview_3d_water_color_falls_back_from_washed_out_palette_color() {
        let palette = RenderPalette::default().with_block_color(
            "minecraft:water",
            bedrock_render::RgbaColor::new(240, 240, 245, 255),
        );
        let water_state = test_block_state("minecraft:water");

        let water_color = preview_3d_water_color_for_block(&palette, &water_state, None);

        assert!(water_color[2] > water_color[1]);
        assert!(water_color[1] > water_color[0]);
        assert!(preview_3d_luminance(water_color) < 0.66);
        assert!((water_color[3] - PREVIEW_3D_WATER_ALPHA).abs() < 0.001);
    }

    #[test]
    fn map_viewer_preview_3d_washed_out_water_color_detection_rejects_white() {
        assert!(preview_3d_water_color_is_washed_out([
            0.94,
            0.94,
            0.96,
            PREVIEW_3D_WATER_ALPHA,
        ]));
        assert!(!preview_3d_water_color_is_washed_out([
            PREVIEW_3D_DEFAULT_WATER_RGB[0],
            PREVIEW_3D_DEFAULT_WATER_RGB[1],
            PREVIEW_3D_DEFAULT_WATER_RGB[2],
            PREVIEW_3D_WATER_ALPHA,
        ]));
    }

    #[test]
    fn map_viewer_preview_3d_grass_and_leaves_use_surface_palette_colors() {
        let palette = RenderPalette::default()
            .with_block_color(
                "minecraft:grass_block",
                bedrock_render::RgbaColor::new(32, 210, 48, 255),
            )
            .with_block_color(
                "minecraft:grass",
                bedrock_render::RgbaColor::new(132, 92, 52, 255),
            );
        let grass_block_state = BlockState {
            name: "minecraft:grass_block".to_string(),
            states: Default::default(),
            version: None,
        };
        let legacy_grass_state = BlockState {
            name: "minecraft:grass".to_string(),
            states: Default::default(),
            version: None,
        };
        let leaf_state = BlockState {
            name: "minecraft:oak_leaves".to_string(),
            states: Default::default(),
            version: None,
        };

        let grass_color = preview_3d_color_for_block(&palette, &grass_block_state, None);
        let legacy_grass_color = preview_3d_color_for_block(&palette, &legacy_grass_state, None);
        let leaf_color = preview_3d_color_for_block(&palette, &leaf_state, None);

        assert!(
            grass_color[1] > grass_color[0] * 1.6,
            "grass block should use the green surface color instead of the dirt side color: {grass_color:?}"
        );
        assert_eq!(legacy_grass_color, grass_color);
        assert!((leaf_color[3] - 1.0).abs() < 0.001);
    }

    #[test]
    fn map_viewer_preview_3d_water_blocks_generate_shell_faces() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        };
        let water_blocks =
            HashSet::from([BlockKey { x: 0, y: 0, z: 0 }, BlockKey { x: 1, y: 0, z: 0 }]);
        let mut water_faces = Vec::new();
        let mut culled = 0usize;
        for block in water_blocks.iter().copied() {
            for face in FACE_DEFINITIONS {
                let neighbor = BlockKey {
                    x: block.x + face.normal[0],
                    y: block.y + face.normal[1],
                    z: block.z + face.normal[2],
                };
                if water_blocks.contains(&neighbor) {
                    culled = culled.saturating_add(1);
                    continue;
                }
                water_faces.push(block_face(block, face, test_water_color()));
            }
        }
        let mesh = test_mesh_with_layers(Vec::new(), Vec::new(), water_faces, bounds);

        assert_eq!(culled, 2);
        assert_eq!(mesh.face_count, 0);
        assert_eq!(mesh.water_face_count, 10);
        assert_eq!(mesh.water_block_count, 1);
    }

    #[test]
    fn map_viewer_preview_3d_lava_blocks_generate_shell_faces() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        };
        let lava_blocks =
            HashSet::from([BlockKey { x: 0, y: 0, z: 0 }, BlockKey { x: 1, y: 0, z: 0 }]);
        let mut lava_faces = Vec::new();
        let mut culled = 0usize;
        for block in lava_blocks.iter().copied() {
            for face in FACE_DEFINITIONS {
                let neighbor = BlockKey {
                    x: block.x + face.normal[0],
                    y: block.y + face.normal[1],
                    z: block.z + face.normal[2],
                };
                if lava_blocks.contains(&neighbor) {
                    culled = culled.saturating_add(1);
                    continue;
                }
                lava_faces.push(block_face(
                    block,
                    face,
                    [0.98, 0.32, 0.05, PREVIEW_3D_LAVA_ALPHA],
                ));
            }
        }
        let meshes =
            build_preview_3d_gpu_meshes(&[], &[], &[], &lava_faces, [0.0, 0.0, 0.0], 16.0, 16.0, 1)
                .unwrap_or_else(|error| panic!("{error}"));
        let gpu_mesh = meshes
            .first()
            .unwrap_or_else(|| panic!("lava mesh should be generated"));

        assert_eq!(culled, 2);
        assert_eq!(lava_faces.len(), 10);
        assert_eq!(gpu_mesh.gpu_mesh.ranges.water.count, 60);
        assert!((gpu_mesh.gpu_mesh.vertices[0].color[3] - PREVIEW_3D_LAVA_ALPHA).abs() < 0.001);
    }

    #[test]
    fn map_viewer_preview_3d_glass_uses_transparent_layer() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        };
        let glass = block_face(
            BlockKey { x: 8, y: 64, z: 8 },
            FACE_DEFINITIONS[0],
            [0.50, 0.72, 0.95, PREVIEW_3D_GLASS_ALPHA],
        );
        let mesh = test_mesh_with_layers(Vec::new(), vec![glass], Vec::new(), bounds);

        assert_eq!(mesh.face_count, 0);
        assert_eq!(mesh.glass_face_count, 1);
        assert_eq!(mesh.water_face_count, 0);
        let gpu_mesh = test_gpu_mesh(&mesh);
        assert_eq!(gpu_mesh.ranges.opaque.count, 0);
        assert_eq!(gpu_mesh.ranges.glass.start, 0);
        assert_eq!(gpu_mesh.ranges.glass.count, 6);
        assert_eq!(gpu_mesh.ranges.water.start, 6);
        assert_eq!(gpu_mesh.vertices[0].color[3], PREVIEW_3D_GLASS_ALPHA);
    }

    #[test]
    fn map_viewer_preview_3d_glass_color_uses_palette_with_alpha() {
        let palette = RenderPalette::default().with_block_color(
            "minecraft:white_stained_glass",
            bedrock_render::RgbaColor::new(32, 96, 224, 255),
        );
        let state = BlockState {
            name: "minecraft:white_stained_glass".to_string(),
            states: Default::default(),
            version: None,
        };
        let color = preview_3d_transparent_color_for_block(&palette, &state, None);
        let expected = palette
            .block_color("minecraft:white_stained_glass")
            .to_array();
        let glass = block_face(BlockKey { x: 8, y: 64, z: 8 }, FACE_DEFINITIONS[0], color);
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        };
        let mesh = test_mesh_with_layers(Vec::new(), vec![glass], Vec::new(), bounds);
        let gpu_mesh = test_gpu_mesh(&mesh);
        let vertex_color = gpu_mesh.vertices[gpu_mesh.ranges.glass.start as usize].color;

        assert_eq!(gpu_mesh.ranges.glass.count, 6);
        assert!((vertex_color[0] - f32::from(expected[0]) / 255.0).abs() < 0.001);
        assert!((vertex_color[1] - f32::from(expected[1]) / 255.0).abs() < 0.001);
        assert!((vertex_color[2] - f32::from(expected[2]) / 255.0).abs() < 0.001);
        assert!((vertex_color[3] - PREVIEW_3D_GLASS_ALPHA).abs() < 0.001);
    }

    #[test]
    fn map_viewer_preview_3d_gpu_projection_keeps_typical_meshes_visible() {
        for span in [1, 4] {
            let bounds = SlimeChunkBounds {
                dimension: bedrock_render::Dimension::Overworld,
                min_chunk_x: 0,
                max_chunk_x: span - 1,
                min_chunk_z: 0,
                max_chunk_z: span - 1,
            };
            let face = block_face(
                BlockKey {
                    x: span * 8,
                    y: 64,
                    z: span * 8,
                },
                FACE_DEFINITIONS[0],
                [0.3, 0.7, 0.2, 1.0],
            );
            let mesh = test_mesh_with_faces(vec![face], bounds);
            let gpu_mesh = test_gpu_mesh(&mesh);
            let camera = Preview3dCamera::default();
            let projected = gpu_mesh
                .vertices
                .iter()
                .map(|vertex| {
                    project_preview_point(
                        vertex.position,
                        gpu_mesh.center,
                        gpu_mesh.fit_scale,
                        camera,
                    )
                })
                .collect::<Vec<_>>();

            assert!(!projected.is_empty());
            assert!(projected.iter().any(|position| {
                position.0 >= -1.0 && position.0 <= 1.0 && position.1 >= -1.0 && position.1 <= 1.0
            }));
        }
    }

    #[test]
    fn map_viewer_preview_3d_projection_keeps_y_up_and_x_right() {
        let camera = Preview3dCamera::new(0.0, 0.68, 1.0);
        let center = [0.0, 64.0, 0.0];
        let base = project_preview_point([0.0, 64.0, 0.0], center, 0.1, camera);
        let higher = project_preview_point([0.0, 80.0, 0.0], center, 0.1, camera);
        let right = project_preview_point([8.0, 64.0, 0.0], center, 0.1, camera);

        assert!(
            higher.1 > base.1,
            "positive Y should move upward in NDC coordinates"
        );
        assert!(
            right.0 > base.0,
            "positive X should remain rightward in NDC coordinates"
        );
    }

    #[test]
    fn map_viewer_preview_3d_projection_keeps_vertical_world_edges_upright() {
        let center = [8.0, 64.0, 8.0];
        let camera = Preview3dCamera::new(-0.65, 50.0_f32.to_radians(), 1.0);
        let bottom = project_preview_point([8.0, 0.0, 8.0], center, 0.01, camera);
        let top = project_preview_point([8.0, 128.0, 8.0], center, 0.01, camera);

        assert!(
            (top.0 - bottom.0).abs() < 0.001,
            "world Y edges should not lean horizontally: bottom={bottom:?} top={top:?}"
        );
        assert!(
            top.1 > bottom.1,
            "positive world Y should remain upward: bottom={bottom:?} top={top:?}"
        );
    }

    #[test]
    fn map_viewer_preview_3d_projection_accepts_near_vertical_angles() {
        let center = [8.0, 64.0, 8.0];
        for pitch in [PREVIEW_3D_MIN_PITCH, PREVIEW_3D_MAX_PITCH] {
            let camera = Preview3dCamera::new(-0.65, pitch, 1.0);
            let projected = project_preview_point([8.0, 64.0, 8.0], center, 0.01, camera);

            assert!(
                projected.0.is_finite() && projected.1.is_finite() && projected.2.is_finite(),
                "near-vertical camera should project finite coordinates: {projected:?}"
            );
        }
    }

    #[test]
    fn map_viewer_preview_3d_mesh_center_keeps_low_and_high_structures_centered() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        };
        for y in [0, 64, 220] {
            let face = block_face(
                BlockKey { x: 8, y, z: 8 },
                FACE_DEFINITIONS[0],
                [0.3, 0.7, 0.2, 1.0],
            );
            let mesh = test_mesh_with_faces(vec![face], bounds);
            let gpu_mesh = test_gpu_mesh(&mesh);
            let camera = Preview3dCamera::default();
            let average_y = gpu_mesh
                .vertices
                .iter()
                .map(|vertex| {
                    project_preview_point(
                        vertex.position,
                        gpu_mesh.center,
                        gpu_mesh.fit_scale,
                        camera,
                    )
                    .1
                })
                .sum::<f32>()
                / gpu_mesh.vertices.len() as f32;

            assert!(
                average_y.abs() < 0.35,
                "y={y} should stay centered, got average NDC y={average_y}"
            );
        }
    }

    #[test]
    fn map_viewer_preview_3d_water_vertices_are_after_opaque_vertices() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        };
        let opaque = block_face(
            BlockKey { x: 7, y: 64, z: 8 },
            FACE_DEFINITIONS[0],
            [0.3, 0.7, 0.2, 1.0],
        );
        let water = block_face(
            BlockKey { x: 8, y: 64, z: 8 },
            FACE_DEFINITIONS[0],
            test_water_color(),
        );
        let mesh = test_mesh_with_layers(vec![opaque], Vec::new(), vec![water], bounds);
        let gpu_mesh = test_gpu_mesh(&mesh);
        let vertices = &gpu_mesh.vertices;
        let ranges = gpu_mesh.ranges;

        assert_eq!(vertices.len(), 12);
        assert_eq!(ranges.opaque.start, 0);
        assert_eq!(ranges.opaque.count, 6);
        assert_eq!(ranges.glass.start, 6);
        assert_eq!(ranges.glass.count, 0);
        assert_eq!(ranges.water.start, 6);
        assert_eq!(ranges.water.count, 6);
        assert_eq!(vertices[ranges.opaque.start as usize].color[3], 1.0);
        let water_color = vertices[ranges.water.start as usize].color;
        assert!((water_color[3] - PREVIEW_3D_WATER_ALPHA).abs() < 0.001);
        assert!(water_color[2] > water_color[1]);
        assert!(water_color[1] > water_color[0]);
    }

    #[test]
    fn map_viewer_preview_3d_gpu_mesh_groups_material_ranges_globally() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 1,
            min_chunk_z: 0,
            max_chunk_z: 0,
        };
        let opaque = block_face(
            BlockKey { x: 7, y: 64, z: 8 },
            FACE_DEFINITIONS[0],
            [0.3, 0.7, 0.2, 1.0],
        );
        let glass = block_face(
            BlockKey { x: 8, y: 64, z: 8 },
            FACE_DEFINITIONS[0],
            [0.8, 0.9, 1.0, PREVIEW_3D_GLASS_ALPHA],
        );
        let water = block_face(
            BlockKey { x: 24, y: 64, z: 8 },
            FACE_DEFINITIONS[0],
            test_water_color(),
        );
        let mesh = test_mesh_with_layers(vec![opaque], vec![glass], vec![water], bounds);
        let gpu_mesh = test_gpu_mesh(&mesh);

        assert_eq!(gpu_mesh.vertices.len(), 18);
        assert_eq!(gpu_mesh.ranges.opaque.start, 0);
        assert_eq!(gpu_mesh.ranges.opaque.count, 6);
        assert_eq!(gpu_mesh.ranges.glass.start, 6);
        assert_eq!(gpu_mesh.ranges.glass.count, 6);
        assert_eq!(gpu_mesh.ranges.water.start, 12);
        assert_eq!(gpu_mesh.ranges.water.count, 6);
    }

    #[test]
    fn map_viewer_preview_3d_gpu_meshes_split_vertex_counts_over_budget() {
        assert!(
            PREVIEW_3D_GPU_VERTEX_BUDGET * std::mem::size_of::<GpuMesh3dVertex>()
                < 256 * 1024 * 1024
        );
        let faces = vec![
            block_face(
                BlockKey { x: 0, y: 0, z: 0 },
                FACE_DEFINITIONS[0],
                [0.3, 0.7, 0.2, 1.0],
            );
            PREVIEW_3D_GPU_VERTEX_BUDGET / 6 + 1
        ];

        let meshes =
            build_preview_3d_gpu_meshes(&faces, &[], &[], &[], [0.0, 0.0, 0.0], 16.0, 16.0, 0)
                .unwrap_or_else(|error| panic!("{error}"));
        let vertex_count = meshes
            .iter()
            .map(|mesh| mesh.gpu_mesh.vertices.len())
            .sum::<usize>();

        assert_eq!(meshes.len(), 2);
        assert_eq!(vertex_count, faces.len() * 6);
        assert!(
            meshes
                .iter()
                .all(|mesh| mesh.gpu_mesh.vertices.len() <= PREVIEW_3D_GPU_VERTEX_BUDGET)
        );
    }

    #[test]
    fn map_viewer_preview_3d_clear_resources_cancels_task_and_clears_state() {
        let cancel = CancelFlag::new();
        let cancel_probe = cancel.clone();
        let mut state = Preview3dState {
            status: Preview3dStatus::Ready,
            mesh: Some(Arc::new(test_mesh_with_faces(
                Vec::new(),
                SlimeChunkBounds {
                    dimension: bedrock_render::Dimension::Overworld,
                    min_chunk_x: 0,
                    max_chunk_x: 0,
                    min_chunk_z: 0,
                    max_chunk_z: 0,
                },
            ))),
            signature: Some(Preview3dSelectionSignature {
                bounds: SlimeChunkBounds {
                    dimension: bedrock_render::Dimension::Overworld,
                    min_chunk_x: 0,
                    max_chunk_x: 0,
                    min_chunk_z: 0,
                    max_chunk_z: 0,
                },
            }),
            movement_input: Preview3dMovementInput {
                forward: true,
                ..Preview3dMovementInput::default()
            },
            render_in_flight: true,
            cancel: Some(cancel),
            ..Preview3dState::default()
        };

        state.clear_resources(true);

        assert!(state.mesh.is_none());
        assert!(state.signature.is_none());
        assert!(!state.render_in_flight);
        assert!(state.cancel.is_none());
        assert!(!state.movement_input.any_active());
        assert!(cancel_probe.is_cancelled());
    }

    #[test]
    fn map_viewer_preview_3d_perspective_keeps_large_selection_depth_in_range() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 3,
            min_chunk_z: 0,
            max_chunk_z: 3,
        };
        let low_front = block_face(
            BlockKey { x: 0, y: 0, z: 0 },
            FACE_DEFINITIONS[0],
            [0.3, 0.7, 0.2, 1.0],
        );
        let high_back = block_face(
            BlockKey {
                x: 63,
                y: 96,
                z: 63,
            },
            FACE_DEFINITIONS[0],
            [0.7, 0.4, 0.2, 1.0],
        );
        let mesh = test_mesh_with_faces(vec![low_front, high_back], bounds);
        let gpu_mesh = test_gpu_mesh(&mesh);
        let camera = Preview3dCamera::new(0.55, 1.35, 1.0);
        let device_depths = gpu_mesh
            .vertices
            .iter()
            .map(|vertex| {
                project_preview_point(vertex.position, gpu_mesh.center, gpu_mesh.fit_scale, camera)
                    .2
            })
            .collect::<Vec<_>>();

        assert!(
            device_depths
                .iter()
                .all(|depth| *depth > 0.0 && *depth < 1.0)
        );
    }

    #[test]
    fn map_viewer_preview_3d_shader_projection_matches_rust_mirror() {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        };
        let face = block_face(
            BlockKey { x: 8, y: 72, z: 8 },
            FACE_DEFINITIONS[0],
            [0.35, 0.65, 0.22, 1.0],
        );
        let mesh = test_mesh_with_faces(vec![face], bounds);
        let gpu_mesh = test_gpu_mesh(&mesh);
        let camera = Preview3dCamera::new(0.25, Preview3dCamera::default().pitch, 1.7);
        let vertex = gpu_mesh.vertices[0];
        let projected =
            project_preview_point(vertex.position, gpu_mesh.center, gpu_mesh.fit_scale, camera);
        let gpu_camera = camera.gpu_camera();

        assert_eq!(gpu_camera.yaw, camera.yaw);
        assert_eq!(gpu_camera.pitch, camera.pitch);
        assert_eq!(gpu_camera.zoom, camera.zoom);
        assert!(projected.0.is_finite());
        assert!(projected.1.is_finite());
        assert!(projected.2.is_finite());
    }
}
