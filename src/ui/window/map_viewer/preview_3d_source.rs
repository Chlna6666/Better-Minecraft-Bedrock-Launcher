use super::model::{CopiedChunk, CopiedChunkData};
use bedrock_block_model::BlockModelRepository;
use bedrock_map_render::{RenderPalette, RgbaColor};
use bedrock_scene::{SceneBuilder, chunk, mesh::SceneMesh, structure};
use bedrock_world::ChunkPos;
use bedrock_world::chunk::SubChunkDecodeMode;
use bedrock_world::query::SlimeChunkBounds;
use bedrock_world::scan::ChunkValue;
use bedrock_world::surface::{
    BiomeDataRequirement, CancelFlag, ChunkData, ChunkDataRequest, ChunkLoadOptions,
    ChunkLoadPriority, WorldPipelineOptions, WorldThreadingOptions,
};
use bedrock_world::world::{OpenOptions, World};
use gpui::{Pixels, Point, SharedString};
use gpui_3d::{Camera, Mat4, Vec3};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

const MIN_PITCH: f32 = -std::f32::consts::PI + 0.02;
const MAX_PITCH: f32 = std::f32::consts::PI - 0.02;
const MODEL_MIN_PITCH: f32 = -std::f32::consts::FRAC_PI_2 + 0.02;
const MODEL_MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.02;
const VERTICAL_SCALE: f32 = 1.0;
const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 64.0;
const DEFAULT_DISTANCE: f32 = 3.0;
const FREE_MOVE_SPEED: f32 = 0.65;
const BASE_FOV_Y_RADIANS: f32 = 55.0_f32.to_radians();
const MIN_FOV_Y_RADIANS: f32 = 14.0_f32.to_radians();
const MAX_GEOMETRIC_ZOOM: f32 = 1.5;
const MIN_NEAR_PLANE: f32 = 0.02;
const FAR_PLANE: f32 = 256.0;
// A source update regenerates all currently loaded faces and the spatial
// region graph. Publish a few bounded snapshots, not dozens of ever smaller
// tail batches which repeatedly remesh the same earlier chunks.
const INCREMENTAL_TARGET_UPDATES: usize = 5;
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
        let pitch = wrap_pitch(pitch);
        Self {
            yaw,
            pitch,
            zoom,
            position: default_camera_position(yaw, pitch),
        }
    }

    pub(super) fn rotate_view(&mut self, delta_x: f32, delta_y: f32) {
        self.yaw = (self.yaw - delta_x * 0.01).rem_euclid(std::f32::consts::TAU);
        self.pitch = wrap_pitch(self.pitch + delta_y * 0.008);
    }

    pub(super) fn rotate_orbit(&mut self, delta_x: f32, delta_y: f32) {
        // Orbiting is not free-look: keep the camera on the sphere centered
        // around the model, otherwise the model drifts across the viewport
        // while yaw/pitch change at a fixed eye position.
        //
        // Preserve the independent free-movement offset (WASD/pan), so
        // orbiting after navigating does not teleport the user to the origin.
        let old_default = default_camera_position(self.yaw, self.pitch);
        let moved = vec3_sub(self.position, old_default);
        self.rotate_view(delta_x, delta_y);
        self.position = vec3_add(default_camera_position(self.yaw, self.pitch), moved);
    }

    pub(super) fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
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

        let speed = FREE_MOVE_SPEED;
        let movement = vec3_scale(
            vec3_normalize(movement),
            speed * delta_seconds.clamp(0.0, 0.05),
        );
        self.position = vec3_add(self.position, movement);
    }

    pub(super) fn forward(self) -> [f32; 3] {
        camera_forward(self.yaw, self.pitch)
    }

    pub(super) fn right(self) -> [f32; 3] {
        camera_right(self.yaw)
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
        self.yaw = (self.yaw - delta_x * 0.01).rem_euclid(std::f32::consts::TAU);
        self.pitch = wrap_model_pitch(self.pitch + delta_y * 0.008);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Preview3dDragMode {
    OrbitCamera,
    RotateModel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Preview3dDragState {
    pub(super) mode: Preview3dDragMode,
    pub(super) position: Point<Pixels>,
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
            "w" | "up" => &mut self.forward,
            "s" | "down" => &mut self.backward,
            "a" | "left" => &mut self.left,
            "d" | "right" => &mut self.right,
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

fn wrap_pitch(pitch: f32) -> f32 {
    if !pitch.is_finite() {
        return 0.0;
    }
    let wrapped =
        (pitch + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    wrapped.clamp(MIN_PITCH, MAX_PITCH)
}

fn wrap_model_pitch(pitch: f32) -> f32 {
    if !pitch.is_finite() {
        return 0.0;
    }
    pitch.clamp(MODEL_MIN_PITCH, MODEL_MAX_PITCH)
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
    pub(super) mesh: Option<Arc<SceneMesh>>,
    pub(super) signature: Option<Preview3dSelectionSignature>,
    pub(super) generation: u64,
    pub(super) drag: Option<Preview3dDragState>,
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
            drag: None,
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
        self.drag = None;
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

pub(super) const fn bounds_width(bounds: SlimeChunkBounds) -> i32 {
    bounds.max_chunk_x.saturating_sub(bounds.min_chunk_x) + 1
}

pub(super) const fn bounds_depth(bounds: SlimeChunkBounds) -> i32 {
    bounds.max_chunk_z.saturating_sub(bounds.min_chunk_z) + 1
}

/// Configuration for one incremental preview-3D mesh load. JS/TS-style parameter
/// object: one `load_mesh` entry instead of a family of `_with_*` variants.
pub(super) struct Preview3dLoadOptions<'a> {
    pub world_path: &'a Path,
    pub bounds: SlimeChunkBounds,
    pub block_models: Option<Arc<BlockModelRepository>>,
    pub cancel: Option<CancelFlag>,
    /// File export must not discard faces for real-time preview budgets.
    pub for_export: bool,
}

pub(super) fn load_mesh(
    options: Preview3dLoadOptions<'_>,
    mut update: impl FnMut(Arc<SceneMesh>, Preview3dBuildStatus) + Send + 'static,
) -> Result<SceneMesh, String> {
    let Preview3dLoadOptions {
        world_path,
        bounds,
        block_models,
        cancel,
        for_export,
    } = options;
    bounds.validate().map_err(|error| error.to_string())?;
    check_cancelled(cancel.as_ref())?;
    let world =
        World::open(world_path, OpenOptions::default()).map_err(|error| error.to_string())?;
    check_cancelled(cancel.as_ref())?;
    let (positions, truncated_chunk_count) = selection_chunk_positions(bounds);
    let chunk_total = positions.len();
    let mut builder = SceneBuilder::new_selection(bounds, truncated_chunk_count, for_export);
    let options = render_chunk_load_options(chunk_total, bounds, cancel.clone());
    let mut completed_chunks = 0usize;
    while completed_chunks < chunk_total {
        check_cancelled(cancel.as_ref())?;
        let count = incremental_batch_size(completed_chunks, chunk_total);
        let next_completed = (completed_chunks + count).min(chunk_total);
        let chunks = world
            .load_render_chunks(
                positions[completed_chunks..next_completed].iter().copied(),
                options.clone(),
            )
            .map_err(|error| error.to_string())?;
        let processed_chunks = chunks
            .into_par_iter()
            .map(|chunk| chunk::prepare(&chunk, block_models.as_deref()))
            .collect::<Result<Vec<_>, _>>()?;
        for processed_chunk in processed_chunks {
            builder.insert_prepared(processed_chunk);
        }
        completed_chunks = next_completed;
        if !for_export && should_emit_incremental_mesh(completed_chunks, chunk_total) {
            let status = Preview3dBuildStatus::new(
                "拼接模型",
                format!("{completed_chunks}/{chunk_total} chunks"),
            );
            emit_mesh_update(&mut builder, &mut update, status)?;
        }
    }

    builder.build_selection()
}

/// Snapshot source for one synchronous preview-3D mesh build.
///
/// Both variants produce the same mesh; callers pick the source they already
/// hold instead of choosing between `from_*_blocking` function variants.
pub(super) enum Preview3dMeshSource<'a> {
    Structure {
        structure: &'a bedrock_world::structure::McStructureFile,
        anchor_chunk: ChunkPos,
        origin_y: i32,
    },
    CopiedChunk(&'a CopiedChunkData),
}

pub(super) fn build_preview_mesh(source: Preview3dMeshSource<'_>) -> Result<SceneMesh, String> {
    match source {
        Preview3dMeshSource::Structure {
            structure,
            anchor_chunk,
            origin_y,
        } => structure::build(
            structure,
            anchor_chunk,
            origin_y,
            structure_preview_palette(),
        ),
        Preview3dMeshSource::CopiedChunk(copied_chunk) => load_copied_chunk_mesh(copied_chunk),
    }
}
fn structure_preview_palette() -> &'static RenderPalette {
    static PALETTE: OnceLock<RenderPalette> = OnceLock::new();
    PALETTE.get_or_init(|| {
        let mut palette = RenderPalette::default();
        // Converted models are colored by the voxelizer's 26.60 palette.
        // Override the builtin table so the 3D preview resolves exactly the
        // same appearance colors for every voxel candidate, including block
        // families the builtin table predates.
        let color =
            |channels: [u8; 4]| RgbaColor::new(channels[0], channels[1], channels[2], channels[3]);
        for candidate in bedrock_voxel::default_block_candidates() {
            palette.set_block_color(candidate.state.name.clone(), color(candidate.top_color));
            palette.set_block_side_color(candidate.state.name.clone(), color(candidate.side_color));
        }
        palette
    })
}

fn load_copied_chunk_mesh(copied_chunk: &CopiedChunkData) -> Result<SceneMesh, String> {
    let bounds = copied_chunk_3d_bounds(copied_chunk)?;
    bounds.validate().map_err(|error| error.to_string())?;
    let mut builder = SceneBuilder::new_selection(bounds, 0, false);
    for snapshot in &copied_chunk.chunks {
        let chunk = render_chunk_from_copied_snapshot(snapshot);
        let processed_chunk = chunk::prepare(&chunk, None)?;
        builder.insert_prepared(processed_chunk);
    }
    builder.build_selection()
}

fn render_chunk_load_options(
    total_chunks: usize,
    bounds: SlimeChunkBounds,
    cancel: Option<CancelFlag>,
) -> ChunkLoadOptions {
    ChunkLoadOptions {
        // 3D meshing only needs palette random access and biome storages. Avoid the
        // separate 256-column surface scan and retain packed palette words instead of
        // expanding 4096 `u16` indices for every storage.
        data_request: ChunkDataRequest::new()
            .full_3d_indices()
            .biome(BiomeDataRequirement::All)
            .block_entities(),
        subchunk_decode: SubChunkDecodeMode::PackedIndices,
        threading: world_threading(total_chunks),
        pipeline: WorldPipelineOptions {
            queue_depth: queue_depth(total_chunks),
            chunk_batch_size: chunk_batch_size(total_chunks),
            subchunk_decode_workers: subchunk_decode_workers(total_chunks),
            ..WorldPipelineOptions::default()
        },
        cancel,
        priority: ChunkLoadPriority::DistanceFrom {
            chunk_x: bounds.min_chunk_x + bounds_width(bounds) / 2,
            chunk_z: bounds.min_chunk_z + bounds_depth(bounds) / 2,
        },
        ..ChunkLoadOptions::default()
    }
}

fn check_cancelled(cancel: Option<&CancelFlag>) -> Result<(), String> {
    if cancel.is_some_and(CancelFlag::is_cancelled) {
        return Err("3D 预览任务已取消".to_string());
    }
    Ok(())
}

fn chunk_positions(bounds: SlimeChunkBounds) -> Vec<ChunkPos> {
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

fn selection_chunk_positions(bounds: SlimeChunkBounds) -> (Vec<ChunkPos>, usize) {
    let mut positions = chunk_positions(bounds);
    sort_positions_by_distance(
        &mut positions,
        bounds.min_chunk_x + bounds_width(bounds) / 2,
        bounds.min_chunk_z + bounds_depth(bounds) / 2,
    );
    (positions, 0)
}

fn sort_positions_by_distance(
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

fn bounds_chunk_count(bounds: SlimeChunkBounds) -> usize {
    usize::try_from(bounds_width(bounds))
        .unwrap_or(0)
        .saturating_mul(usize::try_from(bounds_depth(bounds)).unwrap_or(0))
}

fn queue_depth(total_chunks: usize) -> usize {
    total_chunks.clamp(16, 256)
}

fn chunk_batch_size(total_chunks: usize) -> usize {
    total_chunks.clamp(16, 128)
}

fn world_threading(total_chunks: usize) -> WorldThreadingOptions {
    if total_chunks <= 1 {
        WorldThreadingOptions::Single
    } else {
        WorldThreadingOptions::Auto
    }
}

fn subchunk_decode_workers(total_chunks: usize) -> usize {
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

fn incremental_batch_size(completed_chunks: usize, total_chunks: usize) -> usize {
    let remaining = total_chunks.saturating_sub(completed_chunks);
    if remaining == 0 {
        return 0;
    }
    let first_batch = total_chunks.min(4);
    if completed_chunks == 0 {
        return first_batch;
    }
    // Recompute a fixed *target* batch width from the original total, not
    // the remaining count. Dividing the remainder at every iteration
    // generated a long tail of 1-2 chunk batches, each triggering a full
    // mesh/LOD/spatial-region rebuild (quadratic accumulated CPU work).
    let later_batches = INCREMENTAL_TARGET_UPDATES.saturating_sub(1).max(1);
    let later_total = total_chunks.saturating_sub(first_batch);
    remaining.min(later_total.div_ceil(later_batches).max(1))
}

fn should_emit_incremental_mesh(completed_chunks: usize, total_chunks: usize) -> bool {
    // The final result is returned by the loader and converted once by the caller.
    // Publishing it here would rebuild the full source mesh and spatial regions twice.
    completed_chunks > 0 && completed_chunks < total_chunks
}

fn emit_mesh_update(
    builder: &mut SceneBuilder,
    update: &mut impl FnMut(Arc<SceneMesh>, Preview3dBuildStatus),
    status: Preview3dBuildStatus,
) -> Result<(), String> {
    let mesh = Arc::new(builder.build_selection()?);
    if mesh.surface_face_count() != 0 {
        update(mesh, status);
    }
    Ok(())
}
fn render_chunk_from_copied_snapshot(snapshot: &CopiedChunk) -> ChunkData {
    let parsed = ::bedrock_world::scan::Chunk::new(
        snapshot.chunk,
        &snapshot.records,
        copied_chunk_scan_options(),
    );
    let mut subchunks = BTreeMap::new();
    let mut legacy_terrain = None;
    let mut version = bedrock_world::ChunkVersion::New;
    for record in parsed.records {
        match record.value {
            ChunkValue::SubChunk(subchunk) => {
                subchunks.insert(subchunk.y, subchunk);
            }
            ChunkValue::LegacyTerrain(terrain) => {
                legacy_terrain = Some(terrain);
                version = bedrock_world::ChunkVersion::Old;
            }
            ChunkValue::Version(version_byte) => {
                if version_byte < 25 {
                    version = bedrock_world::ChunkVersion::Old;
                }
            }
            _ => {}
        }
    }
    ChunkData {
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

fn copied_chunk_scan_options() -> bedrock_world::scan::ScanOptions {
    bedrock_world::scan::ScanOptions {
        categories: bedrock_world::scan::ScanCategories {
            chunks: true,
            players: false,
            actors: false,
            maps: false,
            villages: false,
            globals: false,
        },
        retention: bedrock_world::scan::RetentionMode::Structured,
        subchunk_decode_mode: SubChunkDecodeMode::FullIndices,
        actor_resolution: bedrock_world::scan::ActorResolution::None,
    }
}

fn zoom_projection(zoom: f32) -> (f32, f32) {
    let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    let geometric_zoom = zoom.min(MAX_GEOMETRIC_ZOOM);
    let optical_zoom = (zoom / geometric_zoom).max(1.0);
    let vertical_fov =
        (BASE_FOV_Y_RADIANS / optical_zoom).clamp(MIN_FOV_Y_RADIANS, BASE_FOV_Y_RADIANS);
    (geometric_zoom, vertical_fov)
}

fn near_plane(geometric_zoom: f32) -> f32 {
    (0.06 / geometric_zoom.max(1.0)).clamp(MIN_NEAR_PLANE, 0.06)
}
#[cfg(test)]
fn project_preview_point(
    point: [f32; 3],
    center: [f32; 3],
    scale: f32,
    camera: Preview3dCamera,
) -> (f32, f32, f32) {
    let view_proj_model = test_view_proj_model(center, scale, camera);
    let projected = mat4_mul_vec4(view_proj_model, [point[0], point[1], point[2], 1.0]);
    let reciprocal_w = 1.0 / projected[3].max(0.0001);
    (
        projected[0] * reciprocal_w,
        projected[1] * reciprocal_w,
        projected[2] * reciprocal_w,
    )
}

fn view_proj_model(
    aspect: f32,
    center: [f32; 3],
    scale: f32,
    camera: Preview3dCamera,
    model_rotation: Preview3dModelRotation,
) -> [[f32; 4]; 4] {
    let pitch = wrap_pitch(camera.pitch);
    let eye = camera.position;
    let look_at = vec3_add(eye, camera.forward());
    let view = mat4_look_at(eye, look_at, camera_up(pitch));
    let (geometric_zoom, vertical_fov) = zoom_projection(camera.zoom);
    let proj = mat4_perspective(
        aspect.max(0.1),
        vertical_fov,
        near_plane(geometric_zoom),
        FAR_PLANE,
    );
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
            } * geometric_zoom,
            scale * VERTICAL_SCALE * geometric_zoom,
            if model_rotation.mirror_z {
                -scale
            } else {
                scale
            } * geometric_zoom,
        ]),
        mat4_mul(
            rotation,
            mat4_translation([-center[0], -center[1], -center[2]]),
        ),
    );

    mat4_mul(mat4_mul(proj, view), model)
}

pub(super) fn viewport_parameters(
    center: [f32; 3],
    fit_scale: f32,
    camera: Preview3dCamera,
    model_rotation: Preview3dModelRotation,
) -> (Camera, Mat4) {
    let pitch = wrap_pitch(camera.pitch);
    let eye = Vec3::new(camera.position[0], camera.position[1], camera.position[2]);
    let forward = camera.forward();
    let target = eye + Vec3::new(forward[0], forward[1], forward[2]);
    let up = camera_up(pitch);
    let (geometric_zoom, vertical_fov) = zoom_projection(camera.zoom);
    let camera = Camera::perspective(
        eye,
        target,
        Vec3::new(up[0], up[1], up[2]),
        vertical_fov,
        near_plane(geometric_zoom),
        FAR_PLANE,
    );
    let scene_transform = mat4_mul(
        mat4_scale([
            if model_rotation.mirror_x {
                -fit_scale
            } else {
                fit_scale
            } * geometric_zoom,
            fit_scale * VERTICAL_SCALE * geometric_zoom,
            if model_rotation.mirror_z {
                -fit_scale
            } else {
                fit_scale
            } * geometric_zoom,
        ]),
        mat4_mul(
            mat4_mul(
                mat4_rotation_y(model_rotation.yaw),
                mat4_rotation_x(model_rotation.pitch),
            ),
            mat4_translation([-center[0], -center[1], -center[2]]),
        ),
    );
    (camera, Mat4::from_columns(scene_transform))
}

fn draw_parameters(
    aspect: f32,
    center: [f32; 3],
    fit_scale: f32,
    camera: Preview3dCamera,
    model_rotation: Preview3dModelRotation,
) -> [[f32; 4]; 4] {
    view_proj_model(aspect, center, fit_scale, camera, model_rotation)
}

#[cfg(test)]
fn test_view_proj_model(center: [f32; 3], scale: f32, camera: Preview3dCamera) -> [[f32; 4]; 4] {
    view_proj_model(
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
        [0.0, 0.0, far / depth, -1.0],
        [0.0, 0.0, far * near / depth, 0.0],
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

fn camera_up(pitch: f32) -> [f32; 3] {
    if pitch.cos().is_sign_negative() {
        [0.0, -1.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    }
}

fn camera_forward(yaw: f32, pitch: f32) -> [f32; 3] {
    let pitch = wrap_pitch(pitch);
    let horizontal_distance = pitch.cos();
    vec3_normalize([
        -yaw.sin() * horizontal_distance,
        -pitch.sin(),
        -yaw.cos() * horizontal_distance,
    ])
}

fn camera_right(yaw: f32) -> [f32; 3] {
    vec3_normalize([yaw.cos(), 0.0, -yaw.sin()])
}

fn default_camera_position(yaw: f32, pitch: f32) -> [f32; 3] {
    let pitch = wrap_pitch(pitch);
    let horizontal_distance = DEFAULT_DISTANCE * pitch.cos();
    [
        horizontal_distance * yaw.sin(),
        DEFAULT_DISTANCE * pitch.sin(),
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

#[cfg(test)]
mod tests;
