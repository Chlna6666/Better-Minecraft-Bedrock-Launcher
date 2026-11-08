//! Background chunk streaming, textured meshing and probe construction for the 3D editor.
//!
//! LevelDB reads, block-state decoding and material resolution happen on this worker.
//! GPU-capable sections publish compact voxel cells and face templates directly,
//! without first constructing and encoding a full CPU triangle mesh. The UI receives
//! immutable section deltas and never blocks on world I/O.

use super::frame_display::{FrameDisplay, ITEM_MATERIAL_PREFIX, MAP_MATERIAL_PREFIX};
use super::types::{
    ChunkServiceCommand, ChunkServiceEvent, ChunkServiceInfo, ChunkServiceStatus, PROBE_FLAG_FLUID,
    PROBE_FLAG_SOLID, ProbingSubChunk, WORLD3D_MAX_VIEW_RADIUS, World3dChunkCompletion,
    World3dChunkMesh, World3dChunkPayload, World3dChunkProbe,
};
use crate::ui::window::map_viewer::preview_3d_source as p3d;
use bedrock_block_model::{
    MATERIAL_SLOT_SEPARATOR, ObjResolvedTexture, ObjTextureResolver, obj_material_texture_name,
    vanilla_resource_pack_roots_from_packages,
};
use bedrock_render::{ChunkPos, Dimension};
use bedrock_world::block::BlockState;
use bedrock_world::chunk::{ChunkVersion, SubChunkFormat};
use bedrock_world::map_item::MapItemId;
use bedrock_world::surface::ChunkData;
use bedrock_world::surface::{
    BiomeDataRequirement, CancelFlag, ChunkDataRequest, ChunkLoadOptions, ChunkLoadPriority,
    ChunkLoadStats, WorldPipelineOptions, WorldThreadingOptions,
};
use bedrock_world::world::{OpenOptions, World};
use bedrock_world::{SlimeChunkBounds, SubChunkDecodeMode};
use gpui_3d::{AlphaMode, Material, Mesh, MeshId, MeshPart, TextureAsset, Vec2, Vec3, Vertex};
use image::RgbaImage;
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(test)]
pub(super) mod material_audit;
mod mesh_assets;
mod pbr;
mod water;
use mesh_assets::MeshAssets;
mod gpu_cubes;
mod lighting;
mod occluders;
mod prefetch;
use prefetch::{BatchWait, PendingChunkBatch};

const LOAD_BATCH_CHUNKS: usize = 16;
// Keep wide LevelDB read batches, but publish CPU mesh/texture work in small
// camera-priority waves. Commands are observed between waves, so a rapid view
// turn never forces finishing all 16 old-camera mesh builds.
const STREAM_MESH_BATCH_CHUNKS: usize = 4;
const REMESH_BATCH_CHUNKS: usize = 8;
const MAX_PENDING_MESH_EVENTS: usize = 16;
const CACHE_HYSTERESIS_CHUNKS: i32 = 2;
const MESH_BAND_BLOCKS: i32 = 64;
const MESH_BAND_HEIGHT: f64 = MESH_BAND_BLOCKS as f64;
const FAR_LOD_SCALE: i32 = 4;
const TEXTURE_MAX_DIMENSION: u32 = 4096;
const CHUNK_MESH_ID_SALT: u64 = 0x574f_524c_4433_444d;
const MATERIAL_INDEX_FLUSH_INTERVAL: Duration = Duration::from_secs(5);

fn material_index_flush_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|last| now.saturating_duration_since(last) >= MATERIAL_INDEX_FLUSH_INTERVAL)
}

#[cfg(test)]
#[test]
fn material_index_flush_coalesces_status_updates() {
    let first = Instant::now();
    assert!(material_index_flush_due(None, first));
    assert!(!material_index_flush_due(
        Some(first),
        first + MATERIAL_INDEX_FLUSH_INTERVAL - Duration::from_millis(1)
    ));
    assert!(material_index_flush_due(
        Some(first),
        first + MATERIAL_INDEX_FLUSH_INTERVAL
    ));
}

mod texture_mips;
#[cfg(test)]
use texture_mips::downsample_texture;
use texture_mips::{mipmapped_data_texture, mipmapped_texture};

fn texture_storage_bytes(texture: &TextureAsset) -> usize {
    (0..texture.mip_level_count())
        .filter_map(|level| texture.mip_pixels(level))
        .map(<[u8]>::len)
        .sum()
}

/// Configuration for one 3D editor chunk worker.
pub struct ChunkServiceConfig {
    pub world_path: PathBuf,
    pub world_name: String,
    pub dimension: Dimension,
    pub package_roots: Vec<PathBuf>,
    pub view_radius: i32,
}

#[derive(Default)]
struct PendingViews {
    views: VecDeque<ChunkServiceCommand>,
    can_replace_last: bool,
}

/// UI-side handle of the background chunk worker.
pub struct ChunkServiceHandle {
    commands: Sender<ChunkServiceCommand>,
    pending_views: Arc<Mutex<PendingViews>>,
    events: Option<Receiver<ChunkServiceEvent>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ChunkServiceHandle {
    /// Queues a command; pending camera views are replaced by the newest one.
    pub fn send(&self, command: ChunkServiceCommand) -> bool {
        let mut pending = self
            .pending_views
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if matches!(command, ChunkServiceCommand::SetView { .. }) {
            if pending.can_replace_last {
                if let Some(last) = pending.views.back_mut() {
                    *last = command;
                    return true;
                }
            }
            pending.views.push_back(command);
            pending.can_replace_last = true;
            if self
                .commands
                .send(ChunkServiceCommand::ViewUpdated)
                .is_err()
            {
                pending.views.pop_back();
                return false;
            }
            true
        } else {
            pending.can_replace_last = false;
            self.commands.send(command).is_ok()
        }
    }

    /// Pops one pending worker event without blocking.
    pub fn try_recv(&self) -> Option<ChunkServiceEvent> {
        self.events.as_ref()?.try_recv().ok()
    }

    /// Requests worker shutdown. The worker exits after the current chunk.
    pub fn shutdown(&mut self) {
        let _ = self.commands.send(ChunkServiceCommand::Shutdown);
        // Release a worker blocked on a full event queue before detaching it.
        drop(self.events.take());
        if let Some(thread) = self.thread.take() {
            // The worker checks the shutdown command between chunks; joining here would
            // block the UI thread. Detach instead and let the process reclaim it.
            drop(thread);
        }
    }
}

impl Drop for ChunkServiceHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Starts the background worker. World opening happens on the worker thread.
pub fn spawn_chunk_service(config: ChunkServiceConfig) -> Result<ChunkServiceHandle, String> {
    let (command_tx, command_rx) = channel();
    let (event_tx, event_rx) = sync_channel(MAX_PENDING_MESH_EVENTS);
    let pending_views = Arc::new(Mutex::new(PendingViews::default()));
    let worker_views = Arc::clone(&pending_views);
    let thread = std::thread::Builder::new()
        .name(format!(
            "world3d-stream-{}",
            sanitize_thread_name(&config.world_name)
        ))
        .stack_size(8 * 1024 * 1024)
        .spawn(move || run_worker(config, command_rx, event_tx, worker_views))
        .map_err(|error| format!("启动 3D 区块线程失败：{error}"))?;
    Ok(ChunkServiceHandle {
        commands: command_tx,
        pending_views,
        events: Some(event_rx),
        thread: Some(thread),
    })
}

fn sanitize_thread_name(name: &str) -> String {
    name.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .take(24)
        .collect()
}

#[derive(Clone, Copy, Debug)]
struct ViewState {
    generation: u64,
    dimension: Dimension,
    camera: [f64; 3],
    forward_xz: [f32; 2],
    forward_y: f32,
    radius: i32,
    view_distance: f32,
}

#[derive(Default)]
struct ChunkCursor {
    ring: i32,
    offset: usize,
    ring_positions: Vec<(i32, i32)>,
}

impl ChunkCursor {
    /// Re-rank only the unconsumed part of the current ring. Changing yaw
    /// must not restart DB reads or revisit already considered columns.
    fn reprioritize_remaining(&mut self, forward_xz: [f32; 2]) {
        self.ring_positions[self.offset..].sort_by(|&(ax, az), &(bx, bz)| {
            let a = ax as f32 * forward_xz[0] + az as f32 * forward_xz[1];
            let b = bx as f32 * forward_xz[0] + bz as f32 * forward_xz[1];
            b.total_cmp(&a).then_with(|| (ax, az).cmp(&(bx, bz)))
        });
    }

    fn next(
        &mut self,
        dimension: Dimension,
        center_x: i32,
        center_z: i32,
        radius: i32,
        forward_xz: [f32; 2],
    ) -> Option<ChunkPos> {
        (self.ring <= radius)
            .then(|| self.next_in_ring(dimension, center_x, center_z, radius, forward_xz))
    }

    fn next_in_ring(
        &mut self,
        dimension: Dimension,
        center_x: i32,
        center_z: i32,
        radius: i32,
        forward_xz: [f32; 2],
    ) -> ChunkPos {
        if self.ring_positions.is_empty() {
            let r = self.ring;
            if r == 0 {
                self.ring_positions.push((0, 0));
            } else {
                for dx in -r..=r {
                    self.ring_positions.push((dx, -r));
                    self.ring_positions.push((dx, r));
                }
                for dz in (-r + 1)..r {
                    self.ring_positions.push((-r, dz));
                    self.ring_positions.push((r, dz));
                }
                self.ring_positions.retain(|&(dx, dz)| {
                    super::chunk_range::contains(i64::from(dx), i64::from(dz), radius)
                });
                self.ring_positions.sort_by(|&(ax, az), &(bx, bz)| {
                    let a = ax as f32 * forward_xz[0] + az as f32 * forward_xz[1];
                    let b = bx as f32 * forward_xz[0] + bz as f32 * forward_xz[1];
                    b.total_cmp(&a).then_with(|| (ax, az).cmp(&(bx, bz)))
                });
            }
        }
        let (dx, dz) = self.ring_positions[self.offset];
        self.offset += 1;
        if self.offset == self.ring_positions.len() {
            self.ring += 1;
            self.offset = 0;
            self.ring_positions.clear();
        }
        ChunkPos {
            x: center_x + dx,
            z: center_z + dz,
            dimension,
        }
    }
}

#[cfg(test)]
fn desired_chunk_positions(
    dimension: Dimension,
    center_x: i32,
    center_z: i32,
    radius: i32,
) -> BTreeSet<ChunkPos> {
    let mut positions = BTreeSet::new();
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            if !super::chunk_range::contains(i64::from(dx), i64::from(dz), radius) {
                continue;
            }
            positions.insert(ChunkPos {
                x: center_x + dx,
                z: center_z + dz,
                dimension,
            });
        }
    }
    positions
}

fn inside_cache_hysteresis(
    pos: ChunkPos,
    dimension: Dimension,
    center_x: i32,
    center_z: i32,
    radius: i32,
) -> bool {
    let margin = (radius / 4).clamp(1, CACHE_HYSTERESIS_CHUNKS);
    inside_view(
        pos,
        dimension,
        center_x,
        center_z,
        radius.saturating_add(margin),
    )
}

fn prioritize_stream_meshes(
    loaded: &mut [ChunkPos],
    center: (i32, i32),
    forward_xz: [f32; 2],
) {
    // The loader may return chunks in DB scheduling order. A sorted meshing
    // wave should always favor the current view, not the on-disk key order.
    let selected = loaded.iter().copied().collect::<HashSet<_>>();
    loaded.sort_by_key(|pos| mesh_target_priority(*pos, &selected, center, forward_xz));
}

fn mesh_target_priority(
    pos: ChunkPos,
    loaded: &std::collections::HashSet<ChunkPos>,
    center: (i32, i32),
    forward_xz: [f32; 2],
) -> (bool, bool, i64) {
    let dx = i64::from(pos.x) - i64::from(center.0);
    let dz = i64::from(pos.z) - i64::from(center.1);
    let distance_sq = dx * dx + dz * dz;
    let ahead =
        dx as f32 * forward_xz[0] + dz as f32 * forward_xz[1] >= 0.25 * (distance_sq as f32).sqrt();
    (!loaded.contains(&pos), !ahead, distance_sq)
}

#[inline]
fn lazy_meshing_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| std::env::var_os("BMCBL_WORLD3D_DISABLE_LAZY_MESH").is_some())
}

#[inline]
pub(super) fn portal_culling_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        std::env::var_os("BMCBL_WORLD3D_DISABLE_PORTAL_VISIBILITY").is_some()
    })
}

/// Background columns in the rear hemisphere can defer expensive
/// block-model expansion, texture resolution and GPU Section allocation.
/// A generous 4-column near field is always built eagerly. The 120-degree
/// rear cone is deliberately smaller than a full rear half-space so FOV,
/// pitch changes and seam visibility cannot cause obvious near-camera holes.
/// Cached decoded probes remain available for picking and world queries.
fn lazy_mesh_column(pos: ChunkPos, view: ViewState) -> bool {
    if lazy_meshing_disabled() {
        return false;
    }
    if pos.dimension != view.dimension {
        return false;
    }
    let dx = f64::from(pos.x) * 16.0 + 8.0 - view.camera[0];
    let dz = f64::from(pos.z) * 16.0 + 8.0 - view.camera[2];
    let distance_sq = dx * dx + dz * dz;
    if distance_sq < (5.0 * 16.0_f64).powi(2) {
        return false;
    }
    let [fx, fz] = view.forward_xz;
    let forward_length = f64::from(fx).hypot(f64::from(fz));
    if forward_length < 0.5 {
        // Looking nearly vertically: don't skip columns based on yaw.
        return false;
    }
    let dot = (dx * f64::from(fx) + dz * f64::from(fz))
        / (distance_sq.sqrt() * forward_length);
    dot < -0.55
}

fn section_priority(pos: ChunkPos, section_y: i32, view: ViewState) -> (bool, u64) {
    let dx = f64::from(pos.x) * 16.0 + 8.0 - view.camera[0];
    let dy = f64::from(section_y) * MESH_BAND_HEIGHT + MESH_BAND_HEIGHT * 0.5 - view.camera[1];
    let dz = f64::from(pos.z) * 16.0 + 8.0 - view.camera[2];
    let distance_sq = dx * dx + dy * dy + dz * dz;
    let forward = dx * f64::from(view.forward_xz[0])
        + dy * f64::from(view.forward_y)
        + dz * f64::from(view.forward_xz[1]);
    (
        distance_sq > MESH_BAND_HEIGHT * MESH_BAND_HEIGHT && forward < 0.0,
        distance_sq as u64,
    )
}

fn coarse_lod_for_view(pos: ChunkPos, view: ViewState, _was_coarse: bool) -> bool {
    let center_x = (view.camera[0] / 16.0).floor() as i32;
    let center_z = (view.camera[2] / 16.0).floor() as i32;
    // The selected circular radius is the full-detail region. A coarse mesh must
    // switch back immediately when it enters that region, even if it was already
    // coarse; hysteresis there would leave visibly inaccurate blocks inside it.
    !inside_view(pos, view.dimension, center_x, center_z, view.radius.max(1))
}

fn partition_section_faces(
    pos: ChunkPos,
    faces: Vec<p3d::Preview3dStreamFace>,
    view: ViewState,
) -> Vec<(i32, Vec<p3d::Preview3dStreamFace>)> {
    let mut sections = BTreeMap::<i32, Vec<p3d::Preview3dStreamFace>>::new();
    for face in faces {
        sections.entry(face_band_y(&face)).or_default().push(face);
    }
    let mut sections = sections.into_iter().collect::<Vec<_>>();
    sections.sort_by_key(|(section_y, _)| section_priority(pos, *section_y, view));
    sections
}

fn neighbor_affected_bands(min_y: i32, max_y: i32) -> (i32, i32) {
    (
        (min_y - 15).div_euclid(MESH_BAND_BLOCKS),
        (max_y + 15).div_euclid(MESH_BAND_BLOCKS),
    )
}

// A deferred column still owns real neighbor geometry. It may skip its own
// GPU mesh, but must provide a full voxel/boundary mask while an adjacent
// target is meshed; otherwise internal seam faces become visible until the
// delayed column is eventually promoted.
fn needs_mesh_neighbor_hydration(
    is_target: bool,
    has_blocks: bool,
    has_boundary: bool,
) -> bool {
    !has_blocks && (is_target || !has_boundary)
}

const CHUNK_NEIGHBOR_OFFSETS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

fn mesh_target_positions(
    changed: &[ChunkPos],
    include_neighbors: bool,
    affects: impl Fn(&ChunkPos, &ChunkPos) -> bool,
) -> BTreeSet<ChunkPos> {
    let mut targets = BTreeSet::new();
    for pos in changed {
        targets.insert(*pos);
        if include_neighbors {
            for (dx, dz) in CHUNK_NEIGHBOR_OFFSETS {
                let neighbor = ChunkPos {
                    x: pos.x + dx,
                    z: pos.z + dz,
                    dimension: pos.dimension,
                };
                if affects(pos, &neighbor) {
                    targets.insert(neighbor);
                }
            }
        }
    }
    targets
}

/// Canonical unordered boundary edge. A completed pair is refreshed once rather
/// than permanently suppressing all future neighbour updates for either column.
fn record_boundary_pair(
    pairs: &mut BTreeSet<(ChunkPos, ChunkPos)>,
    a: ChunkPos,
    b: ChunkPos,
) -> bool {
    pairs.insert(if a <= b { (a, b) } else { (b, a) })
}

fn forget_boundary_pairs(pairs: &mut BTreeSet<(ChunkPos, ChunkPos)>, pos: ChunkPos) {
    // A column has at most four shared edges. Remove those directly instead
    // of scanning every active edge on each streaming unload (quadratic churn).
    for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        let (Some(x), Some(z)) = (pos.x.checked_add(dx), pos.z.checked_add(dz)) else {
            continue;
        };
        let neighbour = ChunkPos { x, z, ..pos };
        let key = if pos <= neighbour {
            (pos, neighbour)
        } else {
            (neighbour, pos)
        };
        pairs.remove(&key);
    }
}

fn forget_published_sections(
    published: &mut BTreeMap<(ChunkPos, i32), Weak<World3dChunkMesh>>,
    pos: ChunkPos,
    keep: &[i32],
) {
    let stale = published
        .range((pos, i32::MIN)..=(pos, i32::MAX))
        .filter_map(|(key, _)| (!keep.contains(&key.1)).then_some(*key))
        .collect::<Vec<_>>();
    for key in stale {
        published.remove(&key);
    }
}

/// Render state also includes Hi-Z occluder boxes. A change in those boxes
/// needs a new Scene snapshot but does NOT require re-uploading the same voxel
/// mesh to the GPU. Keep the two comparisons separate.
fn same_chunk_render_geometry(left: &World3dChunkMesh, right: &World3dChunkMesh) -> bool {
    left.occluders == right.occluders && same_chunk_mesh_contents(left, right)
}

fn same_chunk_mesh_contents(left: &World3dChunkMesh, right: &World3dChunkMesh) -> bool {
    left.pos == right.pos
        && left.section_y == right.section_y
        && left.origin == right.origin
        && left.faces == right.faces
        && left.triangles == right.triangles
        && left.missing_texture_faces == right.missing_texture_faces
        // A fresh Material::new() has a different ID even when its shader
        // parameters and all texture bindings are bit-for-bit identical.
        // Treat only actual visual changes as Section-content invalidation.
        && left.materials.len() == right.materials.len()
        && left.materials.iter().zip(right.materials.iter())
            .all(|(a, b)| a.render_content_eq(b))
        && left.textures.iter().map(|texture| texture.id())
            .eq(right.textures.iter().map(|texture| texture.id()))
        && left.mesh.same_voxel_quad_geometry(&right.mesh)
}

/// Preserve the existing MeshId/generation and native GPU geometry even if
/// the Section's Hi-Z boxes changed after a neighbouring column was loaded.
/// New scene occluders still flow through the ordinary Section update.
fn reuse_section_gpu_mesh(
    previous: &World3dChunkMesh,
    updated: &mut Arc<World3dChunkMesh>,
) -> bool {
    if !same_chunk_mesh_contents(previous, updated) {
        return false;
    }
    let resident = Arc::make_mut(updated);
    resident.mesh = Arc::clone(&previous.mesh);
    // Reuse immutable MaterialId snapshots as well: otherwise the newly
    // allocated IDs would dirty DrawParams on every unchanged remesh.
    resident.materials = Arc::clone(&previous.materials);
    true
}

/// Select only the nearest K visible columns from a potentially large
/// backlog. The original code cloned and sorted the entire visible set even
/// when the caller could submit only a handful of Sections per batch.
///
/// A max-heap keeps O(K) temporary records and O(N log K) CPU work. Tie
/// ordering follows ChunkPos's BTreeSet order, preserving deterministic
/// center-first publication and the exact result of a stable full sort.
fn nearest_view_positions(
    positions: impl IntoIterator<Item = ChunkPos>,
    dimension: Dimension,
    center_x: i32,
    center_z: i32,
    radius: i32,
    limit: usize,
) -> Vec<ChunkPos> {
    if limit == 0 {
        return Vec::new();
    }
    let mut nearest = std::collections::BinaryHeap::with_capacity(limit);
    for pos in positions {
        if !inside_view(pos, dimension, center_x, center_z, radius) {
            continue;
        }
        let dx = i64::from(pos.x) - i64::from(center_x);
        let dz = i64::from(pos.z) - i64::from(center_z);
        let key = (dx * dx + dz * dz, pos);
        if nearest.len() < limit {
            nearest.push(key);
        } else if nearest.peek().is_some_and(|&worst| key < worst) {
            nearest.pop();
            nearest.push(key);
        }
    }
    nearest.into_sorted_vec().into_iter().map(|(_, pos)| pos).collect()
}

fn take_visible_dirty_meshes(
    dirty: &mut BTreeSet<ChunkPos>,
    dimension: Dimension,
    center_x: i32,
    center_z: i32,
    radius: i32,
    limit: usize,
) -> Vec<ChunkPos> {
    let visible = nearest_view_positions(
        dirty.iter().copied(), dimension, center_x, center_z, radius, limit,
    );
    for pos in &visible {
        dirty.remove(pos);
    }
    visible
}

fn visible_dirty_meshes(
    dirty: &BTreeSet<ChunkPos>,
    dimension: Dimension,
    center_x: i32,
    center_z: i32,
    radius: i32,
) -> Vec<ChunkPos> {
    let mut visible = dirty
        .iter()
        .copied()
        .filter(|pos| inside_view(*pos, dimension, center_x, center_z, radius))
        .collect::<Vec<_>>();
    visible.sort_by_key(|pos| {
        let dx = i64::from(pos.x) - i64::from(center_x);
        let dz = i64::from(pos.z) - i64::from(center_z);
        dx * dx + dz * dz
    });
    visible
}

/// A dirty rear Section remains dirty until the camera makes it relevant.
/// This mirrors JE's visible-first dirty rebuild queue: the underlying
/// immutable world/Probe is never discarded, and the old resident GPU mesh
/// stays valid as a fallback until the replacement is ready.
/// Streaming priority: immediate camera neighborhood, then four-column
/// radial groups ordered by the camera forward cone. Unlike a global
/// forward-first sort, a remote forward chunk cannot starve a nearby side
/// column after a fast turn. The key is deterministic and cheap to recompute
/// when the camera changes; no persistent stale priority queue is required.
fn renderable_dirty_priority(
    pos: ChunkPos,
    view: ViewState,
) -> (u8, i64, u8, i64, ChunkPos) {
    let center_x = (view.camera[0] / 16.0).floor() as i32;
    let center_z = (view.camera[2] / 16.0).floor() as i32;
    let dx = i64::from(pos.x) - i64::from(center_x);
    let dz = i64::from(pos.z) - i64::from(center_z);
    let distance_sq = dx * dx + dz * dz;
    let ring = dx.abs().max(dz.abs());
    // The inner four-column neighborhood is always distance-first, even
    // behind the camera: teleports, turns and noclip must not expose holes.
    if ring <= 4 {
        return (0, 0, 0, distance_sq, pos);
    }
    let forward = dx as f32 * view.forward_xz[0] + dz as f32 * view.forward_xz[1];
    let radius = (distance_sq as f32).sqrt();
    let direction = if view.forward_xz[0].hypot(view.forward_xz[1]) < 0.5 {
        1
    } else if forward >= radius * 0.25 {
        0
    } else if forward < -radius * 0.55 {
        2
    } else {
        1
    };
    (1, (ring - 5) / 4, direction, distance_sq, pos)
}

fn renderable_dirty_meshes(
    dirty: &BTreeSet<ChunkPos>,
    view: ViewState,
) -> Vec<ChunkPos> {
    let center_x = (view.camera[0] / 16.0).floor() as i32;
    let center_z = (view.camera[2] / 16.0).floor() as i32;
    let mut ready = dirty.iter().copied()
        .filter(|pos| inside_view(*pos, view.dimension, center_x, center_z, view.radius))
        .filter(|pos| !lazy_mesh_column(*pos, view))
        .collect::<Vec<_>>();
    ready.sort_by_key(|pos| renderable_dirty_priority(*pos, view));
    ready
}

fn take_renderable_dirty_meshes(
    dirty: &mut BTreeSet<ChunkPos>,
    view: ViewState,
    limit: usize,
) -> Vec<ChunkPos> {
    if limit == 0 {
        return Vec::new();
    }
    let center_x = (view.camera[0] / 16.0).floor() as i32;
    let center_z = (view.camera[2] / 16.0).floor() as i32;
    // O(N log K) selection, same order as the full dirty-drain path.
    // The worker interleaves these tiny waves with DB reads, and a turn
    // automatically changes the next selection without requeueing assets.
    let mut nearest = std::collections::BinaryHeap::with_capacity(limit);
    for pos in dirty.iter().copied() {
        if !inside_view(pos, view.dimension, center_x, center_z, view.radius)
            || lazy_mesh_column(pos, view)
        {
            continue;
        }
        let key = renderable_dirty_priority(pos, view);
        if nearest.len() < limit {
            nearest.push(key);
        } else if nearest.peek().is_some_and(|&worst| key < worst) {
            nearest.pop();
            nearest.push(key);
        }
    }
    let ready = nearest.into_sorted_vec().into_iter()
        .map(|(_, _, _, _, pos)| pos).collect::<Vec<_>>();
    for pos in &ready {
        dirty.remove(pos);
    }
    ready
}

/// Feedback from the previous mesh/upload wave. A slow GPU owner or complex
/// transparent model reduces the next CPU wave, without reducing frame rate
/// or allowing the dirty queue to stall completely.
fn interleaved_mesh_wave_size(face_ms: u64, convert_ms: u64, send_wait_ms: u64) -> usize {
    let work = face_ms.saturating_add(convert_ms).saturating_add(send_wait_ms);
    if work >= 16 { 1 } else if work >= 8 { 2 } else { STREAM_MESH_BATCH_CHUNKS }
}

fn retain_urgent_chunks(urgent: &mut BTreeSet<ChunkPos>, view: ViewState) {
    let center_x = (view.camera[0] / 16.0).floor() as i32;
    let center_z = (view.camera[2] / 16.0).floor() as i32;
    urgent.retain(|pos| {
        inside_cache_hysteresis(*pos, view.dimension, center_x, center_z, view.radius)
    });
}

fn take_visible_urgent_chunks(
    urgent: &mut BTreeSet<ChunkPos>,
    dimension: Dimension,
    center_x: i32,
    center_z: i32,
    radius: i32,
    limit: usize,
) -> Vec<ChunkPos> {
    let visible = nearest_view_positions(
        urgent.iter().copied(), dimension, center_x, center_z, radius, limit,
    );
    for pos in &visible {
        urgent.remove(pos);
    }
    visible
}

#[cfg(test)]
mod view_tests {
    use super::{
        ChunkCursor, ChunkServiceEvent, ChunkServiceHandle, cached_texture_source, chunk_mesh_id,
        coarse_lod_for_view, desired_chunk_positions, downsample_texture, face_band_y,
        face_vertex_color, forget_published_sections, grass_block_side_texture_keys,
        inside_cache_hysteresis, inside_view, intern_face_materials,
        is_full_grass_block_side_material, mesh_target_positions, mesh_target_priority,
        mipmapped_texture, p3d,
        partition_section_faces, same_chunk_render_geometry, section_priority,
        surrounded_chunk_candidates, take_visible_dirty_meshes, take_visible_urgent_chunks,
        visible_dirty_meshes,
    };
    use bedrock_block_model::{ObjResolvedTexture, ResourceFile};
    use bedrock_render::{ChunkPos, Dimension};
    use gpui_3d::TextureColorSpace;
    use image::{Rgba, RgbaImage};
    use std::collections::BTreeSet;
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::mpsc::{channel, sync_channel};
    use std::time::Duration;

    #[test]
    fn deferred_neighbor_without_boundary_must_restore_seam_occupancy() {
        use super::needs_mesh_neighbor_hydration as needs;
        // A deferred chunk may be absent from the GPU draw set, but it must
        // still suppress the covered side faces of adjacent loaded chunks.
        assert!(needs(false, false, false));
        assert!(needs(true, false, true));
        assert!(!needs(false, false, true));
        assert!(!needs(false, true, false));
        assert!(!needs(true, true, false));
    }

    #[test]
    fn rear_metadata_batch_omits_unused_biomes_and_block_entity_nbt() {
        let view = ViewState {
            generation: 1,
            dimension: Dimension::Overworld,
            camera: [8.0, 96.0, 8.0],
            forward_xz: [1.0, 0.0],
            forward_y: 0.0,
            radius: 16,
            view_distance: 320.0,
        };
        let rear = [
            ChunkPos { x: -12, z: 0, dimension: view.dimension },
            ChunkPos { x: -13, z: 0, dimension: view.dimension },
        ];
        let foreground = ChunkPos { x: 12, z: 0, dimension: view.dimension };
        let full = super::ChunkLoadOptions {
            data_request: super::ChunkDataRequest::new()
                .full_3d_indices()
                .biome(super::BiomeDataRequirement::All)
                .block_entities(),
            ..Default::default()
        };
        let lean = super::request_for_stream_batch(full.clone(), &rear, Some(view));
        assert_eq!(lean.data_request.subchunks, full.data_request.subchunks);
        assert_eq!(lean.data_request.biome, super::BiomeDataRequirement::None);
        assert!(!lean.data_request.block_entities);
        let mixed = super::request_for_stream_batch(
            full.clone(),
            &[rear[0], foreground],
            Some(view),
        );
        assert_eq!(mixed.data_request, full.data_request);
        assert_eq!(
            super::request_for_stream_batch(full.clone(), &rear, None).data_request,
            full.data_request
        );
        assert_eq!(
            super::request_for_stream_batch(full.clone(), &[], Some(view)).data_request,
            full.data_request
        );
    }

    #[test]
    fn dirty_mesh_priority_uses_near_first_then_forward_within_radial_band() {
        let at = |x, z| ChunkPos { x, z, dimension: Dimension::Overworld };
        let view = super::ViewState {
            generation: 1,
            dimension: Dimension::Overworld,
            camera: [8.0, 96.0, 8.0],
            forward_xz: [1.0, 0.0],
            forward_y: 0.0,
            radius: 16,
            view_distance: 320.0,
        };
        assert!(super::renderable_dirty_priority(at(-3, 0), view)
            < super::renderable_dirty_priority(at(7, 0), view));
        assert!(super::renderable_dirty_priority(at(7, 0), view)
            < super::renderable_dirty_priority(at(0, 7), view));
        // Compare across distance groups: far forward work cannot block
        // much closer peripheral work.
        assert!(super::renderable_dirty_priority(at(0, 5), view)
            < super::renderable_dirty_priority(at(15, 0), view));
        let mut dirty = BTreeSet::from([at(-3, 0), at(0, 7), at(7, 0)]);
        let waves = super::take_renderable_dirty_meshes(&mut dirty, view, 2);
        assert_eq!(waves, [at(-3, 0), at(7, 0)]);
        assert_eq!(dirty, BTreeSet::from([at(0, 7)]));
    }

    #[test]
    fn changing_heading_reorders_only_unconsumed_ring_positions() {
        let mut cursor = super::ChunkCursor::default();
        // First chunk is always the origin. The next call begins ring 1.
        let origin = cursor.next(Dimension::Overworld, 0, 0, 3, [1.0, 0.0]).unwrap();
        assert_eq!((origin.x, origin.z), (0, 0));
        let first = cursor.next(Dimension::Overworld, 0, 0, 3, [1.0, 0.0]).unwrap();
        assert_eq!(first.x, 1);
        cursor.reprioritize_remaining([-1.0, 0.0]);
        let second = cursor.next(Dimension::Overworld, 0, 0, 3, [-1.0, 0.0]).unwrap();
        assert_eq!(second.x, -1);
        assert_ne!(first, second);
        let mut positions = BTreeSet::from([origin, first, second]);
        for _ in 0..6 {
            positions.insert(cursor.next(Dimension::Overworld, 0, 0, 3, [-1.0, 0.0]).unwrap());
        }
        assert_eq!(positions.len(), 9, "heading change cannot duplicate ring positions");
    }

    #[test]
    fn slow_uploads_admit_one_mesh_per_interleaved_wave() {
        assert_eq!(super::interleaved_mesh_wave_size(0, 0, 0), 4);
        assert_eq!(super::interleaved_mesh_wave_size(3, 5, 0), 2);
        assert_eq!(super::interleaved_mesh_wave_size(9, 6, 2), 1);
        assert_eq!(super::interleaved_mesh_wave_size(0, 0, 30), 1);
    }

    #[test]
    fn staged_stream_meshes_sort_front_before_rear_and_keep_every_chunk() {
        let dimension = Dimension::Overworld;
        let mut loaded = vec![
            ChunkPos { x: -2, z: 0, dimension },
            ChunkPos { x: 3, z: 0, dimension },
            ChunkPos { x: -1, z: 0, dimension },
            ChunkPos { x: 1, z: 0, dimension },
            ChunkPos { x: 0, z: 1, dimension },
        ];
        super::prioritize_stream_meshes(&mut loaded, (0, 0), [1.0, 0.0]);
        assert_eq!((loaded[0].x, loaded[0].z), (1, 0));
        assert_eq!((loaded[1].x, loaded[1].z), (3, 0));
        assert_eq!(loaded.len(), 5);
        assert_eq!(loaded.iter().collect::<HashSet<_>>().len(), 5);
        assert!(loaded.chunks(super::STREAM_MESH_BATCH_CHUNKS).all(|chunk| {
            chunk.len() <= super::STREAM_MESH_BATCH_CHUNKS
        }));
    }

    #[test]
    fn boundary_pairs_are_order_independent_and_rearm_after_unload() {
        let chunk = |x| ChunkPos { x, z: 0, dimension: Dimension::Overworld };
        let (left, middle, right) = (chunk(0), chunk(1), chunk(2));
        let mut pairs = BTreeSet::new();
        assert!(super::record_boundary_pair(&mut pairs, left, middle));
        assert!(!super::record_boundary_pair(&mut pairs, middle, left));
        // A different neighbour of the already processed middle column is new.
        assert!(super::record_boundary_pair(&mut pairs, middle, right));
        assert_eq!(pairs.len(), 2);
        super::forget_boundary_pairs(&mut pairs, middle);
        assert!(pairs.is_empty());
        assert!(super::record_boundary_pair(&mut pairs, left, middle));
    }

    #[test]
    fn published_sections_reuse_exact_meshes_and_forget_deleted_sections() {
        use gpui_3d::{Material, Mesh, MeshPart, Vec2, Vec3, Vertex};
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let quad =
            [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 0.0, 1.0), Vec3::Z].map(|position| Vertex {
                position,
                normal: Vec3::Y,
                uv: Vec2::ZERO,
                color: [1.0; 4],
            });
        let material = Arc::new(Material::new());
        let build = |section_y, vertices| {
            Arc::new(super::World3dChunkMesh {
                occluders: Arc::from([]),
                pos,
                section_y,
                origin: [0; 3],
                mesh: Arc::new(
                    Mesh::from_voxel_quads_without_raycast([vertices], [MeshPart::new(0, 6, 0)])
                        .unwrap(),
                ),
                materials: Arc::from([material.clone()]),
                textures: Arc::from([]),
                triangles: 2,
                faces: 1,
                missing_texture_faces: 0,
            })
        };
        let original = build(0, quad);
        let same = build(0, quad);
        let mut changed_quad = quad;
        changed_quad[0].position.y = 1.0;
        let changed = build(0, changed_quad);
        assert!(same_chunk_render_geometry(&original, &same));
        assert!(!same_chunk_render_geometry(&original, &changed));
        let mut changed_material = (*same).clone();
        changed_material.materials = Arc::from([Arc::new(Material::new())]);
        assert!(same_chunk_render_geometry(&original, &changed_material),
            "new MaterialId alone must not invalidate resident GPU geometry");
        let mut truly_changed = Material::new();
        truly_changed.base_color = [0.25, 0.5, 0.75, 1.0];
        changed_material.materials = Arc::from([Arc::new(truly_changed)]);
        assert!(!same_chunk_render_geometry(&original, &changed_material));
        let mut changed_occlusion = (*same).clone();
        changed_occlusion.occluders = Arc::from([gpui_3d::Occluder {
            bounds: gpui_3d::Aabb::new(Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0)).unwrap(),
            faces: 63,
        }]);
        assert!(!same_chunk_render_geometry(&original, &changed_occlusion));
        assert!(super::same_chunk_mesh_contents(&original, &changed_occlusion));
        let mut update = Arc::new(changed_occlusion);
        assert!(super::reuse_section_gpu_mesh(&original, &mut update));
        assert!(Arc::ptr_eq(&update.mesh, &original.mesh));
        assert!(Arc::ptr_eq(&update.materials, &original.materials));
        assert_ne!(update.occluders, original.occluders);
        let other = build(1, quad);
        let mut published = std::collections::BTreeMap::from([
            ((pos, 0), Arc::downgrade(&original)),
            ((pos, 1), Arc::downgrade(&other)),
        ]);
        forget_published_sections(&mut published, pos, &[1]);
        assert!(!published.contains_key(&(pos, 0)));
        assert!(published.contains_key(&(pos, 1)));
    }

    #[test]
    fn neighbor_height_ranges_include_both_sides_of_band_boundaries() {
        assert_eq!(super::neighbor_affected_bands(63, 63), (0, 1));
        assert_eq!(super::neighbor_affected_bands(64, 64), (0, 1));
        assert_eq!(super::neighbor_affected_bands(-64, -64), (-2, -1));
        assert_eq!(super::neighbor_affected_bands(-1, -1), (-1, 0));
        assert_eq!(super::neighbor_affected_bands(0, 0), (-1, 0));
    }

    #[test]
    fn dirty_remesh_does_not_expand_to_unchanged_neighbors() {
        let pos = |x, z| ChunkPos {
            x,
            z,
            dimension: Dimension::Overworld,
        };
        let cached = BTreeSet::from([pos(0, 0), pos(1, 0), pos(0, 1), pos(1, 1)]);
        let changed = [pos(0, 0)];
        assert_eq!(
            mesh_target_positions(&changed, false, |_, p| cached.contains(p)),
            BTreeSet::from(changed)
        );
        assert_eq!(
            mesh_target_positions(&changed, true, |_, p| cached.contains(p)),
            cached
        );
    }

    #[test]
    fn remote_edit_requests_are_evicted_without_losing_nearby_priority() {
        let pos = |x, dimension| ChunkPos { x, z: 0, dimension };
        let nearby = pos(1, Dimension::Overworld);
        let mut urgent = BTreeSet::from([
            nearby,
            pos(2, Dimension::Overworld),
            pos(100, Dimension::Overworld),
            pos(0, Dimension::Nether),
        ]);
        let view = super::ViewState {
            generation: 1,
            dimension: Dimension::Overworld,
            camera: [0.5, 96.0, 0.5],
            forward_xz: [0.0, 1.0],
            forward_y: 0.0,
            radius: 1,
            view_distance: 320.0,
        };
        super::retain_urgent_chunks(&mut urgent, view);
        assert_eq!(
            urgent,
            BTreeSet::from([nearby, pos(2, Dimension::Overworld)])
        );
        assert_eq!(
            super::take_visible_urgent_chunks(&mut urgent, view.dimension, 0, 0, 1, 16),
            [nearby]
        );
    }

    #[test]
    fn mesh_targets_show_new_camera_facing_chunks_before_neighbor_remeshes() {
        let at = |x, z| ChunkPos {
            x,
            z,
            dimension: Dimension::Overworld,
        };
        let loaded = HashSet::from([at(0, 2), at(0, -1)]);
        let mut targets = [at(1, 0), at(0, -1), at(0, 2)];
        targets.sort_by_key(|pos| mesh_target_priority(*pos, &loaded, (0, 0), [0.0, 1.0]));
        assert_eq!(targets, [at(0, 2), at(0, -1), at(1, 0)]);
    }

    #[test]
    fn rear_dirty_rebuilds_wait_for_camera_without_losing_dirty_state() {
        let pos = |z| ChunkPos {
            x: 0, z, dimension: Dimension::Overworld,
        };
        let mut dirty = BTreeSet::from([pos(-8), pos(-1), pos(2)]);
        let mut view = super::ViewState {
            generation: 1,
            dimension: Dimension::Overworld,
            camera: [8.0, 96.0, 8.0],
            forward_xz: [0.0, 1.0],
            forward_y: 0.0,
            radius: 10,
            view_distance: 320.0,
        };
        let visible = super::take_renderable_dirty_meshes(&mut dirty, view, 8);
        assert!(visible.contains(&pos(-1)) && visible.contains(&pos(2)));
        if std::env::var_os("BMCBL_WORLD3D_DISABLE_LAZY_MESH").is_none() {
            assert_eq!(dirty, BTreeSet::from([pos(-8)]));
        }
        view.forward_xz = [0.0, -1.0];
        assert!(super::renderable_dirty_meshes(&dirty, view).contains(&pos(-8))
            || dirty.is_empty());
    }

    #[test]
    fn bounded_nearest_queue_matches_full_sorted_visible_columns() {
        let dimension = Dimension::Overworld;
        let mut positions = BTreeSet::new();
        for x in -28..=28 {
            for z in -28..=28 {
                if (x * 17 + z * 13) % 7 != 0 {
                    positions.insert(ChunkPos { x, z, dimension });
                }
            }
        }
        let sorted = visible_dirty_meshes(&positions, dimension, 2, -3, 20);
        for limit in [0, 1, 3, 8, 32, 256, 2048] {
            let expected = &sorted[..limit.min(sorted.len())];
            let mut dirty = positions.clone();
            let actual = take_visible_dirty_meshes(
                &mut dirty, dimension, 2, -3, 20, limit,
            );
            assert_eq!(actual, expected);
            assert!(actual.iter().all(|p| !dirty.contains(p)));

            let mut urgent = positions.clone();
            let selected = take_visible_urgent_chunks(
                &mut urgent, dimension, 2, -3, 20, limit,
            );
            assert_eq!(selected, expected);
            assert!(selected.iter().all(|p| !urgent.contains(p)));
        }
    }

    #[test]
    fn deferred_mesh_candidates_do_not_enter_the_sorted_rebuild_queue() {
        let pos = |x, z| ChunkPos { x, z, dimension: Dimension::Overworld };
        let view = super::ViewState {
            generation: 1,
            dimension: Dimension::Overworld,
            camera: [8.0, 96.0, 8.0],
            forward_xz: [0.0, 1.0],
            forward_y: 0.0,
            radius: 12,
            view_distance: 320.0,
        };
        let dirty = BTreeSet::from([pos(0, -8), pos(0, -6), pos(0, 3), pos(0, 7)]);
        let candidates = super::renderable_dirty_meshes(&dirty, view);
        assert_eq!(candidates.first(), Some(&pos(0, 3)));
        assert!(candidates.contains(&pos(0, 7)));
        if std::env::var_os("BMCBL_WORLD3D_DISABLE_LAZY_MESH").is_none() {
            assert!(!candidates.contains(&pos(0, -8)));
            assert!(!candidates.contains(&pos(0, -6)));
        }
        // The dirty source set is retained until the camera turns around.
        assert_eq!(dirty.len(), 4);
    }

    #[test]
    fn lazy_mesh_only_defers_distant_rear_columns_and_recovers_on_turn() {
        let pos = |x, z| ChunkPos { x, z, dimension: Dimension::Overworld };
        let ahead = pos(0, 7);
        let rear = pos(0, -7);
        let near = pos(0, -2);
        let mut view = super::ViewState {
            generation: 1,
            dimension: Dimension::Overworld,
            camera: [8.0, 96.0, 8.0],
            forward_xz: [0.0, 1.0],
            forward_y: 0.0,
            radius: 8,
            view_distance: 320.0,
        };
        assert!(!super::lazy_mesh_column(ahead, view));
        assert!(!super::lazy_mesh_column(near, view));
        if std::env::var_os("BMCBL_WORLD3D_DISABLE_LAZY_MESH").is_none() {
            assert!(super::lazy_mesh_column(rear, view));
        }
        view.forward_xz = [0.0, -1.0];
        assert!(!super::lazy_mesh_column(rear, view));
        view.forward_xz = [0.0, 0.0];
        assert!(!super::lazy_mesh_column(rear, view));
        view.forward_xz = [0.0, 1.0];
        assert!(!super::lazy_mesh_column(pos(7, 0), view));
    }

    #[test]
    fn section_priority_favors_visible_height_before_distant_hidden_height() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let view = super::ViewState {
            generation: 1,
            dimension: Dimension::Overworld,
            camera: [8.0, 170.0, 8.0],
            forward_xz: [0.0, 0.0],
            forward_y: -1.0,
            radius: 8,
            view_distance: 320.0,
        };
        assert!(section_priority(pos, 1, view) < section_priority(pos, 4, view));
    }

    #[test]
    fn coarse_lod_starts_outside_the_selected_circular_radius() {
        let mut view = super::ViewState {
            generation: 1,
            dimension: Dimension::Overworld,
            camera: [8.0, 96.0, 8.0],
            forward_xz: [1.0, 0.0],
            forward_y: 0.0,
            radius: 32,
            view_distance: 528.0,
        };
        for radius in [8, 32, 64] {
            view.radius = radius;
            let edge = ChunkPos {
                x: radius,
                z: 0,
                dimension: Dimension::Overworld,
            };
            let outside = ChunkPos {
                x: radius + 1,
                ..edge
            };
            for was_coarse in [false, true] {
                assert!(!coarse_lod_for_view(edge, view, was_coarse));
                assert!(coarse_lod_for_view(outside, view, was_coarse));
                assert!(coarse_lod_for_view(
                    ChunkPos { z: radius, ..edge },
                    view,
                    was_coarse
                ));
            }
            view.camera[0] = 24.0;
            assert!(!coarse_lod_for_view(outside, view, true));
            view.camera[0] = 8.0;
        }
    }

    #[test]
    fn section_partition_preserves_faces_and_prioritizes_camera_height() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let view = super::ViewState {
            generation: 1,
            dimension: Dimension::Overworld,
            camera: [8.0, 170.0, 8.0],
            forward_xz: [0.0, 0.0],
            forward_y: -1.0,
            radius: 8,
            view_distance: 320.0,
        };
        let face_at = |y: f64| p3d::Preview3dStreamFace {
            ambient_light: [1.0; 4],
            corners: [[0.0, y, 0.0]; 4],
            color: [1.0; 4],
            shade: 1.0,
            normal: [0, 1, 0],
            material: Arc::from("test"),
            uv: None,
            pass: p3d::Preview3dStreamPass::Opaque,
        };
        let sections = partition_section_faces(
            pos,
            vec![face_at(280.0), face_at(100.0), face_at(105.0)],
            view,
        );
        assert_eq!(sections.iter().map(|(y, _)| *y).collect::<Vec<_>>(), [1, 4]);
        assert_eq!(
            sections.iter().map(|(_, faces)| faces.len()).sum::<usize>(),
            3
        );
    }

    #[test]
    fn cube_and_detail_faces_do_not_share_sidedness() {
        use p3d::Preview3dStreamPass as Pass;
        assert!(!super::face_double_sided(Pass::Opaque));
        for pass in [
            Pass::DetailOpaque,
            Pass::Glass,
            Pass::DetailGlass,
            Pass::Water,
            Pass::Lava,
        ] {
            assert!(super::face_double_sided(pass));
        }
        let cube = super::MaterialSlotKey {
            water: false,
            glass: false,
            texture: None,
            normal: None,
            mer: None,
            alpha_mode: 0,
            double_sided: false,
            base_color_bits: [0; 3],
        };
        let detail = super::MaterialSlotKey {
            double_sided: true,
            ..cube
        };
        assert!(cube != detail);
        assert_eq!(HashSet::from([cube, detail]).len(), 2);
    }

    #[test]
    fn missing_textures_do_not_make_ordinary_blocks_translucent() {
        use super::TextureAlphaContent as Content;
        use gpui_3d::AlphaMode;
        use p3d::Preview3dStreamPass as Pass;
        for pass in [Pass::Opaque, Pass::DetailOpaque, Pass::Lava] {
            assert_eq!(
                super::alpha_mode_for_pass(pass, Content::Unknown),
                AlphaMode::Opaque
            );
            assert_eq!(
                super::alpha_mode_for_pass(pass, Content::Opaque),
                AlphaMode::Opaque
            );
            assert_eq!(
                super::alpha_mode_for_pass(pass, Content::Binary),
                AlphaMode::Mask
            );
            // Authored partial coverage (portal, slime) must remain blended.
            assert_eq!(
                super::alpha_mode_for_pass(pass, Content::Partial),
                AlphaMode::Blend
            );
        }
        for pass in [Pass::Glass, Pass::DetailGlass, Pass::Water] {
            assert_eq!(
                super::alpha_mode_for_pass(pass, Content::Unknown),
                AlphaMode::Blend
            );
        }
    }

    #[test]
    fn resolved_texture_path_is_reused_after_asset_eviction() {
        let mut cache = HashMap::new();
        let key = ("minecraft_stone".to_owned(), [0, 1, 0]);
        let mut resolutions = 0;
        let resolve = || {
            resolutions += 1;
            Some(ObjResolvedTexture {
                source_path: ResourceFile::file(PathBuf::from("stone.png")),
                relative_path: "stone.png".to_owned(),
                pack_root: PathBuf::new(),
            })
        };
        let first = cached_texture_source(&mut cache, &key, resolve).unwrap();
        let second = cached_texture_source(&mut cache, &key, || {
            resolutions += 1;
            None
        })
        .unwrap();
        assert_eq!(resolutions, 1);
        assert_eq!(first.relative_path, second.relative_path);
    }

    #[test]
    fn shutdown_releases_a_worker_blocked_by_pending_mesh_events() {
        let (commands, _command_rx) = channel();
        let (events, event_rx) = sync_channel(1);
        events
            .send(ChunkServiceEvent::Fatal {
                error: String::new(),
            })
            .unwrap();
        let (finished_tx, finished_rx) = channel();
        let worker = std::thread::spawn(move || {
            let result = events.send(ChunkServiceEvent::Fatal {
                error: String::new(),
            });
            let _ = finished_tx.send(result.is_err());
        });
        let mut handle = ChunkServiceHandle {
            commands,
            pending_views: Arc::new(Mutex::new(super::PendingViews::default())),
            events: Some(event_rx),
            thread: None,
        };
        handle.shutdown();
        assert_eq!(finished_rx.recv_timeout(Duration::from_secs(1)), Ok(true));
        worker.join().unwrap();
    }

    #[test]
    fn rapid_camera_updates_keep_only_the_latest_pending_view() {
        let (commands, command_rx) = channel();
        let handle = ChunkServiceHandle {
            commands,
            pending_views: Arc::new(Mutex::new(super::PendingViews::default())),
            events: None,
            thread: None,
        };
        for generation in 1..=100 {
            assert!(handle.send(super::ChunkServiceCommand::SetView {
                generation,
                dimension: Dimension::Overworld,
                camera: [generation as f64, 96.0, 0.0],
                forward_xz: [0.0, -1.0],
                forward_y: 0.0,
                radius: 32,
                view_distance: 528.0,
            }));
        }
        assert!(matches!(
            command_rx.try_recv(),
            Ok(super::ChunkServiceCommand::ViewUpdated)
        ));
        assert!(command_rx.try_recv().is_err());
        let latest = handle.pending_views.lock().unwrap().views.pop_front();
        assert!(matches!(
            latest,
            Some(super::ChunkServiceCommand::SetView {
                generation: 100,
                camera: [100.0, 96.0, 0.0],
                ..
            })
        ));
    }

    #[test]
    fn edit_invalidation_keeps_views_on_both_sides_in_order() {
        let (commands, command_rx) = channel();
        let handle = ChunkServiceHandle {
            commands,
            pending_views: Arc::new(Mutex::new(super::PendingViews::default())),
            events: None,
            thread: None,
        };
        for generation in 1..=2 {
            if generation == 2 {
                assert!(handle.send(super::ChunkServiceCommand::Invalidate {
                    generation: 1,
                    chunks: Vec::new(),
                }));
            }
            assert!(handle.send(super::ChunkServiceCommand::SetView {
                generation,
                dimension: Dimension::Overworld,
                camera: [0.0, 96.0, 0.0],
                forward_xz: [0.0, -1.0],
                forward_y: 0.0,
                radius: 8,
                view_distance: 320.0,
            }));
        }
        assert!(matches!(
            command_rx.try_recv(),
            Ok(super::ChunkServiceCommand::ViewUpdated)
        ));
        assert!(matches!(
            command_rx.try_recv(),
            Ok(super::ChunkServiceCommand::Invalidate { generation: 1, .. })
        ));
        assert!(matches!(
            command_rx.try_recv(),
            Ok(super::ChunkServiceCommand::ViewUpdated)
        ));
        let mut pending = handle.pending_views.lock().unwrap();
        for generation in 1..=2 {
            assert!(
                matches!(pending.views.pop_front(), Some(super::ChunkServiceCommand::SetView { generation: view_generation, .. }) if view_generation == generation)
            );
        }
        assert!(pending.views.is_empty());
    }

    #[test]
    fn evicted_neighbor_is_remeshed_when_it_enters_view() {
        let pos = ChunkPos {
            x: 10,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let mut dirty = BTreeSet::from([pos]);
        assert!(take_visible_dirty_meshes(&mut dirty, Dimension::Overworld, 0, 0, 8, 8).is_empty());
        assert!(dirty.contains(&pos));
        assert_eq!(
            take_visible_dirty_meshes(&mut dirty, Dimension::Overworld, 2, 0, 8, 8),
            vec![pos]
        );
        assert!(dirty.is_empty());
    }

    #[test]
    fn dirty_remesh_batch_keeps_farther_chunks_pending() {
        let at = |x| ChunkPos {
            x,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let mut dirty = BTreeSet::from([at(-7), at(1), at(3)]);
        assert_eq!(
            take_visible_dirty_meshes(&mut dirty, Dimension::Overworld, 0, 0, 8, 2),
            vec![at(1), at(3)]
        );
        assert_eq!(dirty, BTreeSet::from([at(-7)]));
    }

    #[test]
    fn dirty_order_snapshot_preserves_unprocessed_and_offscreen_work() {
        let at = |x| ChunkPos {
            x,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let mut dirty = BTreeSet::from([at(-7), at(1), at(3), at(20)]);
        let ordered = visible_dirty_meshes(&dirty, Dimension::Overworld, 0, 0, 8);
        assert_eq!(ordered, [at(1), at(3), at(-7)]);
        assert_eq!(dirty.len(), 4);
        for pos in &ordered[..2] {
            dirty.remove(pos);
        }
        assert_eq!(dirty, BTreeSet::from([at(-7), at(20)]));
    }

    #[test]
    #[ignore = "CPU dirty-queue timing; does not open a world or window"]
    fn profile_dirty_batch_order() {
        use std::time::Instant;
        let original = (-32..=32)
            .flat_map(|x| {
                (-32..=32).map(move |z| ChunkPos {
                    x,
                    z,
                    dimension: Dimension::Overworld,
                })
            })
            .collect::<BTreeSet<_>>();
        let mut repeated = original.clone();
        let start = Instant::now();
        let mut previous = Vec::new();
        while !repeated.is_empty() {
            previous.extend(take_visible_dirty_meshes(
                &mut repeated,
                Dimension::Overworld,
                0,
                0,
                32,
                8,
            ));
        }
        let repeated_time = start.elapsed();
        let mut pending = original.clone();
        let start = Instant::now();
        let ordered = visible_dirty_meshes(&pending, Dimension::Overworld, 0, 0, 32);
        for batch in ordered.chunks(8) {
            for pos in batch {
                pending.remove(pos);
            }
        }
        assert!(pending.is_empty());
        assert_eq!(ordered, previous);
        eprintln!(
            "dirty CPU chunks={} repeated_batch_sort_ms={:.2} single_sort_ms={:.2}",
            original.len(),
            repeated_time.as_secs_f64() * 1000.0,
            start.elapsed().as_secs_f64() * 1000.0
        );
    }

    #[test]
    fn edited_chunks_are_loaded_nearest_first_without_dropping_offscreen_edits() {
        let at = |x| ChunkPos {
            x,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let mut urgent = BTreeSet::from([at(6), at(2), at(1)]);
        assert_eq!(
            take_visible_urgent_chunks(&mut urgent, Dimension::Overworld, 0, 0, 4, 1),
            vec![at(1)]
        );
        assert_eq!(
            take_visible_urgent_chunks(&mut urgent, Dimension::Overworld, 0, 0, 4, 1),
            vec![at(2)]
        );
        assert!(urgent.contains(&at(6)));
    }

    #[test]
    fn view_loads_the_full_eight_chunk_radius() {
        let positions = desired_chunk_positions(Dimension::Overworld, 10, -10, 8);
        let contains = |dx, dz| {
            positions.contains(&ChunkPos {
                x: 10 + dx,
                z: -10 + dz,
                dimension: Dimension::Overworld,
            })
        };
        assert!(contains(0, 8));
        assert!(contains(0, -4));
        assert!(contains(0, -8));
        assert!(!contains(-8, -8));
        assert_eq!(positions.len(), 197);
        assert!(!contains(9, 0));
        assert!(contains(0, 0));
    }

    #[test]
    fn cache_margin_keeps_a_chunk_without_extending_the_render_radius() {
        let pos = ChunkPos {
            x: 9,
            z: 0,
            dimension: Dimension::Overworld,
        };
        assert!(inside_cache_hysteresis(pos, Dimension::Overworld, 0, 0, 8,));
        assert!(!inside_view(pos, Dimension::Overworld, 0, 0, 8));
    }

    #[test]
    fn small_radius_keeps_one_column_margin_without_doubling_cache_area() {
        let pos = ChunkPos {
            x: 6,
            z: 0,
            dimension: Dimension::Overworld,
        };
        assert!(!inside_cache_hysteresis(pos, Dimension::Overworld, 0, 0, 4));
        assert!(inside_cache_hysteresis(
            ChunkPos { x: 5, ..pos },
            Dimension::Overworld,
            0,
            0,
            4,
        ));
    }

    #[test]
    fn chunk_cursor_covers_each_circle_once_and_supports_256() {
        for radius in [4, 8, 12, 16, 24, 32, 256] {
            let mut cursor = ChunkCursor::default();
            let mut positions = std::collections::BTreeSet::new();
            while let Some(pos) = cursor.next(Dimension::Overworld, 10, -10, radius, [0.0, -1.0]) {
                assert!(positions.insert(pos), "duplicate chunk at radius {radius}");
                assert!(inside_view(pos, Dimension::Overworld, 10, -10, radius));
            }
            assert_eq!(
                positions.len(),
                super::super::chunk_range::column_count(radius)
            );
            assert_eq!(
                positions,
                desired_chunk_positions(Dimension::Overworld, 10, -10, radius)
            );
        }
    }

    #[test]
    fn chunk_cursor_loads_each_square_ring_before_the_next() {
        let mut cursor = ChunkCursor::default();
        let positions =
            std::iter::from_fn(|| cursor.next(Dimension::Overworld, 0, 0, 8, [0.0, -1.0]))
                .collect::<Vec<_>>();
        assert_eq!(positions[0].x, 0);
        assert_eq!(positions[0].z, 0);
        assert!(
            positions[..9]
                .iter()
                .all(|pos| pos.x.abs().max(pos.z.abs()) <= 1)
        );
        assert_eq!(positions.len(), 197);
    }

    #[test]
    fn chunk_cursor_prioritizes_camera_facing_side_within_a_ring() {
        let mut cursor = ChunkCursor::default();
        let positions =
            std::iter::from_fn(|| cursor.next(Dimension::Overworld, 0, 0, 1, [0.0, -1.0]))
                .collect::<Vec<_>>();
        assert_eq!(positions.len(), 5);
        assert_eq!(positions[0].x, 0);
        assert_eq!(positions[0].z, 0);
        assert_eq!(positions[1].z, -1);
        assert_eq!(positions[4].z, 1);
    }

    #[test]
    fn newly_loaded_neighbor_releases_an_older_interior_chunk() {
        let mut cached = desired_chunk_positions(Dimension::Overworld, 0, 0, 2);
        let last_loaded = ChunkPos {
            x: 2,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let center = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        cached.remove(&last_loaded);
        assert!(
            surrounded_chunk_candidates(&[last_loaded], |pos| cached.contains(&pos)).is_empty()
        );
        cached.insert(last_loaded);
        let eligible = surrounded_chunk_candidates(&[last_loaded], |pos| cached.contains(&pos));
        assert!(eligible.contains(&center));
        assert!(!eligible.contains(&ChunkPos { x: 1, ..center }));
    }

    #[test]
    fn block_texture_has_levels_for_distant_sampling() {
        let image = RgbaImage::from_fn(16, 16, |x, y| {
            if (x + y) % 2 == 0 {
                Rgba([255, 255, 255, 255])
            } else {
                Rgba([0, 0, 0, 255])
            }
        });
        let texture = mipmapped_texture(image).expect("valid mip chain");
        assert_eq!(texture.mip_level_count(), 5);
        assert_eq!(texture.mip_extent(4), Some((1, 1)));
        assert_eq!(texture.color_space(), TextureColorSpace::Linear);
        assert_eq!(texture.mip_pixels(4).map(|pixels| pixels[0]), Some(188));
    }

    #[test]
    fn transparent_mips_keep_opaque_edge_colors() {
        let image = RgbaImage::from_fn(2, 2, |x, _| {
            if x == 0 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 0, 0])
            }
        });
        assert_eq!(
            downsample_texture(&image).get_pixel(0, 0).0,
            [255, 0, 0, 128]
        );
    }

    #[test]
    fn cached_chunk_survives_turn_and_two_chunks_of_travel() {
        let pos = ChunkPos {
            x: -10,
            z: 0,
            dimension: Dimension::Overworld,
        };
        assert!(inside_cache_hysteresis(pos, Dimension::Overworld, 0, 0, 8,));
        assert!(!inside_cache_hysteresis(pos, Dimension::Overworld, 1, 0, 8,));
    }

    #[test]
    fn section_assignment_uses_world_height_and_unique_mesh_ids() {
        let face = p3d::Preview3dStreamFace {
            ambient_light: [1.0; 4],
            corners: [
                [0.0, -1.0, 0.0],
                [1.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 0.0],
            ],
            color: [1.0; 4],
            shade: 1.0,
            normal: [0, 0, 1],
            material: Arc::from("test"),
            uv: None,
            pass: p3d::Preview3dStreamPass::Opaque,
        };
        assert_eq!(face_band_y(&face), -1);
        let mut face = face;
        for corner in &mut face.corners {
            corner[1] = 63.0;
        }
        assert_eq!(face_band_y(&face), 0);
        for corner in &mut face.corners {
            corner[1] = 64.0;
        }
        assert_eq!(face_band_y(&face), 1);
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        assert_ne!(
            chunk_mesh_id(pos, 0),
            chunk_mesh_id(ChunkPos { x: 1, ..pos }, 0)
        );
        assert_ne!(chunk_mesh_id(pos, 0), chunk_mesh_id(pos, 1));
    }

    #[test]
    fn face_material_names_share_one_allocation_per_chunk() {
        let face = p3d::Preview3dStreamFace {
            ambient_light: [1.0; 4],
            corners: [[0.0; 3]; 4],
            color: [1.0; 4],
            shade: 1.0,
            normal: [0, 1, 0],
            material: Arc::from("minecraft:stone"),
            uv: None,
            pass: p3d::Preview3dStreamPass::Opaque,
        };
        let mut faces = [face.clone(), face];
        faces[1].material = Arc::from("minecraft:stone");
        assert!(!Arc::ptr_eq(&faces[0].material, &faces[1].material));
        intern_face_materials(&mut faces);
        assert!(Arc::ptr_eq(&faces[0].material, &faces[1].material));
    }

    #[test]
    fn source_batch_deduplicates_equal_names_with_distinct_arcs_and_keeps_face_directions() {
        let face = p3d::Preview3dStreamFace {
            ambient_light: [1.0; 4],
            corners: [[0.0; 3]; 4],
            color: [1.0; 4],
            shade: 1.0,
            normal: [0, 1, 0],
            material: Arc::from("minecraft_stone"),
            uv: None,
            pass: p3d::Preview3dStreamPass::Opaque,
        };
        let mut other = face.clone();
        other.material = Arc::from("minecraft_stone");
        assert!(!Arc::ptr_eq(&face.material, &other.material));
        let mut side = other.clone();
        side.normal = [1, 0, 0];
        let mut cached = face.clone();
        cached.material = Arc::from("minecraft_cached");
        let cache =
            std::collections::HashMap::from([((cached.material.to_string(), cached.normal), None)]);
        let actual =
            super::unresolved_material_sources(&[face.clone(), face, other, side, cached], &cache);
        assert_eq!(
            actual,
            vec![
                ("minecraft_stone".to_owned(), [0, 1, 0]),
                ("minecraft_stone".to_owned(), [1, 0, 0])
            ]
        );
    }

    #[test]
    fn pre_tinted_face_keeps_vertex_color_neutral() {
        let face = p3d::Preview3dStreamFace {
            ambient_light: [1.0; 4],
            corners: [[0.0; 3]; 4],
            color: [0.5, 0.7, 0.3, 1.0],
            shade: 1.0,
            normal: [0, 0, 1],
            material: Arc::from("minecraft:grass"),
            uv: None,
            pass: p3d::Preview3dStreamPass::Opaque,
        };
        assert_eq!(
            face_vertex_color(
                &face,
                true,
                false,
                false,
                super::TextureAlphaContent::Opaque
            ),
            [1.0; 4]
        );
        assert_eq!(
            face_vertex_color(&face, true, true, false, super::TextureAlphaContent::Opaque),
            face.color
        );
    }

    #[test]
    fn plant_materials_keep_vertex_tint_and_only_grass_block_sides_pre_tint() {
        assert!(is_full_grass_block_side_material("minecraft_grass_side"));
        assert!(is_full_grass_block_side_material(
            "minecraft_grass_block_side"
        ));
        assert!(!is_full_grass_block_side_material(
            "minecraft_tallgrass__mat_down"
        ));
        assert!(!is_full_grass_block_side_material(
            "minecraft_tall_grass__mat_up"
        ));
        assert!(!is_full_grass_block_side_material(
            "minecraft_seagrass__mat_down"
        ));
        assert_eq!(
            grass_block_side_texture_keys("minecraft_grass_side"),
            Some(("dirt", "grass_side"))
        );
        assert_eq!(
            grass_block_side_texture_keys("minecraft_grass_block_side"),
            Some(("dirt", "grass_side"))
        );
        assert_eq!(
            grass_block_side_texture_keys("minecraft_tallgrass__mat_down"),
            None
        );
        assert_eq!(
            grass_block_side_texture_keys("minecraft_grass_side_snowed"),
            None
        );
    }
}

#[cfg(test)]
mod cache_tests {
    use super::{
        ChunkServiceConfig, ChunkServiceEvent, MaterialSlotKey, ViewState, Worker,
        desired_chunk_positions,
    };
    use bedrock_render::{ChunkPos, Dimension};
    use bedrock_world::world::{OpenOptions, World};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::mpsc::sync_channel;

    #[test]
    fn discarding_full_blocks_keeps_the_existing_neighbor_halo_arc() {
        let pos = ChunkPos { x: 0, z: 0, dimension: Dimension::Overworld };
        let mut blocks = super::p3d::Preview3dChunkBlocks::test_cubes(&[[15, 64, 15]]);
        let boundary = std::sync::Arc::new(blocks.boundary_snapshot());
        let mut cached = super::CachedChunk {
            bytes: blocks.estimated_cpu_bytes(),
            blocks: Some(blocks),
            boundary: Some(boundary.clone()),
            frame_displays: Vec::new(),
            probe: std::sync::Arc::new(super::World3dChunkProbe {
                pos,
                min_y: 0,
                subchunks: Default::default(),
                state_version: None,
                storage_bytes: 0,
            }),
            version: bedrock_world::ChunkVersion::New,
            state_version: None,
            faces: 1,
        };
        let extra_bytes = boundary.estimated_cpu_bytes();
        assert!(cached.estimated_cpu_bytes() >= cached.bytes + extra_bytes);
        cached.discard_blocks();
        assert!(cached.blocks.is_none());
        assert!(std::sync::Arc::ptr_eq(cached.boundary.as_ref().unwrap(), &boundary));
        assert_eq!(cached.bytes, extra_bytes);
    }

    #[test]
    fn discarded_render_blocks_do_not_hide_resident_probe_bytes() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let blocks = super::p3d::Preview3dChunkBlocks::test_cubes(&[[0, 1, 0]]);
        let mut chunk = super::CachedChunk {
            bytes: blocks.estimated_cpu_bytes(),
            blocks: Some(blocks),
            boundary: None,
            frame_displays: Vec::new(),
            probe: std::sync::Arc::new(super::World3dChunkProbe {
                pos,
                min_y: 0,
                subchunks: Default::default(),
                state_version: None,
                storage_bytes: 8192,
            }),
            version: bedrock_world::ChunkVersion::New,
            state_version: None,
            faces: 0,
        };
        let metadata_bytes = chunk.estimated_cpu_bytes() - chunk.bytes;
        chunk.discard_blocks();
        assert!(chunk.blocks.is_none());
        assert_eq!(chunk.estimated_cpu_bytes() - chunk.bytes, metadata_bytes);
        assert!(metadata_bytes >= 8192);
    }

    #[test]
    #[ignore = "requires BMCBL_WORLD3D_TEST_WORLD and BMCBL_WORLD3D_TEST_VERSION"]
    fn neighbor_load_reuses_unaffected_height_band() {
        let world_path = PathBuf::from(std::env::var_os("BMCBL_WORLD3D_TEST_WORLD").unwrap());
        let version_path = PathBuf::from(std::env::var_os("BMCBL_WORLD3D_TEST_VERSION").unwrap());
        let world = World::open(
            &world_path,
            OpenOptions {
                read_only: true,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let config = ChunkServiceConfig {
            world_path,
            world_name: "band-test".to_string(),
            dimension: Dimension::Overworld,
            package_roots: vec![version_path],
            view_radius: 2,
        };
        let mut worker = Worker::new(&config, world);
        worker.view = ViewState {
            generation: 1,
            dimension: Dimension::Overworld,
            camera: [-216.0, 96.0, 344.0],
            forward_xz: [0.0, -1.0],
            forward_y: 0.0,
            radius: 2,
            view_distance: 64.0,
        };
        let center = ChunkPos {
            x: -14,
            z: 21,
            dimension: Dimension::Overworld,
        };
        let east = ChunkPos {
            x: center.x + 1,
            ..center
        };
        let mut columns = worker
            .load_batch(&[center, east])
            .chunks
            .into_iter()
            .map(|(pos, chunk)| (pos, chunk.unwrap().unwrap()))
            .collect::<HashMap<_, _>>();
        let mut target = columns.remove(&center).unwrap();
        target.blocks = Some(super::p3d::Preview3dChunkBlocks::test_cubes(&[
            [center.x * 16 + 15, 8, center.z * 16 + 8],
            [center.x * 16 + 15, 136, center.z * 16 + 8],
        ]));
        target.frame_displays.clear();
        worker.cache.insert(center, target);
        let (events, rx) = sync_channel(4096);
        worker.mesh_targets(&events, &[center], false);
        let mut retained = rx
            .try_iter()
            .filter_map(|event| match event {
                ChunkServiceEvent::Chunk(payload) => Some(payload.mesh),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            retained
                .iter()
                .any(|mesh| mesh.pos == center && mesh.section_y == 0)
        );
        assert!(
            retained
                .iter()
                .any(|mesh| mesh.pos == center && mesh.section_y == 2)
        );
        let original_low = retained
            .iter()
            .find(|mesh| mesh.pos == center && mesh.section_y == 0)
            .unwrap()
            .clone();
        let mut new_neighbor = columns.remove(&east).unwrap();
        new_neighbor.blocks = Some(super::p3d::Preview3dChunkBlocks::test_cubes(&[[
            east.x * 16,
            136,
            east.z * 16 + 8,
        ]]));
        new_neighbor.frame_displays.clear();
        worker.cache.insert(east, new_neighbor);
        worker.mesh_targets(&events, &[east], true);
        let mut target_sections = Vec::new();
        let mut target_retained = Vec::new();
        for event in rx.try_iter() {
            match event {
                ChunkServiceEvent::Chunk(payload) => {
                    assert!(!(payload.mesh.pos == center && payload.mesh.section_y == 0));
                    retained.push(payload.mesh);
                }
                ChunkServiceEvent::Completed(completions) => {
                    for completion in completions {
                        if completion.pos == center {
                            target_sections = completion.sections;
                            target_retained = completion.retained_meshes;
                        }
                    }
                }
                _ => {}
            }
        }
        assert!(target_sections.contains(&0) && target_sections.contains(&2));
        assert_eq!(
            worker.cache[&center].faces,
            worker
                .published_sections
                .range((center, i32::MIN)..=(center, i32::MAX))
                .filter_map(|(_, mesh)| mesh.upgrade())
                .map(|mesh| mesh.faces)
                .sum::<usize>(),
            "the cached face count must include retained bands"
        );
        assert!(
            target_retained
                .iter()
                .any(|mesh| std::sync::Arc::ptr_eq(mesh, &original_low))
        );
        assert!(std::sync::Arc::ptr_eq(
            &worker.published_sections[&(center, 0)].upgrade().unwrap(),
            &original_low
        ));
        worker.mesh_targets(&events, &[center], false);
        assert!(rx.try_iter().all(|event| !matches!(event, ChunkServiceEvent::Chunk(payload) if payload.mesh.pos == center)), "incremental and full rebuild must match");
        // Removing an old boundary block must not use the new empty height range.
        worker.invalidate(&[east]);
        let mut removed = worker
            .load_batch(&[east])
            .chunks
            .pop()
            .unwrap()
            .1
            .unwrap()
            .unwrap();
        removed.blocks = Some(super::p3d::Preview3dChunkBlocks::test_cubes(&[]));
        removed.frame_displays.clear();
        worker.cache.insert(east, removed);
        worker.mesh_targets(&events, &[east], true);
        assert!(rx.try_iter().any(|event| matches!(event, ChunkServiceEvent::Chunk(payload)
            if payload.mesh.pos == center && payload.mesh.section_y == 2 && payload.mesh.faces == 6)),
            "removing a boundary cube must restore the neighboring face");
        drop(retained);
    }

    #[test]
    #[ignore = "requires BMCBL_WORLD3D_TEST_WORLD and BMCBL_WORLD3D_TEST_VERSION"]
    fn interior_chunk_releases_decoded_blocks_and_reloads_for_remesh() {
        let world_path = PathBuf::from(std::env::var_os("BMCBL_WORLD3D_TEST_WORLD").unwrap());
        let version_path = PathBuf::from(std::env::var_os("BMCBL_WORLD3D_TEST_VERSION").unwrap());
        let world = World::open(
            &world_path,
            OpenOptions {
                read_only: true,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let config = ChunkServiceConfig {
            world_path,
            world_name: "cache-test".to_string(),
            dimension: Dimension::Overworld,
            package_roots: vec![version_path],
            view_radius: 2,
        };
        let mut worker = Worker::new(&config, world);
        worker.view = ViewState {
            generation: 1,
            dimension: Dimension::Overworld,
            camera: [-216.0, 96.0, 344.0],
            forward_xz: [0.0, -1.0],
            forward_y: 0.0,
            radius: 2,
            view_distance: 64.0,
        };
        let center = ChunkPos {
            x: -14,
            z: 21,
            dimension: Dimension::Overworld,
        };
        let positions = desired_chunk_positions(Dimension::Overworld, center.x, center.z, 2)
            .into_iter()
            .collect::<Vec<_>>();
        for (pos, chunk) in worker.load_batch(&positions).chunks {
            worker.cache.insert(pos, chunk.unwrap().unwrap());
        }
        let (events, rx) = sync_channel(4096);
        worker.mesh_targets(&events, &positions, true);
        assert!(worker.cache.get(&center).unwrap().blocks.is_none());
        let mut material_ids = HashMap::new();
        let mut reused = 0;
        let mut retained_meshes = Vec::new();
        for event in rx.try_iter() {
            let ChunkServiceEvent::Chunk(payload) = event else {
                continue;
            };
            for material in payload.mesh.materials.iter() {
                let key = MaterialSlotKey {
                    water: material.surface == gpui_3d::SurfaceKind::Water,
                    glass: material.surface == gpui_3d::SurfaceKind::Glass,
                    texture: material.albedo_texture,
                    normal: material.normal_texture,
                    mer: material.metallic_roughness_texture,
                    alpha_mode: material.alpha_mode as u8,
                    double_sided: material.double_sided,
                    base_color_bits: [
                        material.base_color[0],
                        material.base_color[1],
                        material.base_color[2],
                    ]
                    .map(f32::to_bits),
                };
                if let Some(previous) = material_ids.insert(key, material.id()) {
                    assert_eq!(previous, material.id());
                    reused += 1;
                }
            }
            retained_meshes.push(payload.mesh);
        }
        assert!(reused > 0);
        worker.mesh_targets(&events, &[center], false);
        assert!(worker.cache.get(&center).unwrap().blocks.is_none());
        assert!(
            worker.last_reused_sections > 0,
            "unchanged sections should keep their published mesh"
        );
        assert!(
            rx.try_iter()
                .all(|event| !matches!(event, ChunkServiceEvent::Chunk(_)))
        );
        let east = ChunkPos {
            x: center.x + 1,
            ..center
        };
        // Both target columns have discarded their block records, so this remesh
        // exercises one batch rehydration while retaining neighboring summaries.
        worker.cache.get_mut(&east).unwrap().discard_blocks();
        assert!(worker.cache.get(&east).unwrap().blocks.is_none());
        let west = ChunkPos {
            x: center.x - 1,
            ..center
        };
        worker.cache.get_mut(&west).unwrap().discard_blocks();
        worker.mesh_targets(&events, &[center, east], false);
        assert!(
            worker.last_boundary_hits > 0,
            "an input-only neighbor should use its compact summary"
        );
        assert!(worker.last_reused_sections > 0);
        assert!(
            rx.try_iter()
                .all(|event| !matches!(event, ChunkServiceEvent::Chunk(_)))
        );
        // A known-missing position is a negative cache hit, but an explicit edit
        // invalidation must allow that same coordinate to be read again.
        worker.cache.remove(&center);
        worker.missing.insert(center);
        assert!(worker.ensure_cached(center).unwrap().is_none());
        worker.invalidate(&[center]);
        assert!(worker.ensure_cached(center).unwrap().is_some());
        worker.cache.remove(&center);
        worker.failed_positions.insert(center);
        worker.urgent_chunks.insert(center);
        assert!(!worker.next_batch(center.x, center.z).contains(&center));
        worker.invalidate(&[center]);
        assert!(worker.next_batch(center.x, center.z).contains(&center));
        drop(retained_meshes);
    }
}

struct CachedChunk {
    blocks: Option<p3d::Preview3dChunkBlocks>,
    boundary: Option<Arc<p3d::Preview3dChunkBoundary>>,
    frame_displays: Vec<FrameDisplay>,
    probe: Arc<World3dChunkProbe>,
    bytes: usize,
    version: ChunkVersion,
    state_version: Option<i32>,
    faces: usize,
}

impl CachedChunk {
    fn estimated_cpu_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.bytes)
            .saturating_add(
                if self.blocks.is_some() {
                    self.boundary.as_ref().map_or(0, |halo| halo.estimated_cpu_bytes())
                } else {
                    0
                },
            )
            .saturating_add(self.probe.storage_bytes)
            .saturating_add(self.frame_displays.capacity() * std::mem::size_of::<FrameDisplay>())
            .saturating_add(
                self.frame_displays
                    .iter()
                    .map(FrameDisplay::estimated_heap_bytes)
                    .sum::<usize>(),
            )
    }

    fn discard_blocks(&mut self) {
        if let Some(mut blocks) = self.blocks.take() {
            // Neighbor-only meshing may have already extracted the same
            // immutable occupancy halo. Keep its Arc rather than decoding
            // all block classes and rebuilding boundary maps a second time.
            let boundary = self.boundary.take()
                .unwrap_or_else(|| Arc::new(blocks.boundary_snapshot()));
            self.bytes = boundary.estimated_cpu_bytes();
            self.boundary = Some(boundary);
        }
    }
}

#[derive(Clone, Copy)]
struct MaterialSlotKey {
    water: bool,
    glass: bool,
    texture: Option<gpui_3d::TextureAssetId>,
    normal: Option<gpui_3d::TextureAssetId>,
    mer: Option<gpui_3d::TextureAssetId>,
    alpha_mode: u8,
    double_sided: bool,
    base_color_bits: [u32; 3],
}

impl std::hash::Hash for MaterialSlotKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.water.hash(state);
        self.glass.hash(state);
        self.texture.hash(state);
        self.normal.hash(state);
        self.mer.hash(state);
        self.alpha_mode.hash(state);
        self.double_sided.hash(state);
        for bits in self.base_color_bits {
            bits.hash(state);
        }
    }
}

impl PartialEq for MaterialSlotKey {
    fn eq(&self, other: &Self) -> bool {
        self.water == other.water
            && self.glass == other.glass
            && self.texture == other.texture
            && self.normal == other.normal
            && self.mer == other.mer
            && self.alpha_mode == other.alpha_mode
            && self.double_sided == other.double_sided
            && self.base_color_bits == other.base_color_bits
    }
}

impl Eq for MaterialSlotKey {}

struct FaceBucket {
    faces: Vec<(usize, [f32; 4])>,
}

impl FaceBucket {
    fn new() -> Self {
        Self { faces: Vec::new() }
    }
}

pub(super) fn trace_timings() -> bool {
    std::env::var_os("BMCBL_WORLD3D_TRACE_TIMINGS").is_some()
}

fn trace_verbose() -> bool {
    std::env::var_os("BMCBL_WORLD3D_TRACE_VERBOSE").is_some()
}

fn unresolved_material_sources(
    faces: &[p3d::Preview3dStreamFace],
    cache: &HashMap<(String, [i32; 3]), Option<ObjResolvedTexture>>,
) -> Vec<(String, [i32; 3])> {
    let mut seen = HashSet::new();
    let mut seen_names = HashSet::new();
    let mut unresolved = Vec::new();
    for face in faces {
        // Interned faces share an Arc: keep their cheap per-face pointer check.
        if !seen.insert((face.material.as_ptr() as usize, face.normal)) {
            continue;
        }
        let material = face.material.as_ref();
        if material.starts_with(MAP_MATERIAL_PREFIX) || !seen_names.insert((material, face.normal))
        {
            continue;
        }
        let key = (material.to_owned(), face.normal);
        if !cache.contains_key(&key) {
            unresolved.push(key);
        }
    }
    unresolved
}

fn cached_texture_source(
    cache: &mut HashMap<(String, [i32; 3]), Option<ObjResolvedTexture>>,
    key: &(String, [i32; 3]),
    resolve: impl FnOnce() -> Option<ObjResolvedTexture>,
) -> Option<ObjResolvedTexture> {
    cache.entry(key.clone()).or_insert_with(resolve).clone()
}

fn resolve_material_source(
    resolver: &ObjTextureResolver,
    material: &str,
    normal: [i32; 3],
) -> Option<ObjResolvedTexture> {
    let started = trace_timings().then(Instant::now);
    let resolved = if let Some(item) = material.strip_prefix(ITEM_MATERIAL_PREFIX) {
        resolver.item_texture_for(item).or_else(|| {
            let block = item.strip_prefix("minecraft:").unwrap_or(item);
            resolver.texture_for(&format!("minecraft_{block}_side"), normal)
        })
    } else {
        resolver.texture_for(material, normal)
    };
    if let Some(started) = started {
        let elapsed = started.elapsed();
        if elapsed >= Duration::from_millis(50) {
            eprintln!(
                "world3d material_source material={material} normal={normal:?} ms={}",
                elapsed.as_millis(),
            );
        }
    }
    resolved
}

pub(super) fn trace_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

struct LoadedChunkBatch {
    chunks: Vec<(ChunkPos, Result<Option<CachedChunk>, String>)>,
    stats: ChunkLoadStats,
    query_time: Duration,
    collect_time: Duration,
}

/// Metadata-only rear batches need a full indexed Probe for eventual portal
/// and light decisions, but never consume biome shading or block-entity NBT.
/// Keep foreground/mixed batches unchanged to preserve cross-column biome
/// blends; no additional DB query or worker thread is introduced.
fn request_for_stream_batch(
    mut options: ChunkLoadOptions,
    positions: &[ChunkPos],
    view: Option<ViewState>,
) -> ChunkLoadOptions {
    if view.is_some_and(|view| {
        !positions.is_empty()
            && positions.iter().all(|pos| lazy_mesh_column(*pos, view))
    }) {
        options.data_request = ChunkDataRequest::new().full_3d_indices();
    }
    options
}

fn load_batch_from_world(
    world: &World,
    models: &bedrock_block_model::BlockModelRepository,
    positions: &[ChunkPos],
    options: ChunkLoadOptions,
    lazy_view: Option<ViewState>,
) -> LoadedChunkBatch {
    if positions.is_empty() {
        return LoadedChunkBatch {
            chunks: Vec::new(),
            stats: ChunkLoadStats::default(),
            query_time: Duration::ZERO,
            collect_time: Duration::ZERO,
        };
    }
    let started = Instant::now();
    let options = request_for_stream_batch(options, positions, lazy_view);
    let cancel = options.cancel.clone();
    let (chunks, stats) =
        match world.query_chunk_data_with_stats(positions.iter().copied(), options) {
            Ok(result) => result,
            Err(error) => {
                let message = format!("批量读取区块失败：{error}");
                return LoadedChunkBatch {
                    chunks: positions
                        .iter()
                        .map(|pos| (*pos, Err(message.clone())))
                        .collect(),
                    stats: ChunkLoadStats::default(),
                    query_time: started.elapsed(),
                    collect_time: Duration::ZERO,
                };
            }
        };
    let query_time = started.elapsed();
    let trace = trace_verbose();
    // Bedrock fades grass/foliage colors over the block and its eight neighbors. Build the
    // blended column tints once per batch so parallel chunk collection only reads them.
    let need_mesh_data = chunks.iter().any(|data| {
        data.is_loaded && !lazy_view.is_some_and(|view| lazy_mesh_column(data.pos, view))
    });
    // A batch containing only deferred rear columns never touches expensive
    // biome/model collection. When any foreground column is present, keep
    // its original neighbor biomes for exact tinting at chunk boundaries.
    let biome_blend = need_mesh_data
        .then(|| p3d::Preview3dBiomeBlendField::from_chunks(&chunks));
    let collect_started = Instant::now();
    let processed = chunks
        .into_par_iter()
        .map(|data| {
            let pos = data.pos;
            if cancel.as_ref().is_some_and(CancelFlag::is_cancelled) {
                return (pos, Err("区块预读已取消".to_string()));
            }
            (
                pos,
                if lazy_view.is_some_and(|view| lazy_mesh_column(pos, view)) {
                    collect_lazy_probe_chunk(data)
                } else {
                    collect_cached_chunk(data, models, trace, biome_blend.as_ref())
                },
            )
        })
        .collect::<Vec<_>>();
    let collect_time = collect_started.elapsed();
    if trace_verbose() {
        eprintln!(
            "world3d worker: load_batch {} chunks in {}ms",
            positions.len(),
            started.elapsed().as_millis()
        );
    }
    LoadedChunkBatch {
        chunks: processed,
        stats,
        query_time,
        collect_time,
    }
}

struct Worker {
    lighting: super::rendering::LightingMode,
    world: Arc<World>,
    resolver: Arc<ObjTextureResolver>,
    models: Arc<bedrock_block_model::BlockModelRepository>,
    probe_stash: HashMap<
        ChunkPos,
        (
            Arc<World3dChunkProbe>,
            ChunkVersion,
            Option<i32>,
            usize,
            Vec<FrameDisplay>,
        ),
    >,
    view: ViewState,
    cursor: ChunkCursor,
    cache: BTreeMap<ChunkPos, CachedChunk>,
    light_fields: HashMap<ChunkPos, Arc<lighting::ChunkLighting>>,
    // Immutable per-Probe halo decode, reused by the nine overlapping
    // 3x3 light solves as the camera crosses neighboring columns.
    light_columns: HashMap<ChunkPos, lighting::CachedLightColumn>,
    published_sections: BTreeMap<(ChunkPos, i32), Weak<World3dChunkMesh>>,
    /// Bounded strong cache of recently published Sections. A revisit of a
    /// previously loaded area reuses the surviving Arc (stable GPU identity,
    /// no re-upload) even after the scene dropped its reference; FIFO-bounded.
    revisit_cache: BTreeMap<(ChunkPos, i32), Arc<World3dChunkMesh>>,
    revisit_order: std::collections::VecDeque<(ChunkPos, i32)>,
    /// Completed unordered neighbour pairs, cleared on unload/invalidation.
    /// Unlike a per-column flag, a newly arrived neighbour still causes
    /// the previously meshed column to refresh its shared boundary exactly once.
    boundary_pairs: BTreeSet<(ChunkPos, ChunkPos)>,
    lod_modes: HashMap<ChunkPos, bool>,
    dirty_meshes: BTreeSet<ChunkPos>,
    /// Loaded metadata-only columns awaiting a camera-visible mesh request.
    /// Unlike dirty_meshes, these are NOT automatically processed in the
    /// background while the camera still faces away.
    deferred_columns: BTreeSet<ChunkPos>,
    /// Height bands whose CPU/GPU materialization was postponed by portal
    /// culling. Reevaluate when the camera moves or adjacent probes arrive.
    deferred_bands: BTreeSet<(ChunkPos, i32)>,
    urgent_chunks: BTreeSet<ChunkPos>,
    retained_invalidated: BTreeSet<ChunkPos>,
    missing: BTreeSet<ChunkPos>,
    failed_positions: BTreeSet<ChunkPos>,
    textures_by_path: HashMap<String, Option<Weak<TextureAsset>>>,
    texture_alpha_content: HashMap<String, TextureAlphaContent>,
    texture_lookup: HashMap<(String, [i32; 3]), Option<(Weak<TextureAsset>, TextureAlphaContent)>>,
    resolved_texture_lookup: HashMap<(String, [i32; 3]), Option<ObjResolvedTexture>>,
    mesh_assets: MeshAssets,
    pbr: pbr::PbrCache,
    // Batch-local immutable material classification. Reused across height
    // Sections so each distinct material+face-direction resolves its texture
    // sources/PBR attributes once, but never pins assets between mesh waves.
    face_material_cache: HashMap<(Arc<str>, [i32; 3]), FaceMaterialProperties>,
    resolver_time: Duration,
    texture_read_time: Duration,
    texture_decode_time: Duration,
    mesh_generation: u64,
    failed_chunks: usize,
    last_error: Option<String>,
    last_chunk_ms: u64,
    last_query_ms: u64,
    last_collect_ms: u64,
    last_load_stats: ChunkLoadStats,
    last_stage_ms: u64,
    last_face_ms: u64,
    last_convert_ms: u64,
    last_mesh_targets: usize,
    last_reused_sections: usize,
    last_boundary_hits: usize,
    last_rehydrate_ms: u64,
    last_send_wait_ms: u64,
    last_texture_lookup_ms: u64,
    last_texture_read_ms: u64,
    last_texture_decode_ms: u64,
    info: ChunkServiceInfo,
    last_status_at: Option<Instant>,
    last_material_index_flush_at: Option<Instant>,
}

impl Worker {
    fn new(config: &ChunkServiceConfig, world: World) -> Self {
        let roots = build_pack_roots(&config.world_path, &config.package_roots);
        if trace_timings() {
            eprintln!(
                "world3d worker: {} resource pack roots {roots:#?}",
                roots.len()
            );
        }
        let resolver = ObjTextureResolver::with_pack_roots(&roots, "textures");
        if let Some(revision) = resolver.material_index_revision() {
            resolver.open_material_index(
                &crate::utils::file_ops::cache_subdir("world3d/material_index"),
                revision,
            );
        }
        let resolver = Arc::new(resolver);
        let mut models = resolver.models().clone();
        if let Some(root) = discover_capture_library(&config.package_roots) {
            match models.load_capture_catalog(&root) {
                Ok(count) => {
                    if trace_timings() {
                        eprintln!(
                            "world3d: loaded {count} captured model states from {}",
                            root.display()
                        );
                    }
                }
                Err(error) => eprintln!(
                    "world3d: capture library ignored ({}): {error}",
                    root.display()
                ),
            }
        }
        // An installation capture takes precedence. The optional embedded
        // archive is version-specific: never apply its exact-state recipes to
        // an unrelated Minecraft build.
        if models.capture_catalog.is_none() {
            if let Ok(catalog) = bedrock_block_model::RuntimeCaptureCatalog::open_embedded() {
                if capture_version_matches_installation(
                    catalog.game_version(),
                    &config.package_roots,
                ) {
                    let count = catalog.state_count();
                    if trace_timings() {
                        eprintln!(
                            "world3d: using {count} embedded BSC model states for {}",
                            catalog.game_version()
                        );
                    }
                    models.capture_catalog = Some(Arc::new(catalog));
                }
            }
        }
        let models = Arc::new(models);
        let mut info = ChunkServiceInfo::default();
        info.block_model_count = models.blocks.len();
        info.resource_pack_count = roots.len();
        info.texture_resolver_errors = resolver.model_errors().len();
        Self {
            world: Arc::new(world),
            lighting: super::rendering::LightingMode::default(),
            resolver,
            models,
            probe_stash: HashMap::new(),
            view: ViewState {
                generation: 0,
                dimension: config.dimension,
                camera: [0.5, 96.0, 0.5],
                forward_xz: [0.0, -1.0],
                forward_y: 0.0,
                radius: config.view_radius.clamp(1, WORLD3D_MAX_VIEW_RADIUS),
                view_distance: 320.0,
            },
            cursor: ChunkCursor::default(),
            cache: BTreeMap::new(),
            light_fields: HashMap::new(),
            light_columns: HashMap::new(),
            published_sections: BTreeMap::new(),
            revisit_cache: BTreeMap::new(),
            revisit_order: std::collections::VecDeque::new(),
            boundary_pairs: BTreeSet::new(),
            lod_modes: HashMap::new(),
            dirty_meshes: BTreeSet::new(),
            deferred_columns: BTreeSet::new(),
            deferred_bands: BTreeSet::new(),
            urgent_chunks: BTreeSet::new(),
            retained_invalidated: BTreeSet::new(),
            missing: BTreeSet::new(),
            failed_positions: BTreeSet::new(),
            textures_by_path: HashMap::new(),
            texture_alpha_content: HashMap::new(),
            texture_lookup: HashMap::new(),
            resolved_texture_lookup: HashMap::new(),
            mesh_assets: MeshAssets::default(),
            pbr: pbr::PbrCache::default(),
            face_material_cache: HashMap::new(),
            resolver_time: Duration::ZERO,
            texture_read_time: Duration::ZERO,
            texture_decode_time: Duration::ZERO,
            mesh_generation: 0,
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
            last_reused_sections: 0,
            last_boundary_hits: 0,
            last_rehydrate_ms: 0,
            last_send_wait_ms: 0,
            last_texture_lookup_ms: 0,
            last_texture_read_ms: 0,
            last_texture_decode_ms: 0,
            info,
            last_status_at: None,
            last_material_index_flush_at: None,
        }
    }

    fn chunk_load_options(&self) -> ChunkLoadOptions {
        ChunkLoadOptions {
            data_request: ChunkDataRequest::new()
                .full_3d_indices()
                .biome(BiomeDataRequirement::All)
                .block_entities(),
            subchunk_decode: SubChunkDecodeMode::PackedIndices,
            threading: WorldThreadingOptions::Single,
            pipeline: WorldPipelineOptions::default(),
            cancel: None,
            progress: None,
            priority: ChunkLoadPriority::DistanceFrom {
                chunk_x: 0,
                chunk_z: 0,
            },
            storage_cache_policy: Default::default(),
        }
    }

    fn status(&self) -> ChunkServiceStatus {
        let cache_bytes = self
            .cache
            .values()
            .map(CachedChunk::estimated_cpu_bytes)
            .sum::<usize>()
            .saturating_add(
                self.light_fields
                    .values()
                    .map(|light| light.storage_bytes())
                    .sum::<usize>(),
            );
        let (mut texture_count, mut texture_bytes) = self
            .textures_by_path
            .values()
            .filter_map(|texture| texture.as_ref().and_then(Weak::upgrade))
            .fold((0usize, 0usize), |(count, bytes), texture| {
                (
                    count.saturating_add(1),
                    bytes.saturating_add(texture_storage_bytes(&texture)),
                )
            });
        let (tint_count, tint_bytes) = self.mesh_assets.tinted_texture_usage();
        texture_count = texture_count.saturating_add(tint_count);
        texture_bytes = texture_bytes.saturating_add(tint_bytes);
        let (center_x, center_z) = self.view_center_chunk();
        let radius = self.view.radius;
        let loaded_in_view = self
            .cache
            .keys()
            .filter(|pos| inside_view(**pos, self.view.dimension, center_x, center_z, radius))
            .count();
        let target_columns = super::chunk_range::column_count(radius);
        ChunkServiceStatus {
            generation: self.view.generation,
            dimension: self.view.dimension,
            radius: self.view.radius,
            loaded_chunks: self.cache.len(),
            coarse_chunks: self.lod_modes.values().filter(|coarse| **coarse).count(),
            pending_chunks: target_columns.saturating_sub(loaded_in_view),
            // Metadata-only deferred columns must not count as GPU-meshed.
            meshed_chunks: self.cache.len().saturating_sub(self.deferred_columns.len()),
            face_count: self.cache.values().map(|chunk| chunk.faces).sum(),
            texture_count,
            texture_bytes,
            cache_bytes,
            missing_chunks: self
                .missing
                .iter()
                .filter(|pos| inside_view(**pos, self.view.dimension, center_x, center_z, radius))
                .count(),
            failed_chunks: self.failed_chunks,
            last_error: self.last_error.clone(),
            last_chunk_ms: self.last_chunk_ms,
            last_query_ms: self.last_query_ms,
            last_collect_ms: self.last_collect_ms,
            last_load_stats: self.last_load_stats,
            last_stage_ms: self.last_stage_ms,
            last_face_ms: self.last_face_ms,
            last_convert_ms: self.last_convert_ms,
            last_mesh_targets: self.last_mesh_targets,
            model_count: self.info.block_model_count,
            pack_count: self.info.resource_pack_count,
        }
    }

    fn publish_status(&mut self, tx: &SyncSender<ChunkServiceEvent>) {
        // Material lookups are in memory; avoid rewriting the growing index on every status update.
        let now = Instant::now();
        if material_index_flush_due(self.last_material_index_flush_at, now) {
            self.resolver.flush_material_index();
            self.last_material_index_flush_at = Some(now);
        }
        self.mesh_assets.prune();
        self.textures_by_path
            .retain(|_, asset| asset.as_ref().is_none_or(|asset| asset.strong_count() > 0));
        self.texture_lookup.retain(|_, asset| {
            asset
                .as_ref()
                .is_none_or(|(texture, _)| texture.strong_count() > 0)
        });
        self.last_status_at = Some(Instant::now());
        let _ = tx.send(ChunkServiceEvent::Status(self.status()));
    }

    fn view_center_chunk(&self) -> (i32, i32) {
        (
            (self.view.camera[0] / 16.0).floor() as i32,
            (self.view.camera[2] / 16.0).floor() as i32,
        )
    }

    fn stream(
        &mut self,
        tx: &SyncSender<ChunkServiceEvent>,
        commands: &Receiver<ChunkServiceCommand>,
    ) -> Option<ChunkServiceCommand> {
        if let Ok(command) = commands.try_recv() {
            return Some(command);
        }
        let (center_x, center_z) = self.view_center_chunk();
        // Keep previously loaded chunks across camera turns and evict only after spatial travel.
        let stale = self
            .cache
            .keys()
            .copied()
            .filter(|pos| {
                !inside_cache_hysteresis(
                    *pos,
                    self.view.dimension,
                    center_x,
                    center_z,
                    self.view.radius,
                )
            })
            .collect::<Vec<_>>();
        if trace_timings() && !stale.is_empty() {
            eprintln!(
                "world3d evict ts={} gen={} center={},{} radius={} count={} cached_before={}",
                trace_timestamp_ms(),
                self.view.generation,
                center_x,
                center_z,
                self.view.radius,
                stale.len(),
                self.cache.len(),
            );
        }
        for pos in stale {
            self.cache.remove(&pos);
            self.light_fields.remove(&pos);
            self.light_columns.remove(&pos);
            forget_published_sections(&mut self.published_sections, pos, &[]);
            forget_boundary_pairs(&mut self.boundary_pairs, pos);
            self.lod_modes.remove(&pos);
            self.dirty_meshes.remove(&pos);
            self.deferred_columns.remove(&pos);
            self.deferred_bands.retain(|(column, _)| *column != pos);
            self.mark_neighbor_meshes(pos);
            let _ = tx.send(ChunkServiceEvent::Unloaded {
                generation: self.view.generation,
                pos,
            });
        }
        self.light_columns.retain(|pos, _| {
            inside_cache_hysteresis(
                *pos,
                self.view.dimension,
                center_x,
                center_z,
                self.view.radius,
            )
        });
        self.light_fields.retain(|pos, _| {
            inside_cache_hysteresis(
                *pos,
                self.view.dimension,
                center_x,
                center_z,
                self.view.radius,
            )
        });
        let stale_invalidated = self
            .retained_invalidated
            .iter()
            .copied()
            .filter(|pos| {
                !inside_cache_hysteresis(
                    *pos,
                    self.view.dimension,
                    center_x,
                    center_z,
                    self.view.radius,
                )
            })
            .collect::<Vec<_>>();
        for pos in stale_invalidated {
            self.retained_invalidated.remove(&pos);
            forget_published_sections(&mut self.published_sections, pos, &[]);
            forget_boundary_pairs(&mut self.boundary_pairs, pos);
            let _ = tx.send(ChunkServiceEvent::Unloaded {
                generation: self.view.generation,
                pos,
            });
        }
        if let Ok(command) = commands.try_recv() {
            return Some(command);
        }
        let remesh = take_renderable_dirty_meshes(
            &mut self.dirty_meshes,
            self.view,
            REMESH_BATCH_CHUNKS,
        );
        if !remesh.is_empty() {
            self.mesh_targets(tx, &remesh, false);
            if self
                .last_status_at
                .is_none_or(|at| at.elapsed() >= Duration::from_millis(250))
            {
                self.publish_status(tx);
            }
        }
        self.missing.retain(|pos| {
            inside_cache_hysteresis(
                *pos,
                self.view.dimension,
                center_x,
                center_z,
                self.view.radius,
            )
        });
        retain_urgent_chunks(&mut self.urgent_chunks, self.view);

        let mut pending_batch: Option<PendingChunkBatch> = None;
        let mut batch_number = 0u64;
        loop {
            if let Ok(command) = commands.try_recv() {
                if let Some(pending) = pending_batch.take() {
                    self.urgent_chunks.extend(pending.positions.iter().copied());
                }
                return Some(command);
            }
            // A freshly turned camera may already have metadata/blocks in the
            // cache, yet the old loop kept loading an entire render radius
            // before it ever processed those now-visible dirty meshes.
            // Interleave one bounded mesh wave with each DB read batch. Keep
            // pending prefetched I/O live so the two stages overlap, but never
            // launch more workers or build an unbounded GPU upload backlog.
            let ready = take_renderable_dirty_meshes(
                &mut self.dirty_meshes,
                self.view,
                interleaved_mesh_wave_size(
                    self.last_face_ms, self.last_convert_ms, self.last_send_wait_ms,
                ),
            );
            if !ready.is_empty() {
                self.mesh_targets(tx, &ready, false);
                if let Ok(command) = commands.try_recv() {
                    if let Some(pending) = pending_batch.take() {
                        self.urgent_chunks.extend(pending.positions.iter().copied());
                    }
                    return Some(command);
                }
            }
            let batch_started = Instant::now();
            let (loaded, load_time, prefetched) = if let Some(pending) = pending_batch.take() {
                match pending.wait(commands) {
                    BatchWait::Loaded(loaded, load_time) => (loaded, load_time, true),
                    BatchWait::Command(command) => {
                        self.urgent_chunks.extend(pending.positions.iter().copied());
                        return Some(command);
                    }
                    BatchWait::Disconnected => {
                        self.urgent_chunks.extend(pending.positions.iter().copied());
                        let _ = tx.send(ChunkServiceEvent::Fatal {
                            error: "区块预读任务意外结束".to_string(),
                        });
                        break;
                    }
                }
            } else {
                let batch = self.next_batch(center_x, center_z);
                if batch.is_empty() {
                    break;
                }
                let started = Instant::now();
                let loaded = self.load_batch_lazy(&batch);
                (loaded, started.elapsed(), false)
            };
            if let Ok(command) = commands.try_recv() {
                self.urgent_chunks
                    .extend(loaded.chunks.iter().map(|(pos, _)| *pos));
                return Some(command);
            }
            let wait_time = batch_started.elapsed();
            batch_number = batch_number.saturating_add(1);
            let requested_chunks = loaded.chunks.len();
            self.last_chunk_ms = u64::try_from(load_time.as_millis()).unwrap_or(u64::MAX);
            self.last_query_ms = u64::try_from(loaded.query_time.as_millis()).unwrap_or(u64::MAX);
            self.last_collect_ms =
                u64::try_from(loaded.collect_time.as_millis()).unwrap_or(u64::MAX);
            self.last_load_stats = loaded.stats;
            let mut newly_loaded = Vec::new();
            for (pos, result) in loaded.chunks {
                match result {
                    Ok(Some(chunk)) => {
                        let deferred = chunk.blocks.is_none();
                        self.cache.insert(pos, chunk);
                        self.missing.remove(&pos);
                        self.failed_positions.remove(&pos);
                        if deferred {
                            self.deferred_columns.insert(pos);
                        } else {
                            self.deferred_columns.remove(&pos);
                            newly_loaded.push(pos);
                        }
                    }
                    Ok(None) => {
                        self.failed_positions.remove(&pos);
                        self.missing.insert(pos);
                        self.light_fields.remove(&pos);
                        self.lod_modes.remove(&pos);
                        if self.retained_invalidated.remove(&pos) {
                            self.mark_neighbor_meshes(pos);
                            forget_published_sections(&mut self.published_sections, pos, &[]);
                            let _ = tx.send(ChunkServiceEvent::Unloaded {
                                generation: self.view.generation,
                                pos,
                            });
                        }
                    }
                    Err(error) => {
                        self.failed_chunks = self.failed_chunks.saturating_add(1);
                        self.last_error = Some(error.clone());
                        // A failed read does not prove the world has no chunk. Keep any
                        // previously displayed mesh and report the failure; retry after
                        // a spatial view change or explicit edit invalidation.
                        self.failed_positions.insert(pos);
                        let _ = tx.send(ChunkServiceEvent::Fatal { error });
                    }
                }
            }
            let loaded_chunks = newly_loaded.len();
            let textures_before = self.textures_by_path.len();
            let schedule_started = Instant::now();
            if !newly_loaded.is_empty() {
                let next_batch = self.next_batch(center_x, center_z);
                if !next_batch.is_empty() {
                    pending_batch = Some(self.prefetch_batch(next_batch));
                }
            }
            let schedule_time = schedule_started.elapsed();
            let mesh_started = Instant::now();
            if !newly_loaded.is_empty() {
                prioritize_stream_meshes(
                    &mut newly_loaded,
                    (center_x, center_z),
                    self.view.forward_xz,
                );
                // Publish one small immutable Section wave at a time rather
                // than making the owner absorb an entire 16-column upload
                // burst. Neighbor masks are supplied from the same cache;
                // previously meshed seams are queued by boundary_pairs.
                for (index, wave) in newly_loaded.chunks(STREAM_MESH_BATCH_CHUNKS).enumerate() {
                    self.mesh_targets(tx, wave, false);
                    if let Ok(command) = commands.try_recv() {
                        // Do not lose a decoded column when a newer camera
                        // view preempts the current scene build. The ordinary
                        // dirty queue resumes its Section meshing when visible.
                        let done = (index + 1) * STREAM_MESH_BATCH_CHUNKS;
                        self.dirty_meshes.extend(
                            newly_loaded[done.min(newly_loaded.len())..].iter().copied(),
                        );
                        if let Some(pending) = pending_batch.take() {
                            self.urgent_chunks.extend(pending.positions.iter().copied());
                        }
                        return Some(command);
                    }
                }
            } else {
                self.last_stage_ms = 0;
                self.last_rehydrate_ms = 0;
                self.last_face_ms = 0;
                self.last_convert_ms = 0;
                self.last_send_wait_ms = 0;
                self.last_texture_lookup_ms = 0;
                self.last_texture_read_ms = 0;
                self.last_texture_decode_ms = 0;
                self.last_mesh_targets = 0;
                self.last_reused_sections = 0;
                self.last_boundary_hits = 0;
            }
            let mesh_time = mesh_started.elapsed();
            if trace_timings() {
                eprintln!(
                    "world3d batch ts={} gen={} id={} prefetched={} requested={} loaded={} load_ms={} wait_ms={} query_ms={} db_ms={} decode_ms={} collect_ms={} schedule_ms={} mesh_ms={} stage_ms={} rehydrate_ms={} faces_ms={} convert_ms={} send_wait_ms={} tex_lookup_ms={} tex_read_ms={} tex_decode_ms={} new_textures={} targets={} reused_sections={} boundary_hits={} cache_chunks={} lazy_pending={} cache_mib={:.1}",
                    trace_timestamp_ms(),
                    self.view.generation,
                    batch_number,
                    prefetched,
                    requested_chunks,
                    loaded_chunks,
                    load_time.as_millis(),
                    wait_time.as_millis(),
                    self.last_query_ms,
                    self.last_load_stats.db_read_ms,
                    self.last_load_stats.decode_ms,
                    self.last_collect_ms,
                    schedule_time.as_millis(),
                    mesh_time.as_millis(),
                    self.last_stage_ms,
                    self.last_rehydrate_ms,
                    self.last_face_ms,
                    self.last_convert_ms,
                    self.last_send_wait_ms,
                    self.last_texture_lookup_ms,
                    self.last_texture_read_ms,
                    self.last_texture_decode_ms,
                    self.textures_by_path.len().saturating_sub(textures_before),
                    self.last_mesh_targets,
                    self.last_reused_sections,
                    self.last_boundary_hits,
                    self.cache.len(),
                    self.deferred_columns.len(),
                    self.cache
                        .values()
                        .map(CachedChunk::estimated_cpu_bytes)
                        .sum::<usize>() as f64
                        / 1048576.0,
                );
            }
            if let Ok(command) = commands.try_recv() {
                if let Some(pending) = pending_batch.take() {
                    self.urgent_chunks.extend(pending.positions.iter().copied());
                }
                return Some(command);
            }
            if self
                .last_status_at
                .is_none_or(|at| at.elapsed() >= std::time::Duration::from_millis(250))
            {
                self.publish_status(tx);
            }
        }
        let dirty = renderable_dirty_meshes(&self.dirty_meshes, self.view);
        for remesh in dirty.chunks(REMESH_BATCH_CHUNKS) {
            if let Ok(command) = commands.try_recv() {
                return Some(command);
            }
            let remesh = remesh
                .iter()
                .copied()
                .filter(|pos| self.dirty_meshes.remove(pos))
                .collect::<Vec<_>>();
            if remesh.is_empty() {
                continue;
            }
            self.mesh_targets(tx, &remesh, false);
            if self
                .last_status_at
                .is_none_or(|at| at.elapsed() >= Duration::from_millis(250))
            {
                self.publish_status(tx);
            }
        }
        self.publish_status(tx);
        None
    }

    fn next_batch(&mut self, center_x: i32, center_z: i32) -> Vec<ChunkPos> {
        let mut batch = Vec::with_capacity(LOAD_BATCH_CHUNKS);
        let urgent = take_visible_urgent_chunks(
            &mut self.urgent_chunks,
            self.view.dimension,
            center_x,
            center_z,
            self.view.radius,
            LOAD_BATCH_CHUNKS,
        );
        for pos in urgent {
            if !self.cache.contains_key(&pos)
                && !self.missing.contains(&pos)
                && !self.failed_positions.contains(&pos)
            {
                batch.push(pos);
            }
        }
        while batch.len() < LOAD_BATCH_CHUNKS {
            let Some(pos) = self.cursor.next(
                self.view.dimension,
                center_x,
                center_z,
                self.view.radius,
                self.view.forward_xz,
            ) else {
                break;
            };
            if !self.cache.contains_key(&pos)
                && !self.missing.contains(&pos)
                && !self.failed_positions.contains(&pos)
                && !batch.contains(&pos)
            {
                batch.push(pos);
            }
        }
        batch
    }

    fn prefetch_batch(&self, positions: Vec<ChunkPos>) -> PendingChunkBatch {
        let world = Arc::clone(&self.world);
        let models = Arc::clone(&self.models);
        let cancel = CancelFlag::new();
        let mut options = self.batch_load_options(positions.len());
        options.cancel = Some(cancel.clone());
        let (sender, receiver) = sync_channel(1);
        let requested = positions.clone();
        let view = self.view;
        rayon::spawn(move || {
            let started = Instant::now();
            let loaded = load_batch_from_world(&world, &models, &positions, options, Some(view));
            let _ = sender.send((loaded, started.elapsed()));
        });
        PendingChunkBatch {
            receiver,
            cancel,
            positions: requested,
        }
    }

    fn mark_neighbor_meshes(&mut self, pos: ChunkPos) {
        let neighbors = mesh_target_positions(&[pos], true, |_, neighbor| {
            self.cache.contains_key(neighbor)
        });
        self.dirty_meshes.extend(
            neighbors.into_iter().filter(|neighbor| {
                *neighbor != pos && !self.deferred_columns.contains(neighbor)
            }),
        );
    }

    /// Reuse the UI's conservative 6-face portal graph before any Section
    /// mesh exists. Cached metadata-only columns are deliberately represented
    /// as open space; they can never make another Section falsely invisible.
    fn portal_hidden_bands(
        &self,
        candidates: &BTreeSet<(ChunkPos, i32)>,
    ) -> BTreeSet<(ChunkPos, i32)> {
        if candidates.is_empty()
            || portal_culling_disabled()
        {
            return BTreeSet::new();
        }
        let mut probes = self
            .cache
            .iter()
            .map(|(&pos, chunk)| (pos, Arc::clone(&chunk.probe)))
            .collect::<BTreeMap<_, _>>();
        probes.extend(
            self.probe_stash
                .iter()
                .map(|(&pos, (probe, ..))| (pos, Arc::clone(probe))),
        );
        super::visibility::hidden_render_band_keys(
            &probes,
            candidates,
            self.view.dimension,
            self.view.camera.map(|value| value.floor() as i32),
            self.view.radius,
        )
    }

    /// A skipped GPU band is not a discarded or finished Section. Promote it
    /// as soon as the camera crosses into a different air-connected region,
    /// or an adjacent decoded column reveals a previously unknown opening.
    fn promote_visible_bands(&mut self) {
        if self.deferred_bands.is_empty() {
            return;
        }
        let still_hidden = self.portal_hidden_bands(&self.deferred_bands);
        let visible = self
            .deferred_bands
            .difference(&still_hidden)
            .copied()
            .collect::<Vec<_>>();
        for (pos, band) in visible {
            self.deferred_bands.remove(&(pos, band));
            if self.cache.contains_key(&pos) {
                self.dirty_meshes.insert(pos);
            }
        }
    }

    fn mesh_targets(
        &mut self,
        tx: &SyncSender<ChunkServiceEvent>,
        loaded: &[ChunkPos],
        include_neighbors: bool,
    ) {
        // Keep hot material metadata across every Section in this mesh wave,
        // not beyond it. Texture eviction and lighting mode changes therefore
        // continue to follow the existing Worker-owned resource lifecycle.
        self.face_material_cache.clear();
        let trace = trace_timings();
        let mut rehydrate_time = Duration::ZERO;
        let mut send_wait_time = Duration::ZERO;
        let mut texture_lookup_time = Duration::ZERO;
        let mut texture_read_time = Duration::ZERO;
        let mut texture_decode_time = Duration::ZERO;
        let targets = mesh_target_positions(loaded, include_neighbors, |_, neighbor| {
            self.cache.contains_key(neighbor) && !self.deferred_columns.contains(neighbor)
        });
        let (center_x, center_z) = self.view_center_chunk();
        let priority_targets = loaded
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        let mut targets = targets.into_iter().collect::<Vec<_>>();
        targets.sort_by_key(|pos| {
            mesh_target_priority(
                *pos,
                &priority_targets,
                (center_x, center_z),
                self.view.forward_xz,
            )
        });
        let dimension = self.view.dimension;
        let mut local_positions = BTreeSet::new();
        for pos in &targets {
            local_positions.insert(*pos);
            for (dx, dz) in CHUNK_NEIGHBOR_OFFSETS {
                local_positions.insert(ChunkPos {
                    x: pos.x + dx,
                    z: pos.z + dz,
                    dimension,
                });
            }
        }
        let bounds = bounds_for_positions(dimension, local_positions.iter().copied());
        let Some(bounds) = bounds else {
            return;
        };
        let generation = self.view.generation;
        let mesh_started = Instant::now();
        if trace_verbose() {
            eprintln!(
                "world3d worker: mesh start targets={} cache={}",
                targets.len(),
                self.cache.len()
            );
        }
        let mut builder = p3d::Preview3dMeshBuilder::new_stream(bounds);
        builder.set_ambient_occlusion(self.lighting.is_lit());
        let mut boundary_hits = 0usize;
        let target_set = targets.iter().copied().collect::<HashSet<_>>();
        // Rehydrating each discarded target separately serializes world queries and block
        // collection. Reuse the normal parallel batch loader while keeping cached metadata
        // and neighbor-only boundary summaries owned by the original entries.
        // A non-target neighbour with a retained boundary summary already
        // supplies its seam classification to insert_stream_boundary below.
        // A deferred neighbor without a retained boundary is still a known,
        // loaded column. Rehydrate it for seam classification even if its own
        // GPU mesh remains deferred; retain existing compact boundaries.
        let rehydrate_positions = local_positions
            .iter()
            .copied()
            .filter(|pos| {
                self.cache.get(pos).is_some_and(|chunk| {
                    needs_mesh_neighbor_hydration(
                        target_set.contains(pos),
                        chunk.blocks.is_some(),
                        chunk.boundary.is_some(),
                    )
                })
            })
            .collect::<Vec<_>>();
        // The previous second pass reloaded every neighbour with a cached
        // boundary, even though the builder immediately chose that boundary
        // and never consumed the newly fetched block data. Keep those
        // accurate cached summaries resident and skip redundant I/O.
        let mut rehydrated = if rehydrate_positions.is_empty() {
            HashMap::new()
        } else {
            let started = Instant::now();
            let loaded = self.load_batch(&rehydrate_positions);
            if trace {
                rehydrate_time += started.elapsed();
            }
            loaded.chunks.into_iter().collect::<HashMap<_, _>>()
        };
        // A deferred neighbor can be parsed for a newly visible seam before
        // it is itself renderable. Do not promote it to a GPU mesh here: its
        // opaque-block snapshot only supplies neighbor face decisions.
        for pos in local_positions {
            if !target_set.contains(&pos) {
                // A neighboring full column is needed for occupancy and AO
                // decisions, not for its complete model/texture expansion.
                // Build its compact halo once and keep the original blocks
                // resident for its own later mesh wave. This also prevents
                // repeated full-column remove/reinsert across wavefronts.
                let boundary = self.cache.get_mut(&pos).and_then(|chunk| {
                    if chunk.boundary.is_none()
                        && let Some(blocks) = chunk.blocks.as_mut()
                    {
                        chunk.boundary = Some(Arc::new(blocks.boundary_snapshot()));
                    }
                    chunk.boundary.clone()
                });
                if let Some(boundary) = boundary {
                    builder.insert_stream_boundary(pos, boundary);
                    boundary_hits += 1;
                    continue;
                }
            }
            let Some(mut chunk) = self.cache.remove(&pos) else {
                continue;
            };
            let blocks = match chunk.blocks.take() {
                Some(blocks) => blocks,
                None => match rehydrated.remove(&pos).unwrap_or(Ok(None)) {
                    Ok(Some(mut loaded)) => {
                        let Some(blocks) = loaded.blocks.take() else {
                            self.cache.insert(pos, chunk);
                            continue;
                        };
                        // A metadata-only column has no materialized frames or
                        // certified portal blockers yet. Adopt the full probe
                        // and frame-display metadata at its first real mesh
                        // build. Already meshed columns get a fresh immutable
                        // probe after a rehydration, too.
                        chunk.probe = loaded.probe;
                        chunk.version = loaded.version;
                        chunk.state_version = loaded.state_version;
                        chunk.frame_displays = loaded.frame_displays;
                        blocks
                    }
                    Ok(None) => {
                        self.cache.insert(pos, chunk);
                        continue;
                    }
                    Err(error) => {
                        self.last_error = Some(error);
                        self.failed_chunks = self.failed_chunks.saturating_add(1);
                        self.cache.insert(pos, chunk);
                        continue;
                    }
                },
            };
            builder.insert_stream_chunk(pos, blocks);
            // Stash non-mesh metadata so it can be restored after the builder drains blocks.
            self.probe_stash.insert(
                pos,
                (
                    chunk.probe,
                    chunk.version,
                    chunk.state_version,
                    chunk.faces,
                    chunk.frame_displays,
                ),
            );
        }
        // Test Section reachability before material/texture/GPU conversion.
        // The candidate set is built from decoded 16-block subchunk keys,
        // so not-yet-meshed Sections can be culled without allocating a Mesh.
        let mut prospective_bands = BTreeSet::new();
        for &pos in &targets {
            if let Some((probe, _, _, _, _)) = self.probe_stash.get(&pos) {
                for &subchunk in probe.subchunks.keys() {
                    prospective_bands.insert((pos, i32::from(subchunk).div_euclid(4)));
                }
            }
        }
        let hidden_bands = self
            .portal_hidden_bands(&prospective_bands)
            .into_iter()
            .filter(|(pos, _)| !self.retained_invalidated.contains(pos))
            .collect::<BTreeSet<_>>();
        // Record the full plan, not just Sections that survived face
        // generation. Otherwise a fully skipped GPU band could never be
        // promoted when its cave portal becomes reachable.
        self.deferred_bands.extend(hidden_bands.iter().copied());
        if trace && !hidden_bands.is_empty() {
            eprintln!(
                "world3d portal_mesh_skip gen={} targets={} hidden_bands={}",
                self.view.generation,
                targets.len(),
                hidden_bands.len(),
            );
        }
        self.last_stage_ms = u64::try_from(mesh_started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut face_elapsed = Duration::ZERO;
        let mut convert_elapsed = Duration::ZERO;
        let mut face_count = 0usize;
        let mut pass_faces = [0usize; 6];
        let mut completed_sections = BTreeMap::<ChunkPos, Vec<i32>>::new();
        let mut retained_sections = BTreeMap::<ChunkPos, Vec<Arc<World3dChunkMesh>>>::new();
        let mut failed_sections = BTreeSet::new();
        let mut completed_modes = HashMap::new();
        let mut reused_sections = 0usize;
        let coarse_targets = targets
            .iter()
            .copied()
            .filter(|pos| {
                coarse_lod_for_view(
                    *pos,
                    self.view,
                    self.lod_modes.get(pos).copied().unwrap_or(false),
                )
            })
            .collect::<HashSet<_>>();
        // Only reuse untouched height bands for previously published, unchanged cube-only
        // neighbors. Models and edit invalidations can affect geometry beyond this range.
        let mut partial_bands = HashMap::<ChunkPos, Vec<(i32, i32)>>::new();
        let light_started = Instant::now();
        let light_fields = targets
            .iter()
            .copied()
            .filter(|_| self.lighting.is_lit())
            .map(|pos| (pos, self.build_lighting(pos)))
            .collect::<HashMap<_, _>>();
        if trace && light_started.elapsed() >= Duration::from_millis(1) {
            let reused = light_fields
                .iter()
                .filter(|(pos, light)| {
                    light.as_ref().ok().is_some_and(|light| {
                        self.light_fields
                            .get(pos)
                            .is_some_and(|old| Arc::ptr_eq(old, light))
                    })
                })
                .count();
            eprintln!(
                "world3d light gen={} fields={} reused={} solve_ms={:.2}",
                self.view.generation,
                light_fields.len(),
                reused,
                light_started.elapsed().as_secs_f64() * 1000.
            );
        }
        if include_neighbors {
            for &pos in &targets {
                if priority_targets.contains(&pos)
                    || self.dirty_meshes.contains(&pos)
                    || self.retained_invalidated.contains(&pos)
                    || self.lod_modes.get(&pos) != Some(&false)
                    || coarse_targets.contains(&pos)
                    || !builder.stream_is_plain_opaque(pos)
                    || self
                        .probe_stash
                        .get(&pos)
                        .is_none_or(|entry| !entry.4.is_empty())
                {
                    continue;
                }
                let mut bands = Vec::new();
                let mut safe = true;
                for &source in loaded.iter().filter(|&&source| {
                    source != pos
                        && (i64::from(source.x) - i64::from(pos.x)).abs() <= 1
                        && (i64::from(source.z) - i64::from(pos.z)).abs() <= 1
                }) {
                    if self.retained_invalidated.contains(&source)
                        || self.lod_modes.get(&source) == Some(&true)
                        || coarse_targets.contains(&source)
                    {
                        safe = false;
                        break;
                    }
                    let Some((min_y, max_y)) = builder.stream_height_range(source) else {
                        safe = false;
                        break;
                    };
                    bands.push(neighbor_affected_bands(min_y, max_y));
                }
                if let Some(Ok(light)) = light_fields.get(&pos) {
                    bands.extend(light.changed_bands(self.light_fields.get(&pos).map(Arc::as_ref)));
                } else if self.lighting.is_lit() {
                    safe = false;
                }
                if safe && !bands.is_empty() {
                    let keep = self
                        .published_sections
                        .range((pos, i32::MIN)..=(pos, i32::MAX))
                        .filter_map(|((_, band), mesh)| {
                            if bands.iter().any(|&(min, max)| (min..=max).contains(band)) {
                                return None;
                            }
                            mesh.upgrade()
                        })
                        .collect::<Vec<_>>();
                    reused_sections += keep.len();
                    completed_sections
                        .insert(pos, keep.iter().map(|mesh| mesh.section_y).collect());
                    retained_sections.insert(pos, keep);
                    partial_bands.insert(pos, bands);
                }
            }
        }
        let selections = partial_bands
            .iter()
            .map(|(&pos, bands)| {
                (
                    pos,
                    p3d::Preview3dBandSelection {
                        affected: bands.clone(),
                        retained: completed_sections
                            .get(&pos)
                            .into_iter()
                            .flatten()
                            .copied()
                            .collect(),
                        band_height: MESH_BAND_BLOCKS,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let mut coarse_faces_by_pos = None::<HashMap<ChunkPos, p3d::Preview3dStreamChunkFaces>>;
        let (first, remaining) = targets.split_at(targets.len().min(4));
        for batch in std::iter::once(first).chain(remaining.chunks(8)) {
            let face_started = Instant::now();
            let (coarse, fine): (Vec<_>, Vec<_>) = batch
                .iter()
                .copied()
                .partition(|pos| coarse_targets.contains(pos));
            let mut gpu_targets = fine
                .iter()
                .copied()
                .filter(|pos| {
                    self.probe_stash
                        .get(pos)
                        .is_some_and(|(_, _, _, _, frames)| frames.is_empty())
                })
                .collect::<HashSet<_>>();
            let mut gpu_bands = HashMap::new();
            let mut gpu_sections = Vec::new();
            for pos in gpu_targets.clone() {
                let skipped = hidden_bands
                    .iter()
                    .filter_map(|(candidate, band)| (*candidate == pos).then_some(*band))
                    .collect::<BTreeSet<_>>();
                let Some(bands) =
                    builder.gpu_cube_bands_skipping(pos, selections.get(&pos), &skipped)
                else {
                    gpu_targets.remove(&pos);
                    continue;
                };
                for mut band in bands {
                    let faces = std::mem::take(&mut band.faces);
                    gpu_sections.push((pos, band.section_y, faces));
                    gpu_bands.insert((pos, band.section_y), band);
                }
            }
            let mut faces =
                builder.build_stream_faces_with_gpu_cubes_skipping(
                    &fine,
                    &selections,
                    &gpu_targets,
                    &hidden_bands,
                );
            if !coarse.is_empty() && coarse_faces_by_pos.is_none() {
                coarse_faces_by_pos = Some(
                    builder
                        .build_stream_lod_faces(
                            &coarse_targets.iter().copied().collect::<Vec<_>>(),
                            FAR_LOD_SCALE,
                        )
                        .into_iter()
                        .map(|chunk| (chunk.pos, chunk))
                        .collect(),
                );
            }
            let coarse_faces = coarse_faces_by_pos.as_mut();
            let coarse_fallback = coarse
                .iter()
                .copied()
                .filter(|pos| {
                    coarse_faces
                        .as_ref()
                        .is_none_or(|faces| !faces.contains_key(pos))
                })
                .collect::<Vec<_>>();
            if let Some(coarse_faces) = coarse_faces {
                faces.extend(coarse.iter().filter_map(|pos| coarse_faces.remove(pos)));
            }
            faces.extend(builder.build_stream_faces(&coarse_fallback));
            for pos in &fine {
                completed_modes.insert(*pos, false);
            }
            for pos in &coarse {
                completed_modes.insert(*pos, true);
            }
            face_elapsed += face_started.elapsed();
            face_count += faces.iter().map(|chunk| chunk.faces.len()).sum::<usize>();
            if trace_verbose() {
                for face in faces.iter().flat_map(|chunk| &chunk.faces) {
                    let pass = match face.pass {
                        p3d::Preview3dStreamPass::Opaque => 0,
                        p3d::Preview3dStreamPass::DetailOpaque => 1,
                        p3d::Preview3dStreamPass::Glass => 2,
                        p3d::Preview3dStreamPass::DetailGlass => 3,
                        p3d::Preview3dStreamPass::Water => 4,
                        p3d::Preview3dStreamPass::Lava => 5,
                    };
                    pass_faces[pass] += 1;
                }
            }
            let convert_started = Instant::now();
            let mut pending_sections = Vec::new();
            for (pos, section, faces) in gpu_sections {
                if let Some((probe, _, _, _, _)) = self.probe_stash.get(&pos) {
                    completed_sections.entry(pos).or_default();
                    pending_sections.push((pos, section, faces, probe.clone()));
                }
            }
            for mut chunk_faces in faces {
                if !targets.contains(&chunk_faces.pos) {
                    continue;
                }
                let Some((probe, _, _, _, frame_displays)) = self.probe_stash.get(&chunk_faces.pos)
                else {
                    continue;
                };
                let probe = Arc::clone(probe);
                // A map inside an item frame uses a flat model: keep the map quad and drop the frame's
                // raised border geometry so the map cannot clip through it.
                let map_frames = frame_displays
                    .iter()
                    .filter(|display| display.is_map())
                    .map(|display| display.position())
                    .collect::<Vec<_>>();
                if !map_frames.is_empty() {
                    chunk_faces
                        .faces
                        .retain(|face| !map_frames.iter().any(|pos| face_inside_block(face, *pos)));
                }
                chunk_faces
                    .faces
                    .extend(frame_displays.iter().map(FrameDisplay::face));
                let intern_started = Instant::now();
                intern_face_materials(&mut chunk_faces.faces);
                let intern_time = intern_started.elapsed();
                let faces_built = chunk_faces.faces.len();
                let sections =
                    partition_section_faces(chunk_faces.pos, chunk_faces.faces, self.view);
                for (section_y, faces) in sections {
                    if let Some((_, _, existing, _)) =
                        pending_sections.iter_mut().find(|(pos, section, _, _)| {
                            *pos == chunk_faces.pos && *section == section_y
                        })
                    {
                        existing.extend(faces);
                    } else {
                        pending_sections.push((chunk_faces.pos, section_y, faces, probe.clone()));
                    }
                }
                if trace && intern_time >= Duration::from_millis(250) {
                    eprintln!(
                        "world3d slow_intern ts={} pos={},{} faces={} intern_ms={}",
                        trace_timestamp_ms(),
                        chunk_faces.pos.x,
                        chunk_faces.pos.z,
                        faces_built,
                        intern_time.as_millis(),
                    );
                }
                self.dirty_meshes.remove(&chunk_faces.pos);
                completed_sections.entry(chunk_faces.pos).or_default();
            }
            pending_sections.sort_by_key(|(pos, section_y, _, _)| {
                section_priority(*pos, *section_y, self.view)
            });
            for (&pos, light) in &light_fields {
                if let Ok(light) = light {
                    self.light_fields.insert(pos, light.clone());
                }
            }
            for (pos, section_y, mut faces, probe) in pending_sections {
                if hidden_bands.contains(&(pos, section_y))
                    && !self.retained_invalidated.contains(&pos)
                {
                    self.deferred_bands.insert((pos, section_y));
                    continue;
                }
                self.deferred_bands.remove(&(pos, section_y));
                if partial_bands.get(&pos).is_some_and(|bands| {
                    !bands
                        .iter()
                        .any(|&(min, max)| (min..=max).contains(&section_y))
                        && self
                            .published_sections
                            .get(&(pos, section_y))
                            .and_then(Weak::upgrade)
                            .is_some()
                }) {
                    continue;
                }
                let build_started = Instant::now();
                if self.lighting != super::rendering::LightingMode::Vanilla {
                    for face in &mut faces {
                        face.shade = 1.;
                    }
                }
                if !self.lighting.is_lit() {
                    for face in &mut faces {
                        face.ambient_light = [1.; 4];
                    }
                }
                let light = light_fields
                    .get(&pos)
                    .ok_or_else(|| "光照结果缺失".to_string())
                    .and_then(|value| value.as_ref().map_err(Clone::clone));
                let mesh_result = if !self.lighting.is_lit() {
                    if let Some(band) = gpu_bands.remove(&(pos, section_y)) {
                        self.build_gpu_cube_mesh(pos, band, &faces, None)
                    } else {
                        self.build_static_gpu_mesh(pos, section_y, &faces, None)
                    }
                } else if let Some(band) = gpu_bands.remove(&(pos, section_y)) {
                    match light {
                        Ok(light) => self.build_gpu_cube_mesh(pos, band, &faces, Some(light)),
                        Err(error) => Err(error.clone()),
                    }
                } else {
                    match light {
                        Ok(light) => self.build_static_gpu_mesh(
                            pos, section_y, &faces, Some(light),
                        ),
                        Err(error) => Err(error.clone()),
                    }
                };
                if trace {
                    texture_lookup_time += self.resolver_time;
                    texture_read_time += self.texture_read_time;
                    texture_decode_time += self.texture_decode_time;
                }
                match mesh_result {
                    Ok(Some(mesh)) => {
                        if let Some(sections) = completed_sections.get_mut(&pos) {
                            sections.push(section_y);
                        }
                        let mut mesh = Arc::new(mesh);
                        let key = (pos, section_y);
                        let previous = self
                            .revisit_cache
                            .get(&key)
                            .cloned()
                            .or_else(|| {
                                self.published_sections.get(&key).and_then(Weak::upgrade)
                            });
                        if let Some(old) = previous
                            .as_ref()
                            .filter(|old| same_chunk_render_geometry(old, &mesh))
                        {
                            // A revisit-cache hit can survive removal of the weak
                            // published index. Restore it for face accounting,
                            // partial re-meshes and future neighbour lookups.
                            self.published_sections.insert(key, Arc::downgrade(old));
                            reused_sections += 1;
                            retained_sections.entry(pos).or_default().push(old.clone());
                        } else {
                            if let Some(old) = previous.as_ref() {
                                // Occluder-only Section deltas must not retire
                                // or regenerate already resident GPU geometry.
                                reuse_section_gpu_mesh(old, &mut mesh);
                            }
                            self.published_sections.insert(key, Arc::downgrade(&mesh));
                            const REVISIT_CACHE_CAPACITY: usize = 1024;
                            if self.revisit_cache.insert(key, mesh.clone()).is_none() {
                                self.revisit_order.push_back(key);
                            }
                            while self.revisit_cache.len() > REVISIT_CACHE_CAPACITY {
                                let Some(oldest) = self.revisit_order.pop_front() else {
                                    break;
                                };
                                self.revisit_cache.remove(&oldest);
                            }
                            let payload = World3dChunkPayload {
                                lighting: self.lighting,
                                generation,
                                mesh,
                                probe,
                            };
                            let send_started = Instant::now();
                            let _ = tx.send(ChunkServiceEvent::Chunk(payload));
                            if trace {
                                send_wait_time += send_started.elapsed();
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        failed_sections.insert(pos);
                        self.failed_chunks = self.failed_chunks.saturating_add(1);
                        self.last_error = Some(error.clone());
                        let _ = tx.send(ChunkServiceEvent::Fatal { error });
                    }
                }
                if trace && build_started.elapsed() >= Duration::from_millis(250) {
                    eprintln!(
                        "world3d slow_section ts={} pos={},{} section_y={} faces={} build_ms={} tex_lookup_ms={} tex_read_ms={} tex_decode_ms={}",
                        trace_timestamp_ms(),
                        pos.x,
                        pos.z,
                        section_y,
                        faces.len(),
                        build_started.elapsed().as_millis(),
                        self.resolver_time.as_millis(),
                        self.texture_read_time.as_millis(),
                        self.texture_decode_time.as_millis(),
                    );
                }
            }
            convert_elapsed += convert_started.elapsed();
        }
        let mut completions = Vec::new();
        for pos in targets.iter().copied() {
            if failed_sections.contains(&pos) {
                continue;
            }
            if let Some(probe) = self.probe_stash.get(&pos).map(|entry| Arc::clone(&entry.0)) {
                if let Some(coarse) = completed_modes.get(&pos) {
                    self.lod_modes.insert(pos, *coarse);
                }
                self.retained_invalidated.remove(&pos);
                let sections = completed_sections.remove(&pos).unwrap_or_default();
                let faces = sections
                    .iter()
                    .filter_map(|band| {
                        self.published_sections
                            .get(&(pos, *band))
                            .and_then(Weak::upgrade)
                    })
                    .fold(0usize, |total, mesh| total.saturating_add(mesh.faces));
                if let Some(cached) = self.probe_stash.get_mut(&pos) {
                    cached.3 = faces;
                }
                forget_published_sections(&mut self.published_sections, pos, &sections);
                completions.push(World3dChunkCompletion {
                    generation,
                    lights: self
                        .light_fields
                        .get(&pos)
                        .map_or_else(|| Arc::from([]), |field| field.sources.clone()),
                    lighting: self.lighting,
                    pos,
                    sections,
                    retained_meshes: retained_sections.remove(&pos).unwrap_or_default(),
                    probe,
                });
            }
        }
        // Only successfully completed columns participate in boundary reconciliation.
        // Track unordered neighbour PAIRS rather than columns: a column can gain a
        // new neighbour long after it was first meshed, including after an unload.
        // Each pair is reconciled once until either column is invalidated or unloaded.
        let completed_now = completions
            .iter()
            .map(|completion| completion.pos)
            .collect::<HashSet<_>>();
        for pos in completed_now.iter().copied() {
            for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let neighbour = ChunkPos {
                    x: pos.x + dx,
                    z: pos.z + dz,
                    ..pos
                };
                // A retained mesh of an UNLOADED column must not count: the
                // strong revisit cache intentionally outlives active scenes.
                if !self.cache.contains_key(&neighbour) {
                    continue;
                }
                let completed_together = completed_now.contains(&neighbour);
                let neighbour_meshed = completed_together
                    || self
                        .published_sections
                        .range((neighbour, i32::MIN)..=(neighbour, i32::MAX))
                        .any(|(_, mesh)| mesh.strong_count() > 0);
                if neighbour_meshed
                    && record_boundary_pair(&mut self.boundary_pairs, pos, neighbour)
                    && !completed_together
                {
                    // The previously meshed neighbour may still have an exposed
                    // seam built while this column was absent.
                    self.dirty_meshes.insert(neighbour);
                }
            }
        }
        let _ = tx.send(ChunkServiceEvent::Completed(completions));
        self.last_face_ms = u64::try_from(face_elapsed.as_millis()).unwrap_or(u64::MAX);
        if trace_verbose() {
            eprintln!("world3d worker: face passes {pass_faces:?}");
        }
        for (pos, blocks) in builder.take_stream_chunks() {
            let Some((probe, version, state_version, faces, frame_displays)) =
                self.probe_stash.remove(&pos)
            else {
                continue;
            };
            let bytes = blocks.estimated_cpu_bytes();
            self.cache.insert(
                pos,
                CachedChunk {
                    blocks: Some(blocks),
                    boundary: None,
                    frame_displays,
                    probe,
                    bytes,
                    version,
                    state_version,
                    faces,
                },
            );
        }
        self.probe_stash.clear();
        self.promote_visible_bands();
        let positions = surrounded_chunk_candidates(loaded, |pos| self.cache.contains_key(&pos));
        for pos in positions {
            if let Some(chunk) = self.cache.get_mut(&pos) {
                if chunk.faces > 0 {
                    chunk.discard_blocks();
                }
            }
        }
        self.last_convert_ms = u64::try_from(convert_elapsed.as_millis()).unwrap_or(u64::MAX);
        self.last_mesh_targets = targets.len();
        self.last_reused_sections = reused_sections;
        self.last_boundary_hits = boundary_hits;
        self.last_rehydrate_ms = u64::try_from(rehydrate_time.as_millis()).unwrap_or(u64::MAX);
        self.last_send_wait_ms = u64::try_from(send_wait_time.as_millis()).unwrap_or(u64::MAX);
        self.last_texture_lookup_ms =
            u64::try_from(texture_lookup_time.as_millis()).unwrap_or(u64::MAX);
        self.last_texture_read_ms =
            u64::try_from(texture_read_time.as_millis()).unwrap_or(u64::MAX);
        self.last_texture_decode_ms =
            u64::try_from(texture_decode_time.as_millis()).unwrap_or(u64::MAX);
        if trace && !include_neighbors {
            eprintln!(
                "world3d remesh ts={} gen={} targets={} faces={} reused_sections={} boundary_hits={} total_ms={} rehydrate_ms={} face_ms={} convert_ms={} send_wait_ms={}",
                trace_timestamp_ms(),
                generation,
                targets.len(),
                face_count,
                reused_sections,
                boundary_hits,
                mesh_started.elapsed().as_millis(),
                rehydrate_time.as_millis(),
                face_elapsed.as_millis(),
                convert_elapsed.as_millis(),
                send_wait_time.as_millis(),
            );
        }
        if trace_verbose() {
            eprintln!(
                "world3d worker: mesh {} target chunks, {} faces, faces_ms={} convert_ms={} cache_chunks={}",
                targets.len(),
                face_count,
                face_elapsed.as_millis(),
                convert_elapsed.as_millis(),
                self.cache.len()
            );
        }
        self.face_material_cache.clear();
    }

    fn batch_load_options(&self, count: usize) -> ChunkLoadOptions {
        let (center_x, center_z) = self.view_center_chunk();
        let mut options = self.chunk_load_options();
        let workers = count.min(
            std::thread::available_parallelism()
                .map_or(4, |n| n.get())
                .min(8),
        );
        options.threading = WorldThreadingOptions::Fixed(workers);
        options.priority = ChunkLoadPriority::DistanceFrom {
            chunk_x: center_x,
            chunk_z: center_z,
        };
        options
    }

    fn load_batch(&mut self, positions: &[ChunkPos]) -> LoadedChunkBatch {
        load_batch_from_world(
            &self.world,
            &self.models,
            positions,
            self.batch_load_options(positions.len()),
            None,
        )
    }

    fn load_batch_lazy(&mut self, positions: &[ChunkPos]) -> LoadedChunkBatch {
        load_batch_from_world(
            &self.world,
            &self.models,
            positions,
            self.batch_load_options(positions.len()),
            Some(self.view),
        )
    }

    fn load_chunk(&mut self, pos: ChunkPos) -> Result<Option<CachedChunk>, String> {
        let started = Instant::now();
        let data = self
            .world
            .query_chunk_data(pos, self.chunk_load_options())
            .map_err(|error| format!("读取区块 {},{} 失败：{error}", pos.x, pos.z))?;
        let trace = trace_verbose();
        let biome_blend =
            p3d::Preview3dBiomeBlendField::from_chunks(std::slice::from_ref(&data));
        let chunk = collect_cached_chunk(data, self.models.as_ref(), trace, Some(&biome_blend))?;
        if trace {
            eprintln!(
                "world3d worker: load_chunk {},{} total_ms={}",
                pos.x,
                pos.z,
                started.elapsed().as_millis()
            );
        }
        Ok(chunk)
    }

    /// Detail-only Sections (stairs, plants, signs and arbitrary model faces)
    /// share the same GPU resident path as cube Sections. Decode materials and
    /// templates directly; do not build a CPU Mesh then unpack it into the
    /// same GPU templates. CPU fallback remains lazy in VoxelMeshSource.
    fn build_static_gpu_mesh(
        &mut self,
        pos: ChunkPos,
        section_y: i32,
        faces: &[p3d::Preview3dStreamFace],
        light: Option<&lighting::ChunkLighting>,
    ) -> Result<Option<World3dChunkMesh>, String> {
        let mut templates = Vec::with_capacity(faces.len());
        let Some(mut output) = self.build_chunk_mesh_with_templates(
            pos, section_y, faces, Some(&mut templates), 0,
        )? else {
            return Ok(None);
        };
        let source = if let Some(light) = light {
            light.static_source_faces(templates)?
        } else {
            gpui_3d::VoxelMeshSource::new(
                [3, 3, 3],
                {
                    let min = templates.iter()
                        .flat_map(|face| face.vertices.iter())
                        .fold(Vec3::new(f32::MAX, f32::MAX, f32::MAX), |min, vertex| {
                            min.min(vertex.position)
                        });
                    Vec3::new(
                        min.x.floor() - 1.0,
                        min.y.floor() - 1.0,
                        min.z.floor() - 1.0,
                    )
                },
                vec![0u32; 27],
                Vec::new(),
            )
            .and_then(|source| source.with_static_faces(templates))
            .map_err(|error| format!("GPU 静态模型输入无效: {error}"))?
        };
        let mesh = Mesh::from_gpu_voxels(source)
            .with_identity(chunk_mesh_id(pos, section_y), self.mesh_generation);
        output.faces = mesh.index_count() / 6;
        output.triangles = output.faces.saturating_mul(2);
        output.mesh = Arc::new(mesh);
        Ok(Some(output))
    }
    fn build_chunk_mesh_with_templates(
        &mut self,
        pos: ChunkPos,
        section_y: i32,
        faces: &[p3d::Preview3dStreamFace],
        mut gpu_templates: Option<&mut Vec<gpui_3d::VoxelFaceTemplate>>,
        template_local_faces: usize,
    ) -> Result<Option<World3dChunkMesh>, String> {
        if faces.is_empty() {
            return Ok(None);
        }
        let gpu_source = gpu_templates.is_some();
        if let Some(templates) = gpu_templates.as_deref_mut() {
            templates.clear();
            templates.reserve(faces.len());
        }
        let build_started = Instant::now();
        let origin = [pos.x * 16, 0, pos.z * 16];
        let mut material_map = HashMap::<MaterialSlotKey, usize>::new();
        let mut opaque_proxies = Vec::new();
        let mut materials = Vec::<Arc<Material>>::new();
        let mut bucket_map = HashMap::<(i32, usize), usize>::new();
        let mut buckets = Vec::<FaceBucket>::new();
        let mut bucket_material_slots = Vec::<usize>::new();
        let mut used_textures = BTreeMap::<gpui_3d::TextureAssetId, Arc<TextureAsset>>::new();
        let mut chunk_texture_lookup = HashMap::<(usize, [i32; 3]), FaceMaterialProperties>::new();
        let mut missing_texture_faces = 0usize;
        let trace = trace_verbose();
        let mut missing_materials = BTreeMap::<(&str, [i32; 3]), usize>::new();
        let mut texture_resolve_time = Duration::ZERO;
        self.resolver_time = Duration::ZERO;
        self.texture_read_time = Duration::ZERO;
        self.texture_decode_time = Duration::ZERO;
        self.resolve_chunk_texture_sources(faces);
        let origin_f64 = [
            f64::from(origin[0]),
            f64::from(origin[1]),
            f64::from(origin[2]),
        ];
        for (face_index, face) in faces.iter().enumerate() {
            let material_name = face.material.as_ref();
            let lookup_started = trace.then(Instant::now);
            let properties = chunk_texture_lookup
                .entry((face.material.as_ptr() as usize, face.normal))
                .or_insert_with(|| {
                    // Arc-backed string equality (not just a pointer key)
                    // permits reuse across different chunk face arrays.
                    if let Some(cached) = self
                        .face_material_cache
                        .get(&(Arc::clone(&face.material), face.normal))
                    {
                        return cached.clone();
                    }
                    let resolved = self.resolve_texture(material_name, face.normal);
                    let (mut resolved_texture, mut alpha_content, is_missing) = match resolved {
                        Some((texture, content)) => (Some(texture), content, false),
                        None => (None, TEXTURE_ALPHA_CONTENT_UNKNOWN, true),
                    };
                    // Bind the engine's red/black placeholder instead of an
                    // invisible solid color so unresolved textures stay visible
                    // (and counted) rather than rendering as black faces.
                    if is_missing
                        && let Some((missing, missing_content)) = self
                            .resolver
                            .missing_texture()
                            .and_then(|resolved| self.resolve_texture_path(resolved))
                    {
                        resolved_texture = Some(missing);
                        alpha_content = missing_content;
                    }
                    let [color, normal, mer] =
                        if self.lighting == super::rendering::LightingMode::VibrantPreview {
                            self.resolve_pbr(material_name, face.normal)
                        } else {
                            [None, None, None]
                        };
                    if let Some((color, content)) = color {
                        resolved_texture = Some(color);
                        alpha_content = content;
                    }
                    let tint_mask = material_uses_tint(material_name)
                        && resolved_texture
                            .as_ref()
                            .is_some_and(|texture| self.mesh_assets.is_grayscale_mask(texture));
                    let grass_side_sources =
                        grass_block_side_texture_keys(material_name).and_then(
                            |(dirt_key, overlay_key)| {
                                let (dirt, _) = self.resolve_texture_key(dirt_key)?;
                                let (overlay, _) = self.resolve_texture_key(overlay_key)?;
                                (dirt.dimensions() == overlay.dimensions())
                                    .then_some((dirt, overlay))
                            },
                        );
                    let result = FaceMaterialProperties {
                        texture: resolved_texture,
                        normal: normal.map(|(asset, _)| asset),
                        mer: mer.map(|(asset, _)| asset),
                        alpha_content,
                        is_missing,
                        tint_mask,
                        grass_side_sources,
                        opaque_glass: material_name.starts_with("minecraft_glass_"),
                    };
                    self.face_material_cache.insert(
                        (Arc::clone(&face.material), face.normal),
                        result.clone(),
                    );
                    result
                });
            let mut resolved_texture = properties.texture.clone();
            let mut uses_tint = properties.tint_mask;
            if face.normal[1] == 0 {
                let composited = properties
                    .grass_side_sources
                    .as_ref()
                    .and_then(|(dirt, overlay)| {
                        self.mesh_assets
                            .composited_grass_side(dirt, overlay, face.color)
                    });
                if let Some(composited) = composited {
                    resolved_texture = Some(composited);
                    uses_tint = false;
                } else if is_full_grass_block_side_material(material_name)
                    && let Some(tinted) = self
                        .mesh_assets
                        .tinted_grass_side(resolved_texture.as_ref(), face.color)
                {
                    resolved_texture = Some(tinted);
                    uses_tint = false;
                }
            }
            let texture = resolved_texture.as_ref();
            if let Some(started) = lookup_started {
                texture_resolve_time += started.elapsed();
            }
            if properties.is_missing {
                missing_texture_faces = missing_texture_faces.saturating_add(1);
                if trace {
                    *missing_materials
                        .entry((material_name, face.normal))
                        .or_default() += 1;
                }
            }
            if let Some(texture) = texture {
                used_textures
                    .entry(texture.id())
                    .or_insert_with(|| texture.clone());
            }
            for auxiliary in [&properties.normal, &properties.mer].into_iter().flatten() {
                used_textures
                    .entry(auxiliary.id())
                    .or_insert_with(|| auxiliary.clone());
            }
            let alpha_mode = alpha_mode_for_pass(face.pass, properties.alpha_content);
            if (!gpu_source || template_local_faces == 0)
                && alpha_mode == AlphaMode::Opaque
                && let Some(proxy) = occluders::face_proxy_with_template_origin(
                    face,
                    origin,
                    face_index < template_local_faces,
                )
            {
                opaque_proxies.push(proxy);
            }
            let base_color = if texture.is_none() {
                [
                    face.color[0].clamp(0.0, 1.0),
                    face.color[1].clamp(0.0, 1.0),
                    face.color[2].clamp(0.0, 1.0),
                ]
            } else {
                [1.0, 1.0, 1.0]
            };
            let key = MaterialSlotKey {
                water: face.pass == p3d::Preview3dStreamPass::Water,
                glass: alpha_mode == AlphaMode::Blend
                    && matches!(
                        face.pass,
                        p3d::Preview3dStreamPass::Glass | p3d::Preview3dStreamPass::DetailGlass
                    ),
                texture: texture.map(|texture| texture.id()),
                normal: properties.normal.as_ref().map(|asset| asset.id()),
                mer: properties.mer.as_ref().map(|asset| asset.id()),
                alpha_mode: alpha_mode as u8,
                double_sided: face_double_sided(face.pass),
                base_color_bits: base_color.map(f32::to_bits),
            };
            let slot = if let Some(index) = material_map.get(&key).copied() {
                index
            } else {
                let shared = self.mesh_assets.material(key, alpha_mode);
                #[cfg(test)]
                if std::env::var("BMCBL_WORLD3D_TEST_MATERIALS").as_deref() == Ok("1") {
                    let alpha = texture.map(|asset| {
                        let pixels =
                            &asset.pixels()[..asset.width() as usize * asset.height() as usize * 4];
                        let mut counts = [0usize; 3];
                        for pixel in pixels.chunks_exact(4) {
                            counts[match pixel[3] {
                                0 => 0,
                                255 => 2,
                                _ => 1,
                            }] += 1;
                        }
                        counts
                    });
                    eprintln!(
                        "world3d material {}",
                        serde_json::json!({
                            "name": material_name,
                            "pass": format!("{:?}", face.pass),
                            "alpha_mode": format!("{alpha_mode:?}"),
                            "alpha_content": format!("{:?}", properties.alpha_content),
                            "surface": format!("{:?}", shared.surface),
                            "roughness": shared.roughness,
                            "alpha_zero_partial_opaque": alpha,
                        })
                    );
                }
                materials.push(shared);
                let index = materials.len() - 1;
                material_map.insert(key, index);
                index
            };
            let color = face_vertex_color(
                face,
                texture.is_some(),
                uses_tint,
                properties.opaque_glass,
                properties.alpha_content,
            );
            if let Some(templates) = gpu_templates.as_deref_mut() {
                // Source faces are already in the template/instance/static order
                // provided by the streaming decoder. Keep that order and write
                // only the four materialized template corners. Do NOT construct
                // a full CPU Mesh, index buffer or packed vertex array only to
                // read its vertices back for the GPU compute mesher.
                let material_slot = u32::try_from(slot)
                    .map_err(|_| "区块材质过多".to_string())?;
                templates.push(gpui_3d::VoxelFaceTemplate {
                    vertices: world3d_face_vertices(
                        face, origin_f64, face_index < template_local_faces, color,
                    ),
                    material_slot,
                    neighbour: face.normal,
                });
            } else {
                let bucket_index = *bucket_map
                    .entry((face_band_y(face), slot))
                    .or_insert_with(|| {
                        let index = buckets.len();
                        buckets.push(FaceBucket::new());
                        bucket_material_slots.push(slot);
                        index
                    });
                buckets[bucket_index].faces.push((face_index, color));
            }
        }
        if gpu_source {
            // This placeholder is never published: build_gpu_cube_mesh replaces
            // it with Mesh::from_gpu_voxels (or the explicitly requested compact
            // fallback). The material and texture data already have final IDs.
            self.mesh_generation = self.mesh_generation.saturating_add(1);
            let empty = Mesh::from_voxel_quads_without_raycast(
                std::iter::empty::<[Vertex; 4]>(),
                Vec::<MeshPart>::new(),
            )
            .map_err(|error| format!("构造 GPU Section 元数据失败: {error}"))?
            .with_identity(chunk_mesh_id(pos, section_y), self.mesh_generation);
            return Ok(Some(World3dChunkMesh {
                // Detail-only GPU Sections retain the same opaque occluder
                // proxies as the legacy CPU Mesh path. Cube-grid Sections
                // replace these with optimized occluder boxes afterwards.
                occluders: occluders::merge_boxes(opaque_proxies),
                pos,
                section_y,
                origin,
                mesh: Arc::new(empty),
                materials: Arc::from(materials),
                textures: Arc::from(used_textures.into_values().collect::<Vec<_>>()),
                triangles: faces.len().saturating_mul(2),
                faces: faces.len(),
                missing_texture_faces,
            }));
        }
        if buckets.is_empty() {
            return Err("区块网格没有材质".to_string());
        }
        let mut parts = Vec::with_capacity(buckets.len());
        let mut first_index = 0_u32;
        for (bucket_index, bucket) in buckets.iter().enumerate() {
            if bucket.faces.is_empty() {
                continue;
            }
            let index_count = bucket
                .faces
                .len()
                .checked_mul(6)
                .and_then(|count| u32::try_from(count).ok())
                .ok_or_else(|| "区块网格过大".to_string())?;
            parts.push(MeshPart::new(
                first_index,
                index_count,
                u32::try_from(bucket_material_slots[bucket_index])
                    .map_err(|_| "区块材质过多".to_string())?,
            ));
            first_index = first_index
                .checked_add(index_count)
                .ok_or_else(|| "区块网格过大".to_string())?;
        }
        let quads = buckets.into_iter().flat_map(|bucket| {
            bucket.faces.into_iter().map(|(face_index, color)| {
                world3d_face_vertices(
                    &faces[face_index],
                    origin_f64,
                    face_index < template_local_faces,
                    color,
                )
            })
        });
        let bucket_time = build_started.elapsed();
        let part_count = parts.len();
        let pack_started = Instant::now();
        self.mesh_generation = self.mesh_generation.saturating_add(1);
        let mesh = Mesh::from_voxel_quads_without_raycast(quads, parts)
            .map_err(|error| format!("构造区块网格失败：{error}"))?
            .with_identity(chunk_mesh_id(pos, section_y), self.mesh_generation);
        let pack_time = pack_started.elapsed();
        if trace_timings() && build_started.elapsed() >= Duration::from_millis(250) {
            eprintln!(
                "world3d slow_build ts={} pos={},{} section_y={} faces={} parts={} bucket_ms={} pack_ms={} tex_lookup_ms={} tex_read_ms={} tex_decode_ms={}",
                trace_timestamp_ms(),
                pos.x,
                pos.z,
                section_y,
                faces.len(),
                part_count,
                bucket_time.as_millis(),
                pack_time.as_millis(),
                self.resolver_time.as_millis(),
                self.texture_read_time.as_millis(),
                self.texture_decode_time.as_millis(),
            );
        }
        let mesh = World3dChunkMesh {
            occluders: occluders::merge_boxes(opaque_proxies),
            pos,
            section_y,
            origin,
            mesh: Arc::new(mesh),
            materials: Arc::from(materials),
            textures: Arc::from(used_textures.into_values().collect::<Vec<_>>()),
            triangles: faces.len().saturating_mul(2),
            faces: faces.len(),
            missing_texture_faces,
        };
        if trace_verbose() {
            eprintln!(
                "world3d worker: chunk {},{} texture resolve {}ms (lookup {}ms read {}ms decode {}ms), missing {:?}",
                pos.x,
                pos.z,
                texture_resolve_time.as_millis(),
                self.resolver_time.as_millis(),
                self.texture_read_time.as_millis(),
                self.texture_decode_time.as_millis(),
                missing_materials
            );
        }
        Ok(Some(mesh))
    }

    fn resolve_chunk_texture_sources(&mut self, faces: &[p3d::Preview3dStreamFace]) {
        let unresolved = unresolved_material_sources(faces, &self.resolved_texture_lookup);
        if unresolved.len() < 4 {
            return;
        }
        let workers = std::thread::available_parallelism()
            .map_or(4, |count| count.get())
            .min(4);
        let item_count = unresolved
            .iter()
            .filter(|(material, _)| material.starts_with(ITEM_MATERIAL_PREFIX))
            .count();
        let started = Instant::now();
        let resolved = unresolved
            .par_iter()
            .with_min_len(unresolved.len().div_ceil(workers))
            .map(|(material, normal)| resolve_material_source(&self.resolver, material, *normal))
            .collect::<Vec<_>>();
        let elapsed = started.elapsed();
        if trace_timings() && elapsed >= Duration::from_millis(50) {
            eprintln!(
                "world3d source_lookup ts={} count={} items={} ms={}",
                trace_timestamp_ms(),
                unresolved.len(),
                item_count,
                elapsed.as_millis(),
            );
        }
        self.resolver_time += elapsed;
        self.resolved_texture_lookup
            .extend(unresolved.into_iter().zip(resolved));
    }

    fn resolve_texture(
        &mut self,
        material: &str,
        normal: [i32; 3],
    ) -> Option<(Arc<TextureAsset>, TextureAlphaContent)> {
        let lookup_key = (material.to_string(), normal);
        if let Some(cached) = self.texture_lookup.get(&lookup_key) {
            match cached {
                Some((texture, content)) => {
                    if let Some(texture) = texture.upgrade() {
                        return Some((texture, *content));
                    }
                }
                None => return None,
            }
        }
        let result = if let Some(id) = material.strip_prefix(MAP_MATERIAL_PREFIX) {
            self.resolve_map_texture(id)
        } else {
            let resolved =
                cached_texture_source(&mut self.resolved_texture_lookup, &lookup_key, || {
                    let lookup_started = trace_timings().then(Instant::now);
                    let resolved = resolve_material_source(&self.resolver, material, normal);
                    if let Some(started) = lookup_started {
                        self.resolver_time += started.elapsed();
                    }
                    if trace_verbose() {
                        if let Some(resolved) = resolved.as_ref() {
                            eprintln!(
                                "world3d texture {material} {:?} -> {} pack={}",
                                normal,
                                resolved.relative_path,
                                resolved.pack_root.display()
                            );
                        }
                    }
                    resolved
                });
            resolved.and_then(|resolved| self.resolve_texture_path(resolved))
        };
        self.texture_lookup.insert(
            lookup_key,
            result
                .as_ref()
                .map(|(texture, content)| (Arc::downgrade(texture), *content)),
        );
        result
    }

    /// Resolves a `terrain_texture.json` key, for companion textures such as grass side masks.
    fn resolve_texture_key(
        &mut self,
        key: &str,
    ) -> Option<(Arc<TextureAsset>, TextureAlphaContent)> {
        let resolved = self.resolver.texture_for_key(key)?;
        self.resolve_texture_path(resolved)
    }

    fn resolve_map_texture(
        &mut self,
        id: &str,
    ) -> Option<(Arc<TextureAsset>, TextureAlphaContent)> {
        let cache_key = format!("{MAP_MATERIAL_PREFIX}{id}");
        if let Some(cached) = self.textures_by_path.get(&cache_key) {
            match cached {
                Some(texture) => {
                    if let Some(texture) = texture.upgrade() {
                        let content = self
                            .texture_alpha_content
                            .get(&cache_key)
                            .copied()
                            .unwrap_or(TEXTURE_ALPHA_CONTENT_UNKNOWN);
                        return Some((texture, content));
                    }
                }
                None => return None,
            }
        }
        let id = MapItemId::new(id.to_owned()).ok()?;
        let map = self.world.map_item(&id).ok()??;
        let pixels = map.pixels?;
        if pixels.width == 0
            || pixels.height == 0
            || pixels.width > TEXTURE_MAX_DIMENSION
            || pixels.height > TEXTURE_MAX_DIMENSION
        {
            return None;
        }
        let image = RgbaImage::from_raw(pixels.width, pixels.height, pixels.rgba)?;
        let content = texture_alpha_content(&image);
        let asset = self.mesh_assets.intern_texture(mipmapped_texture(image)?);
        self.textures_by_path
            .insert(cache_key.clone(), Some(Arc::downgrade(&asset)));
        self.texture_alpha_content.insert(cache_key, content);
        Some((asset, content))
    }

    fn resolve_texture_path(
        &mut self,
        resolved: bedrock_block_model::ObjResolvedTexture,
    ) -> Option<(Arc<TextureAsset>, TextureAlphaContent)> {
        self.resolve_texture_path_with(resolved, false)
    }

    fn resolve_texture_path_with(
        &mut self,
        resolved: bedrock_block_model::ObjResolvedTexture,
        is_data: bool,
    ) -> Option<(Arc<TextureAsset>, TextureAlphaContent)> {
        let path = resolved.source_path.path().to_string_lossy();
        // A file used as both color and data must not share alpha-weighted mips.
        let cache_key = if is_data {
            format!("data:{path}")
        } else {
            path.into_owned()
        };
        if let Some(cached) = self.textures_by_path.get(&cache_key) {
            match cached {
                Some(texture) => {
                    if let Some(texture) = texture.upgrade() {
                        let content = self
                            .texture_alpha_content
                            .get(&cache_key)
                            .copied()
                            .unwrap_or(TEXTURE_ALPHA_CONTENT_UNKNOWN);
                        return Some((texture, content));
                    }
                }
                None => return None,
            }
        }
        let read_started = trace_timings().then(Instant::now);
        let bytes = match resolved.source_path.read() {
            Ok(bytes) => bytes,
            Err(error) => {
                self.last_error = Some(format!("读取贴图 {cache_key} 失败：{error}"));
                self.textures_by_path.insert(cache_key, None);
                return None;
            }
        };
        if let Some(started) = read_started {
            self.texture_read_time += started.elapsed();
        }
        let decode_started = trace_timings().then(Instant::now);
        // decode_texture_image retries color-mapped TGAs whose headers over-declare
        // palette alpha bits (vanilla water normal maps), which the TGA decoder rejects.
        let decoded = {
            let is_tga = resolved
                .source_path
                .path()
                .extension()
                .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("tga"));
            bedrock_block_model::decode_texture_image(&bytes, is_tga)
        };
        let image = match decoded {
            Ok(image) => image.to_rgba8(),
            Err(error) => {
                self.last_error = Some(format!("解码贴图 {cache_key} 失败：{error}"));
                self.textures_by_path.insert(cache_key, None);
                return None;
            }
        };
        if let Some(started) = decode_started {
            self.texture_decode_time += started.elapsed();
        }
        // Flipbook textures are vertical animation strips (for example 16x512 water). Sampling
        // the whole strip as one texture displays every frame at once; keep the first frame.
        let image = if self.resolver.is_flipbook_texture(&resolved.relative_path) {
            let (width, height) = image.dimensions();
            if height > width && height % width == 0 {
                image::imageops::crop_imm(&image, 0, 0, width, width).to_image()
            } else {
                image
            }
        } else {
            image
        };
        let (width, height) = image.dimensions();
        if width == 0
            || height == 0
            || width > TEXTURE_MAX_DIMENSION
            || height > TEXTURE_MAX_DIMENSION
        {
            self.textures_by_path.insert(cache_key, None);
            return None;
        }
        let content = texture_alpha_content(&image);
        let asset = self.mesh_assets.intern_texture(if is_data {
            mipmapped_data_texture(image)
        } else {
            mipmapped_texture(image)
        }?);
        self.textures_by_path
            .insert(cache_key.clone(), Some(Arc::downgrade(&asset)));
        self.texture_alpha_content.insert(cache_key, content);
        Some((asset, content))
    }

    fn ensure_cached(&mut self, pos: ChunkPos) -> Result<Option<&CachedChunk>, String> {
        if self.missing.contains(&pos) {
            return Ok(None);
        }
        if !self.cache.contains_key(&pos) {
            if let Some(chunk) = self.load_chunk(pos)? {
                self.cache.insert(pos, chunk);
                self.missing.remove(&pos);
            } else {
                self.missing.insert(pos);
            }
        }
        Ok(self.cache.get(&pos))
    }

    fn probe_surface(&mut self, positions: &[(i32, i32)]) -> Vec<Option<i32>> {
        let mut heights = Vec::with_capacity(positions.len());
        for &(block_x, block_z) in positions {
            let chunk = ChunkPos {
                x: block_x.div_euclid(16),
                z: block_z.div_euclid(16),
                dimension: self.view.dimension,
            };
            let top = self.ensure_cached(chunk).ok().flatten().map(|cached| {
                let (min_y, max_y) = chunk.y_range(cached.version);
                let mut fluid_top = None;
                for y in (min_y..=max_y).rev() {
                    if let Some(block) = cached.probe.block_at(block_x, y, block_z) {
                        if block.fluid {
                            fluid_top.get_or_insert(y);
                            continue;
                        }
                        return Some(y);
                    }
                }
                fluid_top
            });
            heights.push(top.flatten());
        }
        heights
    }

    fn invalidate(&mut self, chunks: &[ChunkPos]) {
        for pos in chunks {
            forget_boundary_pairs(&mut self.boundary_pairs, *pos);
            if self.cache.remove(pos).is_some() {
                self.retained_invalidated.insert(*pos);
            }
            self.missing.remove(pos);
            self.failed_positions.remove(pos);
            self.probe_stash.remove(pos);
            self.light_columns.remove(pos);
            self.light_fields.remove(pos);
            self.deferred_columns.remove(pos);
            self.deferred_bands.retain(|(column, _)| column != pos);
            self.urgent_chunks.insert(*pos);
        }
    }
}

pub(super) fn inside_view(
    pos: ChunkPos,
    dimension: Dimension,
    center_x: i32,
    center_z: i32,
    radius: i32,
) -> bool {
    pos.dimension == dimension
        && super::chunk_range::contains(
            i64::from(pos.x) - i64::from(center_x),
            i64::from(pos.z) - i64::from(center_z),
            radius,
        )
}

fn surrounded_chunk_candidates(
    loaded: &[ChunkPos],
    contains: impl Fn(ChunkPos) -> bool,
) -> Vec<ChunkPos> {
    let mut candidates = BTreeSet::new();
    for pos in loaded {
        for dx in -2_i32..=2 {
            for dz in (-2_i32..=2).filter(|dz| dx.abs() + dz.abs() <= 2) {
                candidates.insert(ChunkPos {
                    x: pos.x + dx,
                    z: pos.z + dz,
                    dimension: pos.dimension,
                });
            }
        }
    }
    candidates
        .into_iter()
        .filter(|pos| {
            contains(*pos)
                && (-2_i32..=2).all(|dx| {
                    (-2_i32..=2)
                        .filter(|dz| dx.abs() + dz.abs() <= 2)
                        .all(|dz| {
                            contains(ChunkPos {
                                x: pos.x + dx,
                                z: pos.z + dz,
                                dimension: pos.dimension,
                            })
                        })
                })
        })
        .collect()
}

fn bounds_for_positions(
    dimension: Dimension,
    positions: impl Iterator<Item = ChunkPos>,
) -> Option<SlimeChunkBounds> {
    let mut min_chunk_x = i32::MAX;
    let mut max_chunk_x = i32::MIN;
    let mut min_chunk_z = i32::MAX;
    let mut max_chunk_z = i32::MIN;
    let mut any = false;
    for pos in positions {
        if pos.dimension != dimension {
            continue;
        }
        any = true;
        min_chunk_x = min_chunk_x.min(pos.x);
        max_chunk_x = max_chunk_x.max(pos.x);
        min_chunk_z = min_chunk_z.min(pos.z);
        max_chunk_z = max_chunk_z.max(pos.z);
    }
    any.then_some(SlimeChunkBounds {
        dimension,
        min_chunk_x,
        max_chunk_x,
        min_chunk_z,
        max_chunk_z,
    })
}

fn material_uses_tint(material: &str) -> bool {
    [
        "grass",
        "leaves",
        "fern",
        "vine",
        "kelp",
        "seagrass",
        "lily",
        "foliage",
        "redstone_wire",
    ]
    .iter()
    .any(|needle| material.contains(needle))
}

/// Whether a face material is a full grass block side.
///
/// Those carried side textures already bake the biome tint into their green overlay. Grass-named
/// plant materials (`tallgrass`, `seagrass`, ferns) are grayscale masks and keep the vertex tint.
fn is_full_grass_block_side_material(material: &str) -> bool {
    let base = material
        .split_once(MATERIAL_SLOT_SEPARATOR)
        .map_or(material, |(base, _)| base);
    matches!(
        obj_material_texture_name(base).as_str(),
        "grass_side" | "grass_block_side" | "grass_side_snowed" | "grass_block_side_snowed"
    )
}

/// Companion texture keys for rebuilding a grass block side from resource-pack layers.
///
/// Vanilla composites `dirt` + `grass_side` (grayscale overlay, alpha) * biome tint; the
/// carried side texture is the pre-tinted inventory variant and is too bright for the world.
fn grass_block_side_texture_keys(material: &str) -> Option<(&'static str, &'static str)> {
    let base = material
        .split_once(MATERIAL_SLOT_SEPARATOR)
        .map_or(material, |(base, _)| base);
    match obj_material_texture_name(base).as_str() {
        "grass_side" | "grass_block_side" => Some(("dirt", "grass_side")),
        _ => None,
    }
}

fn face_vertex_color(
    face: &p3d::Preview3dStreamFace,
    has_texture: bool,
    uses_tint: bool,
    opaque_glass: bool,
    alpha_content: TextureAlphaContent,
) -> [f32; 4] {
    let tinted = has_texture && (uses_tint || matches!(face.pass, p3d::Preview3dStreamPass::Water));
    let (r, g, b) = if tinted {
        (
            face.color[0].clamp(0.0, 1.0),
            face.color[1].clamp(0.0, 1.0),
            face.color[2].clamp(0.0, 1.0),
        )
    } else {
        (1.0, 1.0, 1.0)
    };
    let shade = face.shade.clamp(0.0, 1.0);
    let alpha = match face.pass {
        p3d::Preview3dStreamPass::Glass | p3d::Preview3dStreamPass::DetailGlass if opaque_glass => {
            1.0
        }
        p3d::Preview3dStreamPass::Glass | p3d::Preview3dStreamPass::DetailGlass => {
            // Textures with hard 0/255 alpha (vanilla glass) render as cutout;
            // only genuinely translucent textures keep the vertex alpha.
            match alpha_content {
                TextureAlphaContent::Binary | TextureAlphaContent::Opaque => 1.0,
                TextureAlphaContent::Partial | TextureAlphaContent::Unknown => {
                    face.color[3].clamp(0.08, 1.0)
                }
            }
        }
        p3d::Preview3dStreamPass::Water => face.color[3].clamp(0.08, 1.0),
        _ => 1.0,
    };
    [
        (r * shade).clamp(0.0, 1.0),
        (g * shade).clamp(0.0, 1.0),
        (b * shade).clamp(0.0, 1.0),
        alpha,
    ]
}

fn probe_flags_for_state(state: &BlockState) -> u8 {
    let name = state.name.as_str();
    if matches!(
        name,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    ) {
        return 0;
    }
    let mut flags = PROBE_FLAG_SOLID;
    if matches!(
        name,
        "minecraft:water" | "minecraft:flowing_water" | "minecraft:lava" | "minecraft:flowing_lava"
    ) {
        flags |= PROBE_FLAG_FLUID;
    }
    flags
}

fn build_probe(
    pos: ChunkPos,
    data: ChunkData,
    state_version: Option<i32>,
    solid_masks: BTreeMap<i8, [u64; 64]>,
) -> World3dChunkProbe {
    let mut subchunks = BTreeMap::new();
    let (min_y, _) = pos.y_range(data.version);
    for (y, subchunk) in data.subchunks {
        let mut flags = Vec::new();
        if let SubChunkFormat::Paletted { storages, .. } = &subchunk.format {
            for storage in storages {
                let mut storage_flags = Vec::with_capacity(storage.states.len());
                for state in &storage.states {
                    storage_flags.push(probe_flags_for_state(state));
                }
                flags.push(storage_flags);
            }
        }
        subchunks.insert(y, ProbingSubChunk { subchunk, flags });
    }
    // Certified full-cube cells were collected with the rendered block model,
    // not inferred from probe's generic "non-air" flag (which includes fluids,
    // glass, fences and other non-occluding geometry).
    let visibility = solid_masks
        .into_iter()
        .filter(|(_, solid)| solid.iter().any(|&word| word != 0))
        .map(|(y, solid)| {
            (y, Arc::new(super::visibility::SectionVisibility::from_solid_bits(solid)))
        })
        .collect::<BTreeMap<_, _>>();
    let storage_bytes = subchunks.values().fold(
        std::mem::size_of::<World3dChunkProbe>(),
        |bytes, subchunk| {
            bytes
                .saturating_add(std::mem::size_of::<i8>())
                .saturating_add(subchunk.estimated_cpu_bytes())
        },
    ).saturating_add(visibility.len().saturating_mul(
        std::mem::size_of::<i8>()
            + std::mem::size_of::<super::visibility::SectionVisibility>(),
    )).saturating_add(visibility.values().fold(0usize, |total, graph| {
        total.saturating_add(graph.heap_bytes())
    }));
    World3dChunkProbe {
        pos,
        min_y,
        subchunks,
        visibility,
        state_version,
        storage_bytes,
    }
}

/// Match the full BSC game version to a selected installation's version
/// directory, e.g. 1.26.51.1 to versions/26.51. Do not share the database
/// across unrelated versions: network IDs and native UV layouts may change.
fn capture_version_matches_installation(version: &str, package_roots: &[PathBuf]) -> bool {
    let version_tag = version.strip_prefix("1.").unwrap_or(version);
    package_roots.iter().any(|package| {
        package.ancestors().take(5).any(|part| {
            let Some(folder) = part.file_name().and_then(|s| s.to_str()) else {
                return false;
            };
            version_tag == folder
                || version_tag
                    .strip_prefix(folder)
                    .is_some_and(|tail| tail.starts_with('.'))
        })
    })
}

/// Discover the selected installation's exported model catalog. External
/// captures are always preferred over the optional compiled-in model bundle.
fn discover_capture_library(package_roots: &[PathBuf]) -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("BMCBL_CAPTURE_MODEL_DIR")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
    {
        return root.join("manifest.json").is_file().then_some(root);
    }
    // Ignore exports from unrelated installed game versions. A known
    // package root is the only installation identity available to this worker.
    let mut matches = std::collections::BTreeSet::<PathBuf>::new();
    for package in package_roots {
        for base in package.ancestors().take(5) {
            for rel in [
                "BedrockSceneCapture/export",
                "mods/BedrockSceneCapture/export",
                "plugins/BedrockSceneCapture/export",
            ] {
                let Ok(entries) = std::fs::read_dir(base.join(rel)) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let dir = entry.path();
                    if !dir.join("manifest.json").is_file() || !dir.join("blocks.json").is_file() {
                        continue;
                    }
                    let Some(version) = dir.file_name().and_then(|name| name.to_str()) else {
                        continue;
                    };
                    let version_tag = version.strip_prefix("1.").unwrap_or(version);
                    let selected = package.ancestors().take(5).any(|part| {
                        let Some(folder) = part.file_name().and_then(|s| s.to_str()) else {
                            return false;
                        };
                        // BMCBL version directories may be named 26.51 while the
                        // game's full version string is 1.26.51.1.
                        version_tag == folder
                            || version_tag
                                .strip_prefix(folder)
                                .is_some_and(|tail| tail.starts_with('.'))
                    });
                    if selected {
                        matches.insert(dir);
                    }
                }
            }
        }
    }
    // Multiple distinct matches are ambiguous. Require explicit selection
    // rather than rendering the wrong release's directional geometry.
    (matches.len() == 1)
        .then(|| matches.into_iter().next())
        .flatten()
}
fn build_pack_roots(_world_path: &Path, package_roots: &[PathBuf]) -> Vec<PathBuf> {
    vanilla_resource_pack_roots_from_packages(package_roots.iter())
}

/// Metadata-first decode for far rear columns: no block model expansion,
/// biome shading, fluid-face analysis, texture resolution or GPU mesh work.
/// Once the camera points toward this column, the normal mesher rehydrates it
/// through the batched world query and replaces this provisional probe.
fn collect_lazy_probe_chunk(data: ChunkData) -> Result<Option<CachedChunk>, String> {
    if !data.is_loaded {
        return Ok(None);
    }
    let pos = data.pos;
    let version = data.version;
    let probe = Arc::new(build_probe(pos, data, None, BTreeMap::new()));
    Ok(Some(CachedChunk {
        blocks: None,
        boundary: None,
        frame_displays: Vec::new(),
        probe,
        bytes: 0,
        version,
        state_version: None,
        faces: 0,
    }))
}

fn collect_cached_chunk(
    data: ChunkData,
    models: &bedrock_block_model::BlockModelRepository,
    trace: bool,
    biome_blend: Option<&p3d::Preview3dBiomeBlendField>,
) -> Result<Option<CachedChunk>, String> {
    if !data.is_loaded {
        return Ok(None);
    }
    let pos = data.pos;
    let started = Instant::now();
    let frame_displays = FrameDisplay::collect(&data);
    let blocks =
        p3d::preview_3d_collect_chunk_blocks_with_biome_blend(&data, Some(models), biome_blend)
            .map_err(|error| format!("收集区块 {},{} 方块失败：{error}", pos.x, pos.z))?;
    let bytes = blocks.estimated_cpu_bytes();
    let version = data.version;
    let probe = Arc::new(build_probe(pos, data, None, blocks.visibility_solid_masks()));
    if trace {
        eprintln!(
            "world3d worker: collect chunk {},{} bytes={} in {}ms",
            pos.x,
            pos.z,
            bytes,
            started.elapsed().as_millis()
        );
    }
    Ok(Some(CachedChunk {
        blocks: Some(blocks),
        boundary: None,
        frame_displays,
        probe,
        bytes,
        version,
        state_version: None,
        faces: 0,
    }))
}

/// Convert either a world-space face or a GPU cube's local template to a
/// chunk-relative vertex. The offset order matches the former face-clone path.
/// Shared CPU packed-mesh fallback and GPU source-template vertex semantics.
#[inline]
fn world3d_face_vertices(
    face: &p3d::Preview3dStreamFace,
    origin: [f64; 3],
    local_template: bool,
    color: [f32; 4],
) -> [Vertex; 4] {
    let normal = Vec3::new(face.normal[0] as f32, face.normal[1] as f32, face.normal[2] as f32);
    let uv = face.uv.unwrap_or([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
    std::array::from_fn(|corner| Vertex {
        position: chunk_mesh_corner(face.corners[corner], origin, local_template),
        normal,
        uv: Vec2::new(uv[corner][0], uv[corner][1]),
        color: std::array::from_fn(|channel| {
            if channel < 3 { color[channel] * face.ambient_light[corner] }
            else { color[channel] }
        }),
    })
}

#[inline]
fn chunk_mesh_corner(corner: [f64; 3], origin: [f64; 3], local_template: bool) -> Vec3 {
    let dx = if local_template { origin[0] } else { 0. };
    let dz = if local_template { origin[2] } else { 0. };
    Vec3::new(
        (corner[0] + dx - origin[0]) as f32,
        (corner[1] - origin[1]) as f32,
        (corner[2] + dz - origin[2]) as f32,
    )
}

fn intern_face_materials(faces: &mut [p3d::Preview3dStreamFace]) {
    let mut names = HashMap::<Arc<str>, Arc<str>>::new();
    for face in faces {
        if let Some(shared) = names.get(&face.material) {
            face.material = shared.clone();
        } else {
            names.insert(face.material.clone(), face.material.clone());
        }
    }
}

#[derive(Clone)]
struct FaceMaterialProperties {
    normal: Option<Arc<TextureAsset>>,
    mer: Option<Arc<TextureAsset>>,
    texture: Option<Arc<TextureAsset>>,
    alpha_content: TextureAlphaContent,
    /// The face failed texture resolution and is showing the missing-texture
    /// placeholder instead; still counted and traceable.
    is_missing: bool,
    /// The material is biome-tinted and its texture is the grayscale mask, so the face keeps
    /// the voxel vertex color. Pre-tinted resource-pack textures render without a second tint.
    tint_mask: bool,
    /// Dirt and grass-overlay textures used to rebuild a grass block side per biome.
    grass_side_sources: Option<(Arc<TextureAsset>, Arc<TextureAsset>)>,
    opaque_glass: bool,
}

/// Alpha content of a decoded texture, used to pick the alpha pipeline from
/// real pixel data instead of guessing from the block name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextureAlphaContent {
    /// Every pixel is fully opaque; the block can render in the opaque pass.
    Opaque,
    /// Alpha values are only 0 or 255 (vanilla glass, leaves, plants); render
    /// as alpha-tested cutout instead of translucent blend.
    Binary,
    /// Intermediate alpha values exist (ice, slime, stained variants); keep
    /// the translucent blend pipeline.
    Partial,
    /// The texture could not be resolved; use the model render pass as fallback.
    Unknown,
}

const TEXTURE_ALPHA_CONTENT_UNKNOWN: TextureAlphaContent = TextureAlphaContent::Unknown;

fn texture_alpha_content(image: &RgbaImage) -> TextureAlphaContent {
    let mut transparent = false;
    for pixel in image.pixels() {
        match pixel.0[3] {
            255 => {}
            0 => transparent = true,
            _ => return TextureAlphaContent::Partial,
        }
    }
    if transparent {
        TextureAlphaContent::Binary
    } else {
        TextureAlphaContent::Opaque
    }
}

/// The alpha pipeline follows the texture's real pixels: binary-alpha
/// textures cut out, partially translucent ones blend, fully opaque ones
/// render opaque. When pixels are unavailable, only a transmissive model pass
/// selects blending; missing ordinary textures must not turn blocks into glass.
fn alpha_mode_for_pass(
    pass: p3d::Preview3dStreamPass,
    alpha_content: TextureAlphaContent,
) -> AlphaMode {
    use p3d::Preview3dStreamPass as Pass;
    match alpha_content {
        TextureAlphaContent::Opaque => match pass {
            Pass::Water => AlphaMode::Blend,
            _ => AlphaMode::Opaque,
        },
        TextureAlphaContent::Binary => match pass {
            Pass::Water => AlphaMode::Blend,
            _ => AlphaMode::Mask,
        },
        TextureAlphaContent::Partial => AlphaMode::Blend,
        TextureAlphaContent::Unknown => match pass {
            Pass::Water | Pass::Glass | Pass::DetailGlass => AlphaMode::Blend,
            _ => AlphaMode::Opaque,
        },
    }
}

fn face_double_sided(pass: p3d::Preview3dStreamPass) -> bool {
    !matches!(pass, p3d::Preview3dStreamPass::Opaque)
}

fn face_inside_block(face: &p3d::Preview3dStreamFace, pos: [i32; 3]) -> bool {
    const EPSILON: f64 = 0.001;
    face.corners.iter().all(|corner| {
        corner[0] >= f64::from(pos[0]) - EPSILON
            && corner[0] <= f64::from(pos[0]) + 1.0 + EPSILON
            && corner[1] >= f64::from(pos[1]) - EPSILON
            && corner[1] <= f64::from(pos[1]) + 1.0 + EPSILON
            && corner[2] >= f64::from(pos[2]) - EPSILON
            && corner[2] <= f64::from(pos[2]) + 1.0 + EPSILON
    })
}

fn face_band_y(face: &p3d::Preview3dStreamFace) -> i32 {
    let sum: f64 = face.corners.iter().map(|corner| corner[1]).sum();
    (sum / 4.0 / MESH_BAND_HEIGHT).floor() as i32
}

fn chunk_mesh_id(pos: ChunkPos, section_y: i32) -> MeshId {
    let dimension = match pos.dimension {
        Dimension::Overworld => 0_u64,
        Dimension::Nether => 1,
        Dimension::End => 2,
        Dimension::Unknown(value) => value as u64,
    };
    let mut hash = CHUNK_MESH_ID_SALT;
    for value in [pos.x as u64, pos.z as u64, dimension, section_y as u64] {
        hash ^= value.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        hash = hash.rotate_left(27).wrapping_mul(0x94D0_49BB_1331_11EB);
    }
    MeshId(hash)
}

fn run_worker(
    config: ChunkServiceConfig,
    commands: Receiver<ChunkServiceCommand>,
    events: SyncSender<ChunkServiceEvent>,
    pending_views: Arc<Mutex<PendingViews>>,
) {
    let mut options = OpenOptions::default();
    options.read_only = true;
    let world = match World::open(&config.world_path, options) {
        Ok(world) => world,
        Err(error) => {
            let _ = events.send(ChunkServiceEvent::Fatal {
                error: format!("打开世界失败：{error}"),
            });
            return;
        }
    };
    let mut worker = Worker::new(&config, world);
    worker.info.water_textures = worker.load_water_textures();
    worker.info.cloud_texture = worker
        .resolver
        .environment_texture("textures/environment/clouds")
        .or_else(|| worker.resolver.environment_texture("textures/clouds"))
        .and_then(|source| {
            worker
                .resolve_texture_path(source)
                .map(|(texture, _)| texture)
        })
        .and_then(|texture| {
            match TextureAsset::rgba8_mip_chain(
                texture.width(),
                texture.height(),
                (0..texture.mip_level_count()).filter_map(|level| texture.mip_pixels(level)),
            ) {
                Ok(texture) => Some(Arc::new(texture)),
                Err(error) => {
                    worker.last_error = Some(format!("准备云材质失败：{error}"));
                    None
                }
            }
        });
    let _ = events.send(ChunkServiceEvent::Ready(worker.info.clone()));
    worker.publish_status(&events);

    let mut pending: Option<ChunkServiceCommand> = None;
    loop {
        let command = match pending.take() {
            Some(command) => command,
            None => match commands.recv() {
                Ok(command) => command,
                Err(_) => break,
            },
        };
        let command = if matches!(command, ChunkServiceCommand::ViewUpdated) {
            let Some(view) = pending_views
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .views
                .pop_front()
            else {
                continue;
            };
            view
        } else {
            command
        };
        match command {
            ChunkServiceCommand::SetLighting(lighting) => {
                if worker.lighting != lighting {
                    worker.lighting = lighting;
                    worker.light_fields.clear();
                    worker.published_sections.clear();
                    worker.boundary_pairs.clear();
                    worker.revisit_cache.clear();
                    worker.revisit_order.clear();
                    worker.dirty_meshes.extend(worker.cache.keys().copied());
                    while pending.is_none() {
                        match worker.stream(&events, &commands) {
                            Some(command) => pending = Some(command),
                            None => break,
                        }
                    }
                }
            }
            ChunkServiceCommand::ViewUpdated => continue,
            ChunkServiceCommand::Shutdown => break,
            ChunkServiceCommand::SetView {
                generation,
                dimension,
                camera,
                forward_xz,
                forward_y,
                radius,
                view_distance,
            } => {
                let old_center = worker.view_center_chunk();
                let old_radius = worker.view.radius;
                let new_center = (
                    (camera[0] / 16.0).floor() as i32,
                    (camera[2] / 16.0).floor() as i32,
                );
                if old_center != new_center
                    || worker.view.dimension != dimension
                    || worker.view.radius != radius
                {
                    worker.cursor = ChunkCursor::default();
                } else if worker.view.forward_xz != forward_xz {
                    // Position did not change, but a fast turn can make the
                    // rest of the current ring's old camera-first ordering
                    // irrelevant. Reprioritize without repeating DB work.
                    worker.cursor.reprioritize_remaining(forward_xz);
                }
                if worker.view.dimension != dimension {
                    worker.published_sections.clear();
                    worker.boundary_pairs.clear();
                    worker.revisit_cache.clear();
                    worker.revisit_order.clear();
                    for pos in std::mem::take(&mut worker.cache).into_keys() {
                        let _ = events.send(ChunkServiceEvent::Unloaded { generation, pos });
                    }
                    worker.dirty_meshes.clear();
                    worker.deferred_columns.clear();
                    worker.deferred_bands.clear();
                    worker.lod_modes.clear();
                    worker.urgent_chunks.clear();
                    for pos in std::mem::take(&mut worker.retained_invalidated) {
                        let _ = events.send(ChunkServiceEvent::Unloaded { generation, pos });
                    }
                    worker.missing.clear();
                    worker.failed_positions.clear();
                }
                worker.view = ViewState {
                    generation,
                    dimension,
                    camera,
                    forward_xz,
                    forward_y,
                    radius: radius.clamp(1, WORLD3D_MAX_VIEW_RADIUS),
                    view_distance,
                };
                // Turned toward metadata-only columns? Promote them to the
                // regular dirty queue. The batch mesher rehydrates and builds
                // their faces without reloading columns still behind us.
                let newly_visible = worker
                    .deferred_columns
                    .iter()
                    .copied()
                    .filter(|pos| !lazy_mesh_column(*pos, worker.view))
                    .collect::<Vec<_>>();
                for pos in newly_visible {
                    worker.deferred_columns.remove(&pos);
                    worker.dirty_meshes.insert(pos);
                }
                worker.promote_visible_bands();
                if old_center != new_center || old_radius != worker.view.radius {
                    worker.failed_positions.clear();
                }
                if old_center != new_center || old_radius != worker.view.radius {
                    for pos in worker.cache.keys().copied() {
                        if inside_view(
                            pos,
                            dimension,
                            new_center.0,
                            new_center.1,
                            worker.view.radius,
                        ) && let Some(&was_coarse) = worker.lod_modes.get(&pos)
                            && coarse_lod_for_view(pos, worker.view, was_coarse) != was_coarse
                        {
                            worker.dirty_meshes.insert(pos);
                        }
                    }
                }
                if trace_timings() {
                    eprintln!(
                        "world3d view ts={} gen={} center={},{} radius={}",
                        trace_timestamp_ms(),
                        generation,
                        new_center.0,
                        new_center.1,
                        worker.view.radius,
                    );
                }
                while pending.is_none() {
                    match worker.stream(&events, &commands) {
                        Some(command) => pending = Some(command),
                        None => break,
                    }
                }
            }
            ChunkServiceCommand::ProbeSurface {
                request_id,
                dimension,
                positions,
            } => {
                let previous_dimension = worker.view.dimension;
                if previous_dimension != dimension {
                    worker.view.dimension = dimension;
                }
                let heights = worker.probe_surface(&positions);
                worker.view.dimension = previous_dimension;
                let _ = events.send(ChunkServiceEvent::Surface {
                    request_id,
                    heights,
                });
            }
            ChunkServiceCommand::Invalidate { chunks, .. } => {
                worker.invalidate(&chunks);
                while pending.is_none() {
                    match worker.stream(&events, &commands) {
                        Some(command) => pending = Some(command),
                        None => break,
                    }
                }
            }
        }
    }
    worker.resolver.flush_material_index();
}


#[cfg(test)]
mod local_gpu_template_tests {
    use super::chunk_mesh_corner;

    #[test]
    fn template_corners_match_translated_world_corners() {
        let local = [0.25, 32.5, 1.75];
        for origin in [[16., 0., 32.], [-512., 0., -64.], [0., 0., 0.]] {
            let world = [local[0] + origin[0], local[1], local[2] + origin[2]];
            assert_eq!(
                chunk_mesh_corner(local, origin, true),
                chunk_mesh_corner(world, origin, false),
            );
        }
    }
}
