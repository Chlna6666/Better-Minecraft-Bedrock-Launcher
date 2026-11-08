//! The independent 3D world editor window.
//!
//! This window never reuses or mutates the read-only 3D preview panel state. It owns its
//! camera, focus, input capture, streaming worker, chunk meshes and GPU scene snapshot.

use super::camera::{World3dCamera, World3dNavigation};
use super::chunk_service::{ChunkServiceConfig, ChunkServiceHandle, spawn_chunk_service};
use super::import::{World3dImportAction, World3dImportPreview};
use super::registry::{
    World3dRegistryEntry, register_world3d_editor, set_world3d_editor_window,
    unregister_world3d_editor,
};
use super::types::{
    ChunkServiceEvent, ChunkServiceInfo, ChunkServiceStatus, TeleportHeightMode, WORLD3D_BRUSHES,
    WORLD3D_MAX_EDIT_BLOCKS, WORLD3D_MAX_VIEW_RADIUS, World3dBlockEditKind,
    World3dBlockEditRequest, World3dCameraSnapshot, World3dChunkMesh, World3dChunkProbe,
    World3dEditOutcome, World3dProjectionKind, World3dSelection, World3dTargetBlock, World3dTool,
    World3dWorldKey, WorldNavigationTarget, dimension_label,
};
use crate::ui::components::slider::Slider;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::ui::views::settings::state::SettingsPageState;
use crate::ui::window::map_viewer::model::MapViewerWindowView;
use bedrock_render::{ChunkPos, Dimension};
use gpui::StatefulInteractiveElement as _;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_3d::{
    Aabb, AlphaMode, Material, Mesh, Node, NodeId, Scene, SceneView, ShadingModel, TextureAsset,
    TextureSampling, Transform, Vec3,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, mpsc::{self, Receiver, TryRecvError}};
use std::time::{Duration, Instant};
mod rendering;
pub(in super::super) mod scene_registry;

const TOP_BAR_HEIGHT: f32 = 46.0;
const STATUS_BAR_HEIGHT: f32 = 26.0;
const LEFT_RAIL_WIDTH: f32 = 62.0;
const RIGHT_PANEL_WIDTH: f32 = 320.0;
const MIN_WINDOW_WIDTH: f32 = 960.0;
const MIN_WINDOW_HEIGHT: f32 = 620.0;
const VIEW_UPDATE_INTERVAL: Duration = Duration::from_millis(120);
const SNAPSHOT_PUBLISH_INTERVAL: Duration = Duration::from_millis(100);
const NAVIGATION_UI_INTERVAL: Duration = Duration::from_millis(100);
const POLL_INTERVAL: Duration = Duration::from_millis(16);
const PICK_INTERVAL: Duration = Duration::from_micros(33_334);
const MAX_MESH_EVENTS_PER_POLL: usize = 16;
const MESH_SUBMIT_BUDGET_BYTES: usize = 8 * 1024 * 1024;
const STREAM_SCENE_INTERVAL: Duration = Duration::from_millis(50);
const SECTION_EPOCH_MAX_TOMBSTONES: usize = 32_768;
/// Recent connected cave/terrain regions; memory is additionally bounded by
/// the sum of retained hidden Section keys, not just the entry count.
const PORTAL_RESULT_CACHE_ENTRIES: usize = 8;
const PORTAL_RESULT_CACHE_BANDS: usize = 16_384;

/// Streamed Section payloads and their completion typically reuse one
/// immutable Arc<Probe>. Only a new identity can change the portal graph;
/// repeated payloads must not cancel asynchronous culling every poll tick.
fn upsert_shared_probe<K: Ord, V>(
    cache: &mut BTreeMap<K, Arc<V>>,
    key: K,
    probe: Arc<V>,
) -> bool {
    match cache.entry(key) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(probe);
            true
        }
        std::collections::btree_map::Entry::Occupied(mut entry) => {
            if Arc::ptr_eq(entry.get(), &probe) {
                false
            } else {
                entry.insert(probe);
                true
            }
        }
    }
}

#[cfg(test)]
mod stream_probe_epoch_tests {
    use super::*;

    #[test]
    fn redundant_section_events_do_not_invalidate_shared_portal_probe() {
        let mut cache = BTreeMap::<u32, Arc<u32>>::new();
        let original = Arc::new(7);
        assert!(upsert_shared_probe(&mut cache, 1, Arc::clone(&original)));
        assert!(!upsert_shared_probe(&mut cache, 1, Arc::clone(&original)));
        assert!(!upsert_shared_probe(&mut cache, 1, Arc::clone(&original)));
        assert!(upsert_shared_probe(&mut cache, 1, Arc::new(7)));
        assert!(upsert_shared_probe(&mut cache, 2, original));
        assert_eq!(cache.len(), 2);
    }
}

/// Preserve Section upsert/remove ordering across queued background results.
/// The worker's generation is a view epoch, not a content hash: older results
/// are still useful until a newer event for the SAME chunk column arrives.
fn accept_section_epoch(
    latest: &mut HashMap<ChunkPos, u64>,
    pos: ChunkPos,
    generation: u64,
) -> bool {
    match latest.entry(pos) {
        std::collections::hash_map::Entry::Occupied(mut entry) => {
            if generation < *entry.get() {
                return false;
            }
            *entry.get_mut() = generation;
        }
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(generation);
        }
    }
    true
}


fn is_pick_due(last_pick_at: Option<Instant>, now: Instant) -> bool {
    last_pick_at.is_none_or(|at| now.saturating_duration_since(at) >= PICK_INTERVAL)
}

fn camera_snapshot_due(last_publish_at: Option<Instant>, now: Instant) -> bool {
    last_publish_at.is_none_or(|at| now.saturating_duration_since(at) >= SNAPSHOT_PUBLISH_INTERVAL)
}

fn stream_event_updates_ui(event: &ChunkServiceEvent) -> bool {
    matches!(
        event,
        ChunkServiceEvent::Ready(_)
            | ChunkServiceEvent::Status(_)
            | ChunkServiceEvent::Surface { .. }
            | ChunkServiceEvent::Fatal { .. }
    )
}

fn scene_publish_required(
    previous: Option<&Arc<SceneView>>,
    scene: &Arc<Scene>,
    textures_changed: bool,
) -> bool {
    textures_changed || previous.is_none_or(|view| !Arc::ptr_eq(view.scene(), scene))
}

#[cfg(test)]
mod stream_event_tests {
    use super::{ChunkServiceEvent, ChunkServiceStatus, stream_event_updates_ui};

    #[test]
    fn chunk_events_wait_for_scene_publish_before_ui_refresh() {
        assert!(!stream_event_updates_ui(&ChunkServiceEvent::Completed(
            Vec::new()
        )));
        assert!(stream_event_updates_ui(&ChunkServiceEvent::Status(
            ChunkServiceStatus::default()
        )));
    }
}

#[cfg(test)]
mod scene_publish_tests {
    use super::{Arc, Scene, SceneView, Vec3, scene_publish_required};
    use gpui_3d::Camera;

    #[test]
    fn unchanged_scene_and_textures_do_not_publish() {
        let scene = Arc::new(Scene::new());
        let camera = Camera::orthographic(Vec3::ZERO, -Vec3::Z, Vec3::Y, 4.0, 0.1, 10.0);
        let view = Arc::new(SceneView::new(scene.clone(), camera));
        assert!(!scene_publish_required(Some(&view), &scene, false));
        assert!(scene_publish_required(Some(&view), &scene, true));
        assert!(scene_publish_required(
            Some(&view),
            &Arc::new(Scene::new()),
            false
        ));
    }
}

#[cfg(test)]
mod camera_snapshot_tests {
    use super::{Duration, Instant, camera_snapshot_due};

    #[test]
    fn map_overlay_snapshot_is_limited_to_ten_updates_per_second() {
        let now = Instant::now();
        assert!(camera_snapshot_due(None, now));
        assert!(!camera_snapshot_due(
            Some(now),
            now + Duration::from_millis(99)
        ));
        assert!(camera_snapshot_due(
            Some(now),
            now + Duration::from_millis(100)
        ));
    }
}


#[cfg(test)]
mod section_epoch_tests {
    use super::accept_section_epoch;
    use bedrock_render::{ChunkPos, Dimension};
    use std::collections::HashMap;

    #[test]
    fn older_upsert_and_completion_cannot_override_newer_unload() {
        let pos = ChunkPos { x: 4, z: -9, dimension: Dimension::Overworld };
        let mut epochs = HashMap::new();
        assert!(accept_section_epoch(&mut epochs, pos, 7));
        assert!(!accept_section_epoch(&mut epochs, pos, 6));
        assert_eq!(epochs[&pos], 7);
        assert!(accept_section_epoch(&mut epochs, pos, 8));
        assert!(!accept_section_epoch(&mut epochs, pos, 7));
        // Same-epoch remeshing and completions are valid, ordered FIFO.
        assert!(accept_section_epoch(&mut epochs, pos, 8));
        assert!(accept_section_epoch(&mut epochs, pos, 9));
        assert_eq!(epochs[&pos], 9);
    }

    #[test]
    fn newer_event_for_other_column_does_not_reject_useful_older_section() {
        let a = ChunkPos { x: 0, z: 0, dimension: Dimension::Overworld };
        let b = ChunkPos { x: 1, z: 0, dimension: Dimension::Overworld };
        let mut epochs = HashMap::new();
        assert!(accept_section_epoch(&mut epochs, b, 25));
        assert!(accept_section_epoch(&mut epochs, a, 24));
        assert_eq!(epochs[&a], 24);
        assert_eq!(epochs[&b], 25);
    }
}
struct GpuSubmitStats {
    submitted: usize,
    deferred: usize,
    gated: usize,
    coalesced: u64,
    interval_p95_us: u64,
    p95_job: Option<GpuOwnerJobSample>,
    p95_by_kind: [Option<GpuOwnerJobSample>; 3],
}

fn gpu_submit_stats(samples: &[GpuOwnerJobSample], sample_age_ms: Option<u64>) -> GpuSubmitStats {
    let mut stats = GpuSubmitStats {
        submitted: 0,
        deferred: 0,
        gated: 0,
        coalesced: 0,
        interval_p95_us: 0,
        p95_job: None,
        p95_by_kind: [None, None, None],
    };
    let Some(age_ms) = sample_age_ms.filter(|age| *age < 1_000) else {
        return stats;
    };
    let latest_us = samples.last().map_or(0, |sample| sample.completed_at_us);
    let cutoff_us = latest_us.saturating_sub(1_000_000 - age_ms * 1_000);
    let mut intervals = Vec::new();
    let mut recent_jobs = Vec::new();
    let mut jobs_by_kind: [Vec<&GpuOwnerJobSample>; 3] = std::array::from_fn(|_| Vec::new());
    let mut previous_submit_us = None;
    for sample in samples
        .iter()
        .filter(|sample| sample.completed_at_us >= cutoff_us)
    {
        recent_jobs.push(sample);
        match sample.kind {
            GpuOwnerJobKind::Draw => jobs_by_kind[0].push(sample),
            GpuOwnerJobKind::Continue => jobs_by_kind[1].push(sample),
            GpuOwnerJobKind::Tick => jobs_by_kind[2].push(sample),
            _ => {}
        }
        stats.coalesced = stats.coalesced.saturating_add(sample.coalesced_count);
        match sample.outcome {
            GpuOwnerJobOutcome::Submitted => {
                stats.submitted += 1;
                if let Some(previous_us) = previous_submit_us.replace(sample.completed_at_us) {
                    intervals.push(sample.completed_at_us.saturating_sub(previous_us));
                }
            }
            GpuOwnerJobOutcome::Deferred => stats.deferred += 1,
            GpuOwnerJobOutcome::Control if sample.kind == GpuOwnerJobKind::Tick => stats.gated += 1,
            _ => {}
        }
    }
    intervals.sort_unstable();
    if !intervals.is_empty() {
        stats.interval_p95_us = intervals[(intervals.len() * 95).div_ceil(100) - 1];
    }
    recent_jobs.sort_unstable_by_key(|sample| sample.owner_job_duration_us);
    if !recent_jobs.is_empty() {
        stats.p95_job = Some((*recent_jobs[(recent_jobs.len() * 95).div_ceil(100) - 1]).clone());
    }
    for (index, jobs) in jobs_by_kind.iter_mut().enumerate() {
        jobs.sort_unstable_by_key(|sample| sample.owner_job_duration_us);
        if !jobs.is_empty() {
            stats.p95_by_kind[index] = Some((*jobs[(jobs.len() * 95).div_ceil(100) - 1]).clone());
        }
    }
    stats
}

fn owner_stage_ranking(phases: gpui::GpuOwnerJobPhases) -> [(&'static str, u64); 22] {
    let mut stages = [
        ("其他", phases.other_us),
        ("就绪", phases.readiness_us),
        ("原生节奏", phases.native_pacing_us),
        ("后端锁", phases.backend_lock_us),
        ("Fence轮询", phases.fence_poll_us),
        ("Fence等待", phases.fence_wait_us),
        ("尺寸", phases.surface_resize_us),
        ("准备其余", phases.render_prepare_us),
        ("场景打包", phases.pack_scene_us),
        ("扩展准备", phases.extension_prepare_us),
        ("3D准备其余", phases.extension_scene_prepare_us),
        ("3D相机剔除", phases.extension_camera_cull_us),
        ("3D资源同步", phases.extension_resource_sync_us),
        ("3D绘制分组", phases.extension_batch_build_us),
        ("3D缓冲编码", phases.extension_buffer_encode_us),
        ("3D命令编码", phases.extension_command_encode_us),
        ("绘制呈现", phases.draw_present_us),
        ("后端提交", phases.backend_submit_us),
        ("发布", phases.publication_us),
        ("诊断", phases.diagnostics_us),
        ("日志", phases.log_output_us),
        ("回报", phases.report_us),
    ];
    stages.sort_unstable_by_key(|stage| std::cmp::Reverse(stage.1));
    stages
}

fn owner_phase_summary(sample: &GpuOwnerJobSample) -> String {
    let phases = sample.phases;
    let stages = owner_stage_ranking(phases);
    format!(
        "Owner近1秒 p95 {:?}/{:?} {:.1} ms · 主耗时 {} {:.1} / {} {:.1} / {} {:.1} ms · Fence等待 {:.1} ms · 阻塞总计 {:.1} ms",
        sample.kind,
        sample.outcome,
        sample.owner_job_duration_us as f64 / 1000.0,
        stages[0].0,
        stages[0].1 as f64 / 1000.0,
        stages[1].0,
        stages[1].1 as f64 / 1000.0,
        stages[2].0,
        stages[2].1 as f64 / 1000.0,
        phases.fence_wait_us as f64 / 1000.0,
        sample.owner_blocking_wait_us as f64 / 1000.0,
    )
}

fn owner_path_summary(sample: &GpuOwnerJobSample) -> String {
    let top = owner_stage_ranking(sample.phases)[0];
    format!(
        "{:?} {:.0}ms({} {:.0})",
        sample.kind,
        sample.owner_job_duration_us as f64 / 1000.0,
        top.0,
        top.1 as f64 / 1000.0,
    )
}
const RENDER_ORIGIN_GRID: f64 = 512.0;

mod gizmo;
mod overlay;
mod raycast;

/// Everything the independent 3D editor needs when its window is created.
pub struct World3dEditorInit {
    pub world_key: World3dWorldKey,
    pub world_path: PathBuf,
    pub world_name: String,
    pub dimension: Dimension,
    pub spawn: Option<[f64; 3]>,
    pub spawn_source: String,
    pub package_roots: Vec<PathBuf>,
    pub owner: Option<WeakEntity<MapViewerWindowView>>,
    pub initial_target: Option<WorldNavigationTarget>,
    pub initial_selection: Option<World3dSelection>,
    pub view_radius: i32,
}

#[derive(Clone, Copy, Debug)]
struct SelectionDrag {
    anchor: [i32; 3],
    last: [i32; 3],
    previous: Option<World3dSelection>,
}

#[derive(Clone, Copy, Debug)]
struct LookDrag {
    last: Point<Pixels>,
}

#[derive(Clone, Debug)]
enum StatusKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug)]
struct FrameCounter {
    started_at: Instant,
    frames: u32,
    fps: f32,
    last_frame_ms: f32,
    last_frame_at: Option<Instant>,
    average_frame_ms: f32,
    frame_ms_accum: f32,
    frame_ms_samples: u32,
}

impl FrameCounter {
    fn new() -> Self {
        Self {
            started_at: Instant::now(),
            frames: 0,
            fps: 0.0,
            last_frame_ms: 0.0,
            last_frame_at: None,
            average_frame_ms: 0.0,
            frame_ms_accum: 0.0,
            frame_ms_samples: 0,
        }
    }

    fn record(&mut self, now: Instant) {
        self.frames = self.frames.saturating_add(1);
        if let Some(previous) = self.last_frame_at {
            let ms = (now - previous).as_secs_f32() * 1000.0;
            if ms.is_finite() && (0.0..2000.0).contains(&ms) {
                self.last_frame_ms = ms;
                self.frame_ms_accum += ms;
                self.frame_ms_samples += 1;
            }
        }
        self.last_frame_at = Some(now);
        let elapsed = now - self.started_at;
        if elapsed >= Duration::from_millis(500) {
            self.fps = self.frames as f32 / elapsed.as_secs_f32().max(0.001);
            self.frames = 0;
            self.started_at = now;
            if self.frame_ms_samples > 0 {
                self.average_frame_ms = self.frame_ms_accum / self.frame_ms_samples as f32;
                self.frame_ms_accum = 0.0;
                self.frame_ms_samples = 0;
            }
        }
    }
}

/// Immutable section graph source; cache one snapshot per chunk-stream
/// revision. Background tasks borrow only Arc-owned decoded terrain, never
/// the GPUI window, native renderer or mutable Scene registry.
struct PortalVisibilitySnapshot {
    probes: BTreeMap<ChunkPos, Arc<World3dChunkProbe>>,
    // Graph topology is independent of the currently uploaded GPU sections.
    // Include every decoded prospective band so later mesh publications can
    // reuse the exact visibility result without another whole-world BFS.
    bands: BTreeSet<(ChunkPos, i32)>,
}

fn portal_candidate_bands(
    probes: &BTreeMap<ChunkPos, Arc<World3dChunkProbe>>,
    resident: impl IntoIterator<Item = (ChunkPos, i32)>,
) -> BTreeSet<(ChunkPos, i32)> {
    let mut bands = resident.into_iter().collect::<BTreeSet<_>>();
    for (&pos, probe) in probes {
        extend_portal_probe_bands(&mut bands, pos, probe.subchunks.keys().copied());
    }
    bands
}

/// A 64-block render band spans four 16-block decoded SubChunks. This tiny
/// helper can be verified independently of constructing a full LevelDB probe.
fn extend_portal_probe_bands(
    bands: &mut BTreeSet<(ChunkPos, i32)>,
    pos: ChunkPos,
    subchunks: impl IntoIterator<Item = i8>,
) {
    bands.extend(subchunks.into_iter().map(|y| (pos, i32::from(y).div_euclid(4))));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PortalVisibilityKey {
    revision: u64,
    dimension: Dimension,
    voxel: [i32; 3],
    radius: i32,
    /// Two voxels in the same 16^3 air component share a portal result.
    component: Option<(ChunkPos, i32, u16, u8)>,
}

/// Camera movement within an already evaluated connected air component
/// preserves the graph result. Stream revisions and radius changes do not.
/// The camera may move while a portal graph task runs. A completed
/// result from another air pocket is still a valid LRU candidate provided
/// the decoded world snapshot, dimension and radius have not changed.
fn portal_snapshot_compatible(a: PortalVisibilityKey, b: PortalVisibilityKey) -> bool {
    a.revision == b.revision
        && a.dimension == b.dimension
        && a.radius == b.radius
}

fn same_portal_region(a: PortalVisibilityKey, b: PortalVisibilityKey) -> bool {
    a.revision == b.revision
        && a.dimension == b.dimension
        && a.radius == b.radius
        && a.component == b.component
}

#[cfg(test)]
mod async_portal_visibility_tests {
    use super::*;

    fn request(revision: u64, x: i32, exits: u8) -> PortalVisibilityKey {
        PortalVisibilityKey {
            revision,
            dimension: Dimension::Overworld,
            voxel: [x, 80, 0],
            radius: 12,
            component: Some((
                ChunkPos { x: 0, z: 0, dimension: Dimension::Overworld },
                5,
                1,
                exits,
            )),
        }
    }

    #[test]
    fn prospective_portal_bands_cover_not_yet_uploaded_sections() {
        let pos = ChunkPos { x: 1, z: 2, dimension: Dimension::Overworld };
        let mut bands = BTreeSet::from([(pos, 4)]);
        super::extend_portal_probe_bands(&mut bands, pos, [0, 7, -1]);
        assert!(bands.contains(&(pos, 0)));
        assert!(bands.contains(&(pos, 1)));
        assert!(bands.contains(&(pos, -1)));
        assert!(bands.contains(&(pos, 4)));
    }

    #[test]
    fn stale_camera_results_are_cacheable_only_with_the_same_world_snapshot() {
        let old = request(4, 1, 2);
        let moved = request(4, 7, 3);
        assert!(!same_portal_region(old, moved));
        assert!(portal_snapshot_compatible(old, moved));
        assert!(!portal_snapshot_compatible(old, request(5, 1, 2)));
        assert!(!portal_snapshot_compatible(old, PortalVisibilityKey {
            radius: 18, ..old
        }));
        assert!(!portal_snapshot_compatible(old, PortalVisibilityKey {
            dimension: Dimension::Nether, ..old
        }));
    }

    #[test]
    fn camera_movement_inside_one_air_pocket_reuses_async_visibility() {
        let a = request(4, 1, 0b001011);
        let b = request(4, 6, 0b001011);
        assert!(same_portal_region(a, b));
        assert_ne!(a.voxel, b.voxel);
    }

    #[test]
    fn completed_regions_reuse_exact_arc_and_respect_lru_bound() {
        let mut cache = PortalResultCache::default();
        let pos = ChunkPos { x: 0, z: 0, dimension: Dimension::Overworld };
        let first = request(4, 1, 1);
        let hidden = Arc::new(BTreeSet::from([(pos, 2)]));
        cache.insert(first, Arc::clone(&hidden));
        let nearby = request(4, 7, 1);
        let hit = cache.find(nearby).expect("same air region must reuse cached graph");
        assert!(Arc::ptr_eq(&hit, &hidden));
        assert!(cache.find(request(5, 1, 1)).is_none());
        assert!(cache.find(request(4, 1, 2)).is_none());

        for exits in 2..=11 {
            cache.insert(
                request(4, 1, exits),
                Arc::new(BTreeSet::from([(pos, i32::from(exits))])),
            );
        }
        assert!(cache.entries.len() <= PORTAL_RESULT_CACHE_ENTRIES);
        assert_eq!(
            cache.total_bands,
            cache.entries.iter().map(|(_, hidden)| hidden.len()).sum(),
        );
        assert!(cache.find(first).is_none(), "oldest region must be evicted");
        cache.clear();
        assert!(cache.entries.is_empty());
        assert_eq!(cache.total_bands, 0);
    }

    #[test]
    fn changing_portal_revision_exits_dimension_or_radius_invalidates_result() {
        let key = request(4, 1, 0b001011);
        assert!(!same_portal_region(key, request(5, 1, 0b001011)));
        assert!(!same_portal_region(key, request(4, 1, 0b111111)));
        assert!(!same_portal_region(key, PortalVisibilityKey {
            component: key.component.map(|(pos, y, _, exits)| (pos, y, 2, exits)),
            ..key
        }), "two isolated caves with identical exits must invalidate async cache");
        assert!(!same_portal_region(key, PortalVisibilityKey {
            dimension: Dimension::Nether, ..key
        }));
        assert!(!same_portal_region(key, PortalVisibilityKey {
            radius: 13, ..key
        }));
        assert!(!same_portal_region(key, PortalVisibilityKey {
            component: None, ..key
        }));
    }
}

/// LRU of successfully solved regions. A return to an explored cave can
/// republish its mask without another Rayon traversal, provided the decoded
/// graph revision, radius and exact air-component identity all match.
#[derive(Default)]
struct PortalResultCache {
    entries: VecDeque<(PortalVisibilityKey, Arc<BTreeSet<(ChunkPos, i32)>>)>,
    total_bands: usize,
}

impl PortalResultCache {
    fn find(&mut self, key: PortalVisibilityKey) -> Option<Arc<BTreeSet<(ChunkPos, i32)>>> {
        let at = self.entries.iter().position(|(old, _)| same_portal_region(*old, key))?;
        let (_, hidden) = self.entries.remove(at)?;
        let result = Arc::clone(&hidden);
        self.entries.push_front((key, hidden));
        Some(result)
    }

    fn insert(&mut self, key: PortalVisibilityKey, hidden: Arc<BTreeSet<(ChunkPos, i32)>>) {
        if let Some(at) = self.entries.iter().position(|(old, _)| same_portal_region(*old, key)) {
            if let Some((_, removed)) = self.entries.remove(at) {
                self.total_bands = self.total_bands.saturating_sub(removed.len());
            }
        }
        if hidden.len() > PORTAL_RESULT_CACHE_BANDS {
            return;
        }
        self.total_bands += hidden.len();
        self.entries.push_front((key, hidden));
        while self.entries.len() > PORTAL_RESULT_CACHE_ENTRIES
            || self.total_bands > PORTAL_RESULT_CACHE_BANDS
        {
            if let Some((_, retired)) = self.entries.pop_back() {
                self.total_bands -= retired.len();
            }
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.total_bands = 0;
    }
}

struct PortalVisibilityResult {
    key: PortalVisibilityKey,
    hidden: BTreeSet<(ChunkPos, i32)>,
    solve_us: u128,
}

/// The independent 3D world editor window view.
pub struct World3dEditorView {
    session_id: u64,
    world_key: World3dWorldKey,
    world_path: PathBuf,
    world_name: String,
    spawn: Option<[f64; 3]>,
    spawn_source: String,
    package_roots: Vec<PathBuf>,
    owner: Option<WeakEntity<MapViewerWindowView>>,
    window_handle: Option<AnyWindowHandle>,
    dimension: Dimension,
    camera: World3dCamera,
    navigation: World3dNavigation,
    saved_cameras: BTreeMap<Dimension, World3dCamera>,
    focus: FocusHandle,
    look_drag: Option<LookDrag>,
    selection_drag: Option<SelectionDrag>,
    axis_drag: Option<gizmo::AxisDrag>,
    last_frame_at: Option<Instant>,
    last_navigation_ui_at: Option<Instant>,
    navigation_ui_dirty: bool,
    service: Option<ChunkServiceHandle>,
    poll_task: Option<Task<anyhow::Result<()>>>,
    stream_generation: u64,
    last_view_key: Option<(Dimension, i32, i32, i32, i32)>,
    /// Only cross the worker boundary when camera direction has materially
    /// changed; the render thread samples small rotations independently.
    last_view_forward: Option<[f32; 3]>,
    last_view_camera: Option<[f64; 3]>,
    last_view_sent_at: Option<Instant>,
    worker_status: ChunkServiceStatus,
    worker_info: ChunkServiceInfo,
    chunk_lights: BTreeMap<ChunkPos, Arc<[super::types::World3dLocalLight]>>,
    dirty_light_chunks: BTreeSet<ChunkPos>,
    chunk_meshes: BTreeMap<(ChunkPos, i32), Arc<World3dChunkMesh>>,
    /// Per-column epoch ledger, including recent unload tombstones. Prevents
    /// queued stale Section payloads from resurrecting GPU-resident geometry.
    section_epochs: HashMap<ChunkPos, u64>,
    dirty_sections: BTreeSet<(ChunkPos, i32)>,
    probes: BTreeMap<ChunkPos, Arc<World3dChunkProbe>>,
    /// Monotonic decoded-portal snapshot revision, independent of mesh generation.
    visibility_revision: u64,
    /// Versioned snapshot is rebuilt only when streamed probes or resident
    /// Section keys change, not for camera movement within a loaded region.
    visibility_snapshot: Option<(u64, Arc<PortalVisibilitySnapshot>)>,
    visibility_job: Option<Receiver<PortalVisibilityResult>>,
    visibility_job_key: Option<PortalVisibilityKey>,
    visibility_completed_key: Option<PortalVisibilityKey>,
    visibility_cache: PortalResultCache,
    visibility_signature_voxel: Option<(u64, [i32; 3])>,
    visibility_signature: Option<(ChunkPos, i32, u16, u8)>,
    hidden_draw_nodes: Arc<HashSet<NodeId>>,
    /// Mesh render bands unreachable from the camera or any unknown exterior.
    hidden_sections: Arc<BTreeSet<(ChunkPos, i32)>>,
    /// Edited columns whose old portal data must not suppress geometry.
    invalidated_visibility: HashSet<ChunkPos>,
    textures: HashMap<gpui_3d::TextureAssetId, Arc<TextureAsset>>,
    texture_bytes: usize,
    texture_prune_pending: bool,
    texture_table_dirty: bool,
    scene_view: Option<Arc<SceneView>>,
    scene_nodes: scene_registry::ChunkScene,
    scene_has_preview: bool,
    render_settings: super::rendering::RenderSettings,
    last_light_tick: Option<Instant>,
    last_pick_at: Option<Instant>,
    target_scene_view: Option<Arc<SceneView>>,
    target_outline_mesh: Option<Arc<Mesh>>,
    target_outline_material: Arc<Material>,
    scene_dirty: bool,
    scene_rebuild_required: bool,
    scene_retry_at: Option<Instant>,
    stream_scene_pending: bool,
    last_scene_build_at: Option<Instant>,
    import_preview: Option<World3dImportPreview>,
    import_generation: u64,
    render_origin: [i32; 3],
    stage_size: (f32, f32),
    tool: World3dTool,
    brush_index: usize,
    selection: Option<World3dSelection>,
    target: Option<World3dTargetBlock>,
    pointer_position: Option<Point<Pixels>>,
    last_pick_camera: Option<World3dCameraSnapshot>,
    target_pick_dirty: bool,
    pointer_pick_dirty: bool,
    hud_debug: bool,
    hud_visible: bool,
    chunk_clipboard_count: usize,
    right_panel_open: bool,
    left_rail_open: bool,
    status: SharedString,
    status_kind: StatusKind,
    last_publish_at: Option<Instant>,
    last_published: Option<World3dCameraSnapshot>,
    nav_generation: u64,
    pending_surface: Option<u64>,
    edit_request_seq: u64,
    edit_busy: bool,
    frame: FrameCounter,
    backend_label: SharedString,
    gpu_diagnostics: String,
    frame_diagnostics: String,
    owner_stage_diagnostics: String,
    gpu_stage_diagnostics: String,
    schedule_diagnostics: String,
    memory_diagnostics: String,
    last_gpu_sample_at: Option<Instant>,
    last_scene_build_ms: u64,
    trace_poll_count: u64,
    trace_poll_max_us: u128,
    trace_scene_build_count: u64,
    trace_scene_build_max_us: u128,
    view_radius: i32,
    pending_view_radius: Option<i32>,
    last_chunk_update_at: Option<Instant>,
    _subscriptions: Vec<Subscription>,
}

impl World3dEditorView {
    fn new(init: World3dEditorInit, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let session_id = next_session_id();
        let mut camera = World3dCamera::default();
        let (spawn, spawn_source) = match init.spawn {
            Some(spawn) => {
                camera.position = spawn;
                camera.pitch = 0.42;
                (Some(spawn), init.spawn_source)
            }
            None => {
                camera.position = [0.5, 96.0, 0.5];
                (
                    None,
                    SharedString::from("回退坐标（未找到出生点）").to_string(),
                )
            }
        };
        camera.position[1] = camera.position[1].clamp(-256.0, 900.0);
        let view_size = window.viewport_size();
        let stage_size = (
            (view_size.width / px(1.0) - LEFT_RAIL_WIDTH - RIGHT_PANEL_WIDTH).max(1.0),
            (view_size.height / px(1.0) - TOP_BAR_HEIGHT - STATUS_BAR_HEIGHT).max(1.0),
        );
        let mut this = Self {
            session_id,
            world_key: init.world_key.clone(),
            world_path: init.world_path.clone(),
            world_name: init.world_name.clone(),
            spawn,
            spawn_source,
            package_roots: init.package_roots.clone(),
            owner: init.owner.clone(),
            window_handle: None,
            dimension: init.dimension,
            camera,
            navigation: World3dNavigation::new(camera),
            saved_cameras: BTreeMap::new(),
            focus: cx.focus_handle().tab_stop(true),
            look_drag: None,
            selection_drag: None,
            axis_drag: None,
            last_frame_at: None,
            last_navigation_ui_at: None,
            navigation_ui_dirty: false,
            service: None,
            poll_task: None,
            stream_generation: 0,
            last_view_key: None,
            last_view_forward: None,
            last_view_camera: None,
            last_view_sent_at: None,
            worker_status: ChunkServiceStatus::default(),
            worker_info: ChunkServiceInfo::default(),
            chunk_lights: BTreeMap::new(),
            dirty_light_chunks: BTreeSet::new(),
            chunk_meshes: BTreeMap::new(),
            section_epochs: HashMap::new(),
            dirty_sections: BTreeSet::new(),
            probes: BTreeMap::new(),
            visibility_revision: 0,
            visibility_snapshot: None,
            visibility_job: None,
            visibility_job_key: None,
            visibility_completed_key: None,
            visibility_cache: PortalResultCache::default(),
            visibility_signature_voxel: None,
            visibility_signature: None,
            hidden_draw_nodes: Arc::new(HashSet::new()),
            hidden_sections: Arc::new(BTreeSet::new()),
            invalidated_visibility: HashSet::new(),
            textures: HashMap::new(),
            texture_bytes: 0,
            texture_prune_pending: false,
            texture_table_dirty: true,
            scene_view: None,
            scene_nodes: scene_registry::ChunkScene::default(),
            scene_has_preview: false,
            render_settings: super::rendering::RenderSettings::default(),
            last_light_tick: None,
            last_pick_at: None,
            target_scene_view: None,
            target_outline_mesh: block_outline_mesh().map(Arc::new),
            target_outline_material: selection_material(AlphaMode::Blend),
            scene_dirty: true,
            scene_rebuild_required: false,
            scene_retry_at: None,
            stream_scene_pending: false,
            last_scene_build_at: None,
            import_preview: None,
            import_generation: 0,
            render_origin: [
                (camera.position[0] / RENDER_ORIGIN_GRID).floor() as i32
                    * RENDER_ORIGIN_GRID as i32,
                0,
                (camera.position[2] / RENDER_ORIGIN_GRID).floor() as i32
                    * RENDER_ORIGIN_GRID as i32,
            ],
            stage_size,
            tool: World3dTool::Navigate,
            brush_index: 0,
            selection: init.initial_selection,
            target: None,
            pointer_position: None,
            last_pick_camera: None,
            target_pick_dirty: true,
            pointer_pick_dirty: false,
            hud_debug: false,
            hud_visible: true,
            chunk_clipboard_count: 0,
            right_panel_open: true,
            left_rail_open: true,
            status: SharedString::from("正在启动 3D 区块流..."),
            status_kind: StatusKind::Info,
            last_publish_at: None,
            last_published: None,
            nav_generation: init
                .initial_target
                .as_ref()
                .map_or(0, |target| target.request_generation),
            pending_surface: None,
            edit_request_seq: 0,
            edit_busy: false,
            frame: FrameCounter::new(),
            backend_label: cx
                .try_global::<SettingsPageState>()
                .map(|settings| settings.renderer_backend.clone())
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| SharedString::from("auto")),
            gpu_diagnostics: String::new(),
            frame_diagnostics: String::new(),
            owner_stage_diagnostics: String::new(),
            gpu_stage_diagnostics: String::new(),
            schedule_diagnostics: String::new(),
            memory_diagnostics: String::new(),
            last_gpu_sample_at: None,
            last_scene_build_ms: 0,
            trace_poll_count: 0,
            trace_poll_max_us: 0,
            trace_scene_build_count: 0,
            trace_scene_build_max_us: 0,
            view_radius: init.view_radius.clamp(1, WORLD3D_MAX_VIEW_RADIUS),
            pending_view_radius: None,
            last_chunk_update_at: None,
            _subscriptions: vec![cx.observe_global::<ThemeState>(|_this, cx| cx.notify())],
        };
        this.camera.view_distance = ((this.view_radius + 1) * 16).max(320) as f32;
        this.navigation.set_camera(this.camera);
        this._subscriptions
            .push(cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.stop_navigation(cx);
                }
            }));
        this._subscriptions
            .push(cx.on_blur(&this.focus, window, |this, _, cx| {
                this.stop_navigation(cx);
            }));
        this.update_target_scene();
        this.start_stream(cx);
        if let Some(target) = init.initial_target {
            this.navigate(target, cx);
        } else if this.spawn.is_none() {
            // The saved spawn height is unusable, so resolve the real terrain surface
            // asynchronously instead of guessing an underground camera position.
            let x = this.camera.position[0].floor() as i32;
            let z = this.camera.position[2].floor() as i32;
            this.pending_surface = Some(this.nav_generation);
            this.request_surface_height(x, z, this.nav_generation);
        }
        this.request_view_update(true, cx);
        this
    }

    fn start_stream(&mut self, cx: &mut Context<Self>) {
        let config = ChunkServiceConfig {
            world_path: self.world_path.clone(),
            world_name: self.world_name.clone(),
            dimension: self.dimension,
            package_roots: self.package_roots.clone(),
            view_radius: self.view_radius,
        };
        match spawn_chunk_service(config) {
            Ok(service) => {
                service.send(super::types::ChunkServiceCommand::SetLighting(
                    self.render_settings.lighting,
                ));
                self.service = Some(service);
                self.status = SharedString::from("3D 区块流已启动，正在加载出生点周围区块");
                self.status_kind = StatusKind::Info;
            }
            Err(error) => {
                self.status = SharedString::from(format!("启动 3D 区块流失败：{error}"));
                self.status_kind = StatusKind::Error;
            }
        }
        self.poll_task = Some(cx.spawn(async move |handle, cx| {
            loop {
                Timer::after(POLL_INTERVAL).await;
                let Some(view) = handle.upgrade() else {
                    break;
                };
                if view
                    .update(cx, |this, cx| {
                        this.sync_navigation_camera(cx);
                        this.poll_stream_events(cx);
                        this.maybe_update_view(cx);
                        this.publish_camera_snapshot(cx);
                        if this.pointer_pick_dirty && is_pick_due(this.last_pick_at, Instant::now())
                        {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
            Ok::<(), anyhow::Error>(())
        }));
    }

    fn poll_stream_events(&mut self, cx: &mut Context<Self>) {
        let trace_started = super::chunk_service::trace_timings().then(Instant::now);
        let mut ui_changed = false;
        let mut meshes_added = 0usize;
        let mut textures_may_be_unused = false;
        let mut probes_changed = false;
        let previous_texture_count = self.textures.len();
        let poll_started = Instant::now();
        let mut submitted_bytes = 0usize;
        loop {
            let Some(event) = self.service.as_ref().and_then(ChunkServiceHandle::try_recv) else {
                break;
            };
            ui_changed |= stream_event_updates_ui(&event);
            let event_trace = trace_started.map(|_| {
                let (kind, items) = match &event {
                    ChunkServiceEvent::Ready(_) => ("ready", 1),
                    ChunkServiceEvent::Chunk(_) => ("chunk", 1),
                    ChunkServiceEvent::Completed(items) => ("completed", items.len()),
                    ChunkServiceEvent::Unloaded { .. } => ("unloaded", 1),
                    ChunkServiceEvent::Status(_) => ("status", 1),
                    ChunkServiceEvent::Surface { heights, .. } => ("surface", heights.len()),
                    ChunkServiceEvent::Fatal { .. } => ("fatal", 1),
                };
                (Instant::now(), kind, items)
            });
            match event {
                ChunkServiceEvent::Ready(info) => {
                    self.worker_info = info;
                    self.texture_table_dirty = true;
                    self.status = SharedString::from(format!(
                        "资源就绪：{} 个方块模型 · {} 个资源包根",
                        self.worker_info.block_model_count, self.worker_info.resource_pack_count
                    ));
                }
                ChunkServiceEvent::Chunk(payload)
                    if payload.lighting != self.render_settings.lighting
                        || payload.mesh.pos.dimension != self.dimension => {}
                ChunkServiceEvent::Chunk(payload) => {
                    if !accept_section_epoch(
                        &mut self.section_epochs, payload.mesh.pos, payload.generation,
                    ) {
                        continue;
                    }
                    // Time-budget pressure follows newly resident data, not the same
                    // texture bytes multiplied by every section which references it.
                    let geometry_bytes = payload.mesh.estimated_geometry_cpu_bytes();
                    let mut new_texture_bytes = 0usize;
                    for texture in payload.mesh.textures.iter() {
                        let id = texture.id();
                        if !self.textures.contains_key(&id) {
                            let bytes = texture_storage_bytes(texture);
                            new_texture_bytes = new_texture_bytes.saturating_add(bytes);
                            self.texture_bytes = self.texture_bytes.saturating_add(bytes);
                            self.textures.insert(id, texture.clone());
                        }
                    }
                    submitted_bytes = submitted_bytes
                        .saturating_add(geometry_bytes)
                        .saturating_add(new_texture_bytes);
                    probes_changed |= upsert_shared_probe(
                        &mut self.probes,
                        payload.mesh.pos,
                        Arc::clone(&payload.probe),
                    );
                    // Mesh residency is not a change to the decoded portal
                    // graph. New bands are already included in the graph's
                    // prospective candidates, so this must not invalidate
                    // a good traversal or briefly clear the GPU portal mask.
                    let key = (payload.mesh.pos, payload.mesh.section_y);
                    if self.chunk_meshes.insert(key, payload.mesh).is_some() {
                        textures_may_be_unused = true;
                    }
                    self.dirty_sections.insert(key);
                    meshes_added = meshes_added.saturating_add(1);
                    self.stream_scene_pending = true;
                    if meshes_added >= MAX_MESH_EVENTS_PER_POLL {
                        break;
                    }
                }
                ChunkServiceEvent::Completed(completions) => {
                    let mut removed = false;
                    let mut restored = false;
                    for completion in completions {
                        if completion.lighting != self.render_settings.lighting {
                            continue;
                        }
                        if completion.pos.dimension != self.dimension
                            || !accept_section_epoch(
                                &mut self.section_epochs,
                                completion.pos,
                                completion.generation,
                            )
                        {
                            continue;
                        }
                        if completion.lights.is_empty() {
                            if self.chunk_lights.remove(&completion.pos).is_some() {
                                self.dirty_light_chunks.insert(completion.pos);
                                self.stream_scene_pending = true;
                            }
                        } else if self
                            .chunk_lights
                            .get(&completion.pos)
                            .is_none_or(|old| old.as_ref() != completion.lights.as_ref())
                        {
                            self.chunk_lights.insert(completion.pos, completion.lights);
                            self.dirty_light_chunks.insert(completion.pos);
                            self.stream_scene_pending = true;
                        }
                        restored |= restore_retained_sections(
                            &mut self.chunk_meshes,
                            &mut self.dirty_sections,
                            &mut self.textures,
                            &mut self.texture_bytes,
                            completion.retained_meshes,
                        );
                        removed |= retain_chunk_sections(
                            &mut self.chunk_meshes,
                            &mut self.dirty_sections,
                            completion.pos,
                            &completion.sections,
                        );
                        probes_changed |= upsert_shared_probe(
                            &mut self.probes, completion.pos, completion.probe,
                        );
                        // Completion only changes GPU Section residency.
                        // Graph revision is advanced above solely if the
                        // immutable Probe identity actually changed.
                        self.invalidated_visibility.remove(&completion.pos);
                    }
                    textures_may_be_unused |= removed;
                    self.stream_scene_pending |= removed || restored;
                }
                ChunkServiceEvent::Unloaded { pos, .. } if pos.dimension != self.dimension => {}
                ChunkServiceEvent::Unloaded { generation, pos } => {
                    if !accept_section_epoch(&mut self.section_epochs, pos, generation) {
                        continue;
                    }
                    if self.chunk_lights.remove(&pos).is_some() {
                        self.dirty_light_chunks.insert(pos);
                        self.stream_scene_pending = true;
                    }
                    let removed = retain_chunk_sections(
                        &mut self.chunk_meshes,
                        &mut self.dirty_sections,
                        pos,
                        &[],
                    );
                    textures_may_be_unused |= removed;
                    probes_changed |= self.probes.remove(&pos).is_some();
                    self.invalidated_visibility.remove(&pos);
                    self.stream_scene_pending |= removed;
                }
                ChunkServiceEvent::Status(status) => {
                    self.worker_status = status;
                }
                ChunkServiceEvent::Surface {
                    request_id,
                    heights,
                } => {
                    if Some(request_id) == self.pending_surface {
                        self.pending_surface = None;
                        if let (Some(surface), Some(target)) = (
                            heights.first().copied().flatten(),
                            self.nav_target_position(),
                        ) {
                            self.camera = self.navigation.update_camera(|camera| {
                                camera.teleport_to_surface(target[0], surface, target[1]);
                            });
                            self.status = SharedString::from(format!(
                                "已按地表安全高度传送到 Y {}",
                                surface + 1
                            ));
                            self.status_kind = StatusKind::Success;
                            self.publish_camera_snapshot(cx);
                            self.request_view_update(true, cx);
                        }
                    }
                }
                ChunkServiceEvent::Fatal { error } => {
                    self.status = SharedString::from(error);
                    self.status_kind = StatusKind::Error;
                }
            }
            if let Some((started, kind, items)) = event_trace {
                let elapsed = started.elapsed();
                if elapsed >= Duration::from_millis(16) {
                    eprintln!(
                        "world3d ui event ts={} kind={} items={} elapsed_ms={:.1} sections={}",
                        super::chunk_service::trace_timestamp_ms(),
                        kind,
                        items,
                        elapsed.as_secs_f64() * 1000.0,
                        self.chunk_meshes.len(),
                    );
                }
            }
            if poll_started.elapsed() >= Duration::from_millis(2)
                || submitted_bytes >= MESH_SUBMIT_BUDGET_BYTES
            {
                break;
            }
        }
        // Bound stale unload tombstones while keeping epochs for resident Sections.
        if self.section_epochs.len() > SECTION_EPOCH_MAX_TOMBSTONES {
            let recent = self.stream_generation.saturating_sub(64);
            let resident = self.chunk_meshes.keys()
                .map(|(pos, _)| *pos)
                .collect::<HashSet<_>>();
            self.section_epochs.retain(|pos, generation| {
                resident.contains(pos) || *generation >= recent
            });
        }
        // Consume completed CPU-only fusion jobs on the UI owner. Until the
        // matching source fingerprint is ready, independent Section meshes
        // remain visible and no synchronous geometry merge blocks the frame.
        // The completed tile keys join the next immutable Scene delta even if
        // no new chunk arrived in this poll.
        if self.scene_nodes.harvest_finished_fusions() {
            self.scene_dirty = true;
            self.stream_scene_pending = true;
            ui_changed = true;
        }
        self.texture_prune_pending |= textures_may_be_unused;
        self.texture_table_dirty |= self.textures.len() != previous_texture_count;
        self.target_pick_dirty |= probes_changed;
        if probes_changed {
            self.visibility_revision = self.visibility_revision.wrapping_add(1);
            self.visibility_snapshot = None;
            self.visibility_signature_voxel = None;
            self.visibility_completed_key = None;
            self.visibility_cache.clear();
            // Any in-flight graph may contain already invalidated or unloaded
            // Sections. Do not keep its old GPU occlusion mask while streaming.
            ui_changed |= self.clear_portal_visibility();
        }
        if meshes_added > 0 {
            self.last_chunk_update_at = Some(Instant::now());
        }
        if ui_changed {
            cx.notify();
        }
        if let Some(started) = trace_started {
            self.trace_poll_count = self.trace_poll_count.saturating_add(1);
            self.trace_poll_max_us = self.trace_poll_max_us.max(started.elapsed().as_micros());
        }
    }

    fn nav_target_position(&self) -> Option<[i32; 2]> {
        // Surface probe answers are matched to the camera column; the camera already sits
        // at the requested X/Z, so reuse its current integer column.
        Some([
            self.camera.position[0].floor() as i32,
            self.camera.position[2].floor() as i32,
        ])
    }

    fn current_view_key(&self) -> (Dimension, i32, i32, i32, i32) {
        (
            self.dimension,
            (self.camera.position[0] / 16.0).floor() as i32,
            (self.camera.position[1] / 16.0).floor() as i32,
            (self.camera.position[2] / 16.0).floor() as i32,
            self.view_radius.max(1),
        )
    }

    fn set_view_radius(&mut self, radius: i32, cx: &mut Context<Self>) {
        self.pending_view_radius = None;
        self.view_radius = radius.clamp(1, WORLD3D_MAX_VIEW_RADIUS);
        self.camera = self.navigation.update_camera(|camera| {
            camera.view_distance = (self.view_radius + 1).saturating_mul(16).max(320) as f32;
        });
        self.last_view_key = None;
        self.request_view_update(true, cx);
    }

    fn request_view_update(&mut self, force: bool, cx: &mut Context<Self>) {
        let key = self.current_view_key();
        let forward = self.camera.forward_f32();
        let direction_changed = self.last_view_forward.is_none_or(|previous| {
            previous
                .iter()
                .zip(forward)
                .map(|(&a, b)| a * b)
                .sum::<f32>() < 0.975
        });
        // Section portals are sensitive to moving between disconnected
        // pockets even inside the same 16^3 SubChunk. Report a material
        // camera displacement to the worker (bounded by the 120 ms throttle)
        // instead of waiting for an X/Z or Y Section boundary crossing.
        let moved = self.last_view_camera.is_none_or(|last| {
            self.camera.position.iter().zip(last)
                .map(|(now, prior)| (now - prior).powi(2))
                .sum::<f64>() >= 16.0
        });
        let now = Instant::now();
        let throttled = self
            .last_view_sent_at
            .is_some_and(|at| now - at < VIEW_UPDATE_INTERVAL);
        if !force && ((self.last_view_key == Some(key) && !direction_changed && !moved) || throttled) {
            return;
        }
        self.last_view_key = Some(key);
        self.last_view_forward = Some(forward);
        self.last_view_camera = Some(self.camera.position);
        self.last_view_sent_at = Some(now);
        self.stream_generation = self.stream_generation.saturating_add(1);
        self.last_publish_at = None;
        let Some(service) = self.service.as_ref() else {
            return;
        };
        service.send(super::types::ChunkServiceCommand::SetView {
            generation: self.stream_generation,
            dimension: self.dimension,
            camera: self.camera.position,
            forward_xz: [forward[0], forward[2]],
            forward_y: forward[1],
            radius: self.view_radius.max(1),
            view_distance: self.camera.view_distance,
        });
        cx.notify();
    }

    /// Publish a camera reachability mask, not an authored Scene fork.
    /// A pending async result is never allowed to hide a Section after the
    /// camera leaves its air component or the decoded column epoch changes.
    fn apply_portal_visibility(&mut self, hidden: Arc<BTreeSet<(ChunkPos, i32)>>) -> bool {
        if Arc::ptr_eq(&self.hidden_sections, &hidden)
            || self.hidden_sections.as_ref() == hidden.as_ref()
        {
            return false;
        }
        self.hidden_sections = hidden;
        self.hidden_draw_nodes =
            Arc::new(self.scene_nodes.hidden_draw_nodes(&self.hidden_sections));
        if super::chunk_service::trace_timings() {
            eprintln!(
                "world3d portal_visibility hidden_bands={} hidden_nodes={} resident_bands={}",
                self.hidden_sections.len(),
                self.hidden_draw_nodes.len(),
                self.chunk_meshes.len(),
            );
        }
        true
    }

    /// Reset portal masking without allocating on the common already-empty
    /// path. Unknown sections and edits must fail open.
    fn clear_portal_visibility(&mut self) -> bool {
        if self.hidden_sections.is_empty() {
            false
        } else {
            self.apply_portal_visibility(Arc::new(BTreeSet::new()))
        }
    }

    /// Sodium-style frame-independent graph solve. The GPUI thread only
    /// computes an O(1) cached air-component signature and publishes the
    /// completed snapshot; a single Rayon job handles all Section traversal.
    /// Stale results are ignored, and a newer camera request coalesces behind
    /// the currently running job rather than accumulating background work.
    fn update_section_visibility(&mut self) -> bool {
        let voxel = self.camera.position.map(|v| v.floor() as i32);
        if self.visibility_signature_voxel != Some((self.visibility_revision, voxel)) {
            self.visibility_signature = super::visibility::camera_signature(
                &self.probes, self.dimension, voxel,
            );
            self.visibility_signature_voxel = Some((self.visibility_revision, voxel));
        }
        let key = PortalVisibilityKey {
            revision: self.visibility_revision,
            dimension: self.dimension,
            voxel,
            radius: self.view_radius,
            component: self.visibility_signature,
        };
        let safe = !self.edit_busy
            && self.invalidated_visibility.is_empty()
            && !super::chunk_service::portal_culling_disabled();

        let result = self.visibility_job.as_ref().map(Receiver::try_recv);
        match result {
            Some(Ok(result)) => {
                self.visibility_job = None;
                self.visibility_job_key = None;
                if safe && portal_snapshot_compatible(result.key, key) {
                    if super::chunk_service::trace_timings() && result.solve_us >= 2_000 {
                        eprintln!(
                            "world3d portal_visibility_async_us={} hidden_bands={} current_component={}",
                            result.solve_us,
                            result.hidden.len(),
                            same_portal_region(result.key, key),
                        );
                    }
                    let hidden = Arc::new(result.hidden);
                    // Keep useful work done for a previous camera region in
                    // the bounded cache instead of discarding it when the
                    // camera moves faster than the background traversal.
                    self.visibility_cache.insert(result.key, Arc::clone(&hidden));
                    if same_portal_region(result.key, key) {
                        self.visibility_completed_key = Some(key);
                        return self.apply_portal_visibility(hidden);
                    }
                }
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.visibility_job = None;
                self.visibility_job_key = None;
            }
            _ => {}
        }
        // During an edit or while the camera's containing Section is
        // unknown/solid, fail open immediately: rendering extra geometry is
        // always safer than showing an outdated hole in a mine or cliff.
        if !safe {
            // Do not mark an unsafe edit as a completed cull result: if the
            // edit is cancelled, the same component must be scheduled again.
            self.visibility_completed_key = None;
            return self.clear_portal_visibility();
        }
        if key.component.is_none() {
            self.visibility_completed_key = Some(key);
            return self.clear_portal_visibility();
        }
        if self
            .visibility_completed_key
            .is_some_and(|previous| same_portal_region(previous, key))
        {
            return false;
        }
        if let Some(hidden) = self.visibility_cache.find(key) {
            if super::chunk_service::trace_timings() {
                eprintln!(
                    "world3d portal_result_cache_hit revision={} hidden_bands={} retained_regions={}",
                    key.revision,
                    hidden.len(),
                    self.visibility_cache.entries.len(),
                );
            }
            self.visibility_completed_key = Some(key);
            return self.apply_portal_visibility(hidden);
        }
        if let Some(inflight) = self.visibility_job_key {
            // When moving inside the same connected air pocket we may reuse
            // an in-flight result. Otherwise retire the prior mask now and
            // wait for the single task to finish before submitting the next.
            return if same_portal_region(inflight, key) {
                false
            } else {
                self.clear_portal_visibility()
            };
        }
        let snapshot = match &self.visibility_snapshot {
            Some((revision, cached)) if *revision == key.revision => Arc::clone(cached),
            _ => {
                let snapshot = Arc::new(PortalVisibilitySnapshot {
                    probes: self.probes.clone(),
                    bands: portal_candidate_bands(
                        &self.probes,
                        self.chunk_meshes.keys().copied(),
                    ),
                });
                self.visibility_snapshot = Some((key.revision, Arc::clone(&snapshot)));
                snapshot
            }
        };
        if snapshot.probes.is_empty() || snapshot.bands.is_empty() {
            self.visibility_completed_key = Some(key);
            return self.clear_portal_visibility();
        }
        let (tx, rx) = mpsc::sync_channel(1);
        rayon::spawn(move || {
            let started = Instant::now();
            let hidden = super::visibility::hidden_render_band_keys(
                &snapshot.probes,
                &snapshot.bands,
                key.dimension,
                key.voxel,
                key.radius,
            );
            let _ = tx.send(PortalVisibilityResult {
                key,
                hidden,
                solve_us: started.elapsed().as_micros(),
            });
        });
        self.visibility_job_key = Some(key);
        self.visibility_job = Some(rx);
        // A previous camera/component's occlusion mask cannot safely persist
        // while the new one is computed. Keep the resident Draw slots visible
        // until the worker publishes a matching snapshot.
        self.clear_portal_visibility()
    }

    fn maybe_update_view(&mut self, cx: &mut Context<Self>) {
        if self.update_section_visibility() {
            cx.notify();
        }
        // Camera/radius changes update SceneView::culling_bounds and trigger
        // GPU visibility, not a full CPU Scene fork and Section-visible walk.
        if self.stream_scene_pending
            && self
                .last_scene_build_at
                .is_none_or(|last| last.elapsed() >= STREAM_SCENE_INTERVAL)
        {
            self.scene_dirty = true;
        }
        if self.scene_dirty {
            if self.ensure_scene(cx) {
                cx.notify();
            }
        }
        self.request_view_update(false, cx);
    }

    fn camera_snapshot(&self) -> World3dCameraSnapshot {
        let aspect = if self.stage_size.0 > 1.0 && self.stage_size.1 > 1.0 {
            self.stage_size.0 / self.stage_size.1
        } else {
            16.0 / 9.0
        };
        self.camera
            .snapshot(self.dimension, aspect, self.stream_generation)
    }

    fn sync_navigation_camera(&mut self, cx: &mut Context<Self>) {
        let camera = self.navigation.camera();
        if camera != self.camera {
            self.camera = camera;
            self.navigation_ui_dirty = true;
        }
        let now = Instant::now();
        if self.navigation_ui_dirty
            && self
                .last_navigation_ui_at
                .is_none_or(|last| now - last >= NAVIGATION_UI_INTERVAL)
        {
            self.last_navigation_ui_at = Some(now);
            self.navigation_ui_dirty = false;
            cx.notify();
        }
    }

    fn publish_camera_snapshot(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        if !camera_snapshot_due(self.last_publish_at, now) {
            return;
        }
        let snapshot = self.camera_snapshot();
        if self.last_published == Some(snapshot) {
            return;
        }
        self.last_published = Some(snapshot);
        self.last_publish_at = Some(now);
        if let Some(owner) = self.owner.as_ref() {
            let owner = owner.clone();
            // Navigation can run while the map entity is still being updated.
            cx.defer(move |cx| {
                let _ = owner.update(cx, |main, cx| {
                    main.update_world3d_camera_snapshot(Some(snapshot), cx);
                });
            });
        }
    }

    fn ensure_scene(&mut self, cx: &mut Context<Self>) -> bool {
        let _ = cx;
        if !self.scene_dirty {
            return false;
        }
        if self
            .scene_retry_at
            .is_some_and(|retry_at| Instant::now() < retry_at)
        {
            return false;
        }
        let started = Instant::now();
        self.last_scene_build_at = Some(Instant::now());
        if std::mem::take(&mut self.texture_prune_pending) {
            let previous_count = self.textures.len();
            self.texture_bytes = retain_chunk_textures(&self.chunk_meshes, &mut self.textures);
            self.texture_table_dirty |= self.textures.len() != previous_count;
        }
        let previous_origin = self.render_origin;
        let next_origin = [
            (self.camera.position[0] / RENDER_ORIGIN_GRID).round() as i32
                * RENDER_ORIGIN_GRID as i32,
            0,
            (self.camera.position[2] / RENDER_ORIGIN_GRID).round() as i32
                * RENDER_ORIGIN_GRID as i32,
        ];
        let base = self
            .scene_view
            .as_ref()
            .filter(|_| {
                !self.scene_rebuild_required
                    && self.import_preview.is_none()
                    && !self.scene_has_preview
            })
            .map(|view| view.scene().as_ref());
        let mut scene = match self.scene_nodes.update(
            base,
            &self.chunk_meshes,
            Some(&self.dirty_sections),
            next_origin,
        ) {
            Ok(scene) => scene,
            Err(error) => {
                self.scene_nodes = scene_registry::ChunkScene::default();
                self.scene_rebuild_required = true;
                self.scene_retry_at = Some(Instant::now() + STREAM_SCENE_INTERVAL);
                self.status = format!("更新世界场景失败：{error}").into();
                self.status_kind = StatusKind::Error;
                return true;
            }
        };
        if let Err(error) = self.scene_nodes.update_lights(
            &mut scene,
            &self.chunk_lights,
            base.map(|_| &self.dirty_light_chunks),
            next_origin,
        ) {
            self.scene_nodes = scene_registry::ChunkScene::default();
            self.scene_rebuild_required = true;
            self.scene_retry_at = Some(Instant::now() + STREAM_SCENE_INTERVAL);
            self.status = format!("更新世界光源失败：{error}").into();
            self.status_kind = StatusKind::Error;
            return true;
        }
        // Camera portal visibility is now a per-view GPU resident Bounds
        // overlay. Never fork a Scene merely to change Node::visible.
        self.hidden_draw_nodes =
            Arc::new(self.scene_nodes.hidden_draw_nodes(&self.hidden_sections));
        if let Some(preview) = self.import_preview.as_ref()
            && preview.target_anchor.dimension == self.dimension
            && let Err(error) = preview
                .mesh
                .append_world_preview(&mut scene, preview.scene_transform(next_origin))
        {
            self.scene_nodes = scene_registry::ChunkScene::default();
            self.scene_rebuild_required = true;
            self.scene_retry_at = Some(Instant::now() + STREAM_SCENE_INTERVAL);
            self.status = SharedString::from(format!("导入预览场景失败：{error}"));
            self.status_kind = StatusKind::Error;
            return true;
        }
        let scene = match (self.scene_view.as_ref(), base) {
            (Some(view), Some(previous)) if previous.version() == scene.version() => {
                view.scene().clone()
            }
            _ => Arc::new(scene),
        };
        let textures_changed = self.texture_table_dirty;
        let published = scene_publish_required(self.scene_view.as_ref(), &scene, textures_changed);
        if published {
            let camera = self.camera.gpu_camera(next_origin);
            let base = match self.scene_view.as_ref() {
                Some(view) => view.with_scene(scene).with_camera(camera),
                None => SceneView::new(scene, camera),
            }
            .with_hidden_nodes(Arc::clone(&self.hidden_draw_nodes))
            .with_sampling(TextureSampling::PixelArt)
            .with_voxel_shader()
            .with_continuous_presentation();
            let scene_view = if textures_changed {
                match base.with_textures(
                    self.textures
                        .values()
                        .cloned()
                        .chain(self.worker_info.cloud_texture.iter().cloned())
                        .chain(
                            self.worker_info
                                .water_textures
                                .iter()
                                .flatten()
                                .map(|v| v.asset.clone()),
                        ),
                ) {
                    Ok(view) => view,
                    Err(error) => {
                        self.scene_nodes = scene_registry::ChunkScene::default();
                        self.scene_rebuild_required = true;
                        self.scene_retry_at = Some(Instant::now() + STREAM_SCENE_INTERVAL);
                        self.status = format!("更新世界纹理失败：{error}").into();
                        self.status_kind = StatusKind::Error;
                        return true;
                    }
                }
            } else {
                base
            };
            self.scene_view = Some(Arc::new(scene_view));
        }
        self.render_origin = next_origin;
        self.navigation.set_origin(next_origin);
        if next_origin != previous_origin {
            self.update_target_scene();
        }
        self.scene_has_preview = self.import_preview.is_some();
        self.texture_table_dirty = false;
        self.dirty_sections.clear();
        self.dirty_light_chunks.clear();
        self.scene_dirty = false;
        self.scene_rebuild_required = false;
        self.scene_retry_at = None;
        self.stream_scene_pending = false;
        self.last_scene_build_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if super::chunk_service::trace_timings() {
            self.trace_scene_build_count = self.trace_scene_build_count.saturating_add(1);
            self.trace_scene_build_max_us = self
                .trace_scene_build_max_us
                .max(started.elapsed().as_micros());
        }
        published
    }

    fn stage_aspect(&self) -> f32 {
        if self.stage_size.0 > 1.0 && self.stage_size.1 > 1.0 {
            (self.stage_size.0 / self.stage_size.1).clamp(0.1, 10.0)
        } else {
            16.0 / 9.0
        }
    }

    fn scene_culling_bounds(&self) -> Aabb {
        let radius = f64::from(self.view_radius.max(1));
        let x = (self.camera.position[0] / 16.0).floor();
        let z = (self.camera.position[2] / 16.0).floor();
        Aabb {
            min: Vec3::new(
                ((x - radius) * 16.0 - f64::from(self.render_origin[0])) as f32,
                -f32::MAX,
                ((z - radius) * 16.0 - f64::from(self.render_origin[2])) as f32,
            ),
            max: Vec3::new(
                ((x + radius + 1.0) * 16.0 - f64::from(self.render_origin[0])) as f32,
                f32::MAX,
                ((z + radius + 1.0) * 16.0 - f64::from(self.render_origin[2])) as f32,
            ),
        }
    }

    pub(super) fn set_import_preview(
        &mut self,
        generation: u64,
        preview: Option<World3dImportPreview>,
        cx: &mut Context<Self>,
    ) {
        if generation < self.import_generation {
            return;
        }
        self.import_generation = generation;
        let appeared = self.import_preview.is_none() && preview.is_some();
        self.import_preview = preview;
        if appeared {
            self.tool = World3dTool::Import;
            self.right_panel_open = true;
        } else if self.import_preview.is_none() && self.tool == World3dTool::Import {
            self.axis_drag = None;
            self.tool = World3dTool::Navigate;
        }
        self.scene_dirty = true;
        cx.notify();
    }

    fn request_import_action(&mut self, action: World3dImportAction, cx: &mut Context<Self>) {
        if self
            .import_preview
            .as_ref()
            .is_some_and(|preview| preview.is_writing)
        {
            self.status = "粘贴正在写入；请等待完成，或在 2D 任务中取消".into();
            self.status_kind = StatusKind::Warning;
            cx.notify();
            return;
        }
        let Some(owner) = self.owner.clone() else {
            self.status = SharedString::from("当前 3D 窗口没有关联的 2D 地图文档");
            self.status_kind = StatusKind::Warning;
            cx.notify();
            return;
        };
        cx.defer(move |cx| {
            let _ = owner.update(cx, |main, cx| {
                main.apply_world3d_import_action(action, cx);
            });
        });
    }

    pub(super) fn set_chunk_clipboard_count(&mut self, count: usize, cx: &mut Context<Self>) {
        self.chunk_clipboard_count = count;
        cx.notify();
    }

    pub(super) fn set_clipboard_copy_status(
        &mut self,
        message: String,
        ok: bool,
        cx: &mut Context<Self>,
    ) {
        self.status = SharedString::from(message);
        self.status_kind = if ok {
            StatusKind::Success
        } else {
            StatusKind::Error
        };
        cx.notify();
    }

    fn copy_chunks_to_map(&mut self, cx: &mut Context<Self>) {
        let Some(owner) = self.owner.clone() else {
            self.status = SharedString::from("没有关联的 2D 地图文档");
            self.status_kind = StatusKind::Warning;
            cx.notify();
            return;
        };
        let chunks = if let Some(selection) = self.selection {
            selection.chunk_positions()
        } else if let Some(target) = self.target.as_ref() {
            vec![target.chunk]
        } else {
            vec![ChunkPos {
                x: (self.camera.position[0] / 16.0).floor() as i32,
                z: (self.camera.position[2] / 16.0).floor() as i32,
                dimension: self.dimension,
            }]
        };
        let count = chunks.len();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |main, cx| main.copy_world3d_chunks(chunks, cx));
        });
        self.status = SharedString::from(format!("正在通过 2D 文档复制 {count} 个完整区块"));
        self.status_kind = StatusKind::Info;
        cx.notify();
    }

    fn copy_selection_to_map(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.selection else {
            self.status = SharedString::from("请先在 3D 中建立包含高度的选区");
            self.status_kind = StatusKind::Warning;
            cx.notify();
            return;
        };
        let Some(owner) = self.owner.clone() else {
            self.status = SharedString::from("没有关联的 2D 地图文档");
            self.status_kind = StatusKind::Warning;
            cx.notify();
            return;
        };
        cx.defer(move |cx| {
            let _ = owner.update(cx, |main, cx| main.copy_world3d_selection(selection, cx));
        });
        self.status = SharedString::from(format!("正在复制 3D 选区，尺寸 {:?}", selection.size()));
        self.status_kind = StatusKind::Info;
        cx.notify();
    }

    fn start_chunk_paste(&mut self, cx: &mut Context<Self>) {
        if self.chunk_clipboard_count == 0 {
            self.status = SharedString::from("地图剪贴板为空；请先复制 3D 选区或完整区块");
            self.status_kind = StatusKind::Warning;
            cx.notify();
            return;
        }
        let position = self
            .target
            .as_ref()
            .map(World3dTargetBlock::face_adjacent)
            .unwrap_or_else(|| {
                [
                    self.camera.position[0].floor() as i32,
                    0,
                    self.camera.position[2].floor() as i32,
                ]
            });
        self.request_import_action(
            World3dImportAction::StartAt(ChunkPos {
                x: position[0].div_euclid(16),
                z: position[2].div_euclid(16),
                dimension: self.dimension,
            }),
            cx,
        );
        self.status = SharedString::from("已请求创建地图粘贴预览");
        self.status_kind = StatusKind::Info;
        cx.notify();
    }

    fn stage_rect(&self, window: &Window) -> (f32, f32, f32, f32) {
        let size = window.viewport_size();
        let width = (size.width / px(1.0)).max(1.0);
        let height = (size.height / px(1.0)).max(1.0);
        let left = if self.left_rail_open {
            LEFT_RAIL_WIDTH
        } else {
            0.0
        };
        let right = if self.right_panel_open {
            RIGHT_PANEL_WIDTH
        } else {
            0.0
        };
        (
            left,
            TOP_BAR_HEIGHT,
            (width - left - right).max(1.0),
            (height - TOP_BAR_HEIGHT - STATUS_BAR_HEIGHT).max(1.0),
        )
    }

    fn update_stage_size(&mut self, window: &Window) -> bool {
        let (_, _, width, height) = self.stage_rect(window);
        let changed =
            (self.stage_size.0 - width).abs() > 0.5 || (self.stage_size.1 - height).abs() > 0.5;
        if changed {
            self.stage_size = (width, height);
        }
        changed
    }

    // ---- navigation -------------------------------------------------------------------

    pub(super) fn navigate_for_map(
        &mut self,
        target: WorldNavigationTarget,
        cx: &mut Context<Self>,
    ) {
        self.navigate(target, cx);
    }

    fn navigate(&mut self, target: WorldNavigationTarget, cx: &mut Context<Self>) {
        self.camera = self.navigation.camera();
        self.finish_axis_drag(cx);
        self.selection_drag = None;
        self.nav_generation = target.request_generation;
        if target.dimension != self.dimension {
            self.saved_cameras.insert(self.dimension, self.camera);
            self.dimension = target.dimension;
            self.chunk_meshes.clear();
            self.dirty_sections.clear();
            self.chunk_lights.clear();
            self.dirty_light_chunks.clear();
            self.probes.clear();
            self.textures.clear();
            self.texture_bytes = 0;
            self.texture_prune_pending = false;
            self.texture_table_dirty = true;
            self.scene_view = None;
            self.scene_retry_at = None;
            self.target_scene_view = None;
            self.target = None;
            self.scene_dirty = true;
            let restored = self
                .saved_cameras
                .get(&target.dimension)
                .copied()
                .unwrap_or_else(|| {
                    let mut camera = World3dCamera::default();
                    if let Some(spawn) = self.spawn {
                        camera.position = spawn;
                    }
                    camera
                });
            self.camera = restored;
        }
        let y = match target.height_mode {
            TeleportHeightMode::Exact => target.requested_y.unwrap_or(96.0),
            TeleportHeightMode::CurrentMapLayer => target
                .requested_y
                .unwrap_or(f64::from(self.camera.position[1])),
            TeleportHeightMode::SurfaceSafe => {
                self.pending_surface = Some(target.request_generation);
                let fallback = self.spawn.map_or(96.0, |spawn| spawn[1]);
                target.requested_y.unwrap_or(fallback)
            }
        };
        self.camera
            .teleport_to(target.block_x, y.clamp(-256.0, 900.0), target.block_z);
        self.navigation.set_camera(self.camera);
        self.status = SharedString::from(format!(
            "{}传送到 X {} · Y {} · Z {}",
            dimension_label(target.dimension),
            target.block_x,
            y.round(),
            target.block_z
        ));
        self.status_kind = StatusKind::Info;
        if target.height_mode == TeleportHeightMode::SurfaceSafe {
            self.request_surface_height(target.block_x, target.block_z, target.request_generation);
        }
        if self.selection.is_some()
            && self
                .selection
                .is_some_and(|selection| selection.dimension != target.dimension)
        {
            self.selection = None;
        }
        self.update_target_scene();
        self.last_pick_camera = None;
        self.request_view_update(true, cx);
        self.publish_camera_snapshot(cx);
        cx.notify();
    }

    fn request_surface_height(&mut self, block_x: i32, block_z: i32, request_id: u64) {
        let Some(service) = self.service.as_ref() else {
            return;
        };
        service.send(super::types::ChunkServiceCommand::ProbeSurface {
            request_id,
            dimension: self.dimension,
            positions: vec![(block_x, block_z)],
        });
    }

    fn switch_dimension(&mut self, dimension: Dimension, cx: &mut Context<Self>) {
        if dimension == self.dimension {
            return;
        }
        self.camera = self.navigation.camera();
        self.finish_axis_drag(cx);
        self.selection_drag = None;
        self.saved_cameras.insert(self.dimension, self.camera);
        self.dimension = dimension;
        self.chunk_meshes.clear();
        self.dirty_sections.clear();
        self.chunk_lights.clear();
        self.dirty_light_chunks.clear();
        self.probes.clear();
        self.last_view_forward = None;
        self.last_view_camera = None;
        self.visibility_revision = self.visibility_revision.wrapping_add(1);
        self.visibility_snapshot = None;
        self.visibility_job = None;
        self.visibility_job_key = None;
        self.visibility_completed_key = None;
        self.visibility_cache.clear();
        self.visibility_signature_voxel = None;
        self.visibility_signature = None;
        self.hidden_sections = Arc::new(BTreeSet::new());
        self.hidden_draw_nodes = Arc::new(HashSet::new());
        self.invalidated_visibility.clear();
        self.textures.clear();
        self.texture_bytes = 0;
        self.texture_prune_pending = false;
        self.texture_table_dirty = true;
        self.scene_view = None;
        self.scene_retry_at = None;
        self.target_scene_view = None;
        self.scene_dirty = true;
        self.camera = self
            .saved_cameras
            .get(&dimension)
            .copied()
            .unwrap_or_else(|| {
                let mut camera = World3dCamera::default();
                if let Some(spawn) = self.spawn {
                    camera.position = spawn;
                }
                if dimension == Dimension::Nether {
                    camera.position[1] = 72.0;
                }
                camera
            });
        self.navigation.set_camera(self.camera);
        self.selection = None;
        self.target = None;
        self.scene_dirty = true;
        self.request_view_update(true, cx);
        self.publish_camera_snapshot(cx);
        self.status = SharedString::from(format!("切换到{}", dimension_label(dimension)));
        self.status_kind = StatusKind::Info;
        cx.notify();
    }

    fn locate_in_2d(&mut self, cx: &mut Context<Self>) {
        let Some(owner) = self.owner.as_ref() else {
            self.status = SharedString::from("当前 3D 编辑器没有关联的 2D 地图窗口");
            self.status_kind = StatusKind::Warning;
            cx.notify();
            return;
        };
        let (x, y, z) = if let Some(target) = self.target.as_ref() {
            (target.position[0], target.position[1], target.position[2])
        } else {
            (
                self.camera.position[0].floor() as i32,
                self.camera.position[1].floor() as i32,
                self.camera.position[2].floor() as i32,
            )
        };
        let dimension = self.dimension;
        let selection = self.selection;
        let located = owner.update(cx, |main, cx| {
            main.locate_world3d_in_map(dimension, [x, y, z], selection, cx)
        });
        if matches!(located, Ok(true)) {
            self.status = SharedString::from(format!("已在 2D 地图定位 X {x} · Y {y} · Z {z}"));
            self.status_kind = StatusKind::Success;
        } else {
            self.status = "2D 定位未完成；地图正在写入或文档已关闭".into();
            self.status_kind = StatusKind::Warning;
        }
        cx.notify();
    }

    pub(super) fn set_selection_from_document(
        &mut self,
        dimension: Dimension,
        selection: Option<World3dSelection>,
        cx: &mut Context<Self>,
    ) {
        if dimension != self.dimension {
            return;
        }
        if self.selection == selection {
            return;
        }
        self.selection_drag = None;
        if self.tool == World3dTool::Select {
            self.axis_drag = None;
        }
        self.selection = selection;
        self.update_target_scene();
        cx.notify();
    }

    fn publish_selection_to_2d(&self, cx: &mut Context<Self>) {
        let Some(owner) = self.owner.clone() else {
            return;
        };
        let selection = self.selection;
        let dimension = self.dimension;
        cx.defer(move |cx| {
            let _ = owner.update(cx, |main, cx| {
                main.set_world3d_selection_from_editor(dimension, selection, cx)
            });
        });
    }

    fn open_chunk_details_in_2d(&mut self, cx: &mut Context<Self>) {
        let Some(owner) = self.owner.as_ref() else {
            self.status = SharedString::from("当前 3D 编辑器没有关联的 2D 地图窗口");
            self.status_kind = StatusKind::Warning;
            cx.notify();
            return;
        };
        let Some(target) = self.target.as_ref() else {
            self.status = SharedString::from("没有可打开区块数据的 3D 目标方块");
            self.status_kind = StatusKind::Warning;
            cx.notify();
            return;
        };
        let chunk = target.chunk;
        let _ = owner.update(cx, |main, cx| {
            main.open_world3d_chunk_detail(chunk, cx);
        });
        self.status = SharedString::from(format!(
            "已在 2D 专业面板打开区块 {},{} 的方块实体/实体数据",
            chunk.x, chunk.z
        ));
        self.status_kind = StatusKind::Success;
        cx.notify();
    }

    // ---- raycasting and tools ---------------------------------------------------------

    fn raycast_at(&self, position: Point<Pixels>, window: &Window) -> Option<World3dTargetBlock> {
        let (left, top, width, height) = self.stage_rect(window);
        let local_x = (position.x / px(1.0)) - left;
        let local_y = (position.y / px(1.0)) - top;
        if local_x < 0.0 || local_y < 0.0 || local_x > width || local_y > height {
            return None;
        }
        let ndc_x = (local_x / width) * 2.0 - 1.0;
        let ndc_y = 1.0 - (local_y / height) * 2.0;
        let (origin, direction) = self.camera.ray(ndc_x, ndc_y, self.stage_aspect());
        self.raycast_world(origin, direction)
    }

    fn raycast_world(&self, origin: [f64; 3], direction: [f64; 3]) -> Option<World3dTargetBlock> {
        let (dimension, x, _section_y, z, radius) = self.current_view_key();
        raycast::cast(
            |pos| {
                if !super::chunk_service::inside_view(pos, dimension, x, z, radius) {
                    return None;
                }
                self.probes.get(&pos).map(Arc::as_ref)
            },
            self.dimension,
            origin,
            direction,
            f64::from(self.camera.view_distance).max(48.0),
        )
    }

    fn update_target_at(&mut self, position: Point<Pixels>, window: &Window) -> bool {
        let target = self.raycast_at(position, window);
        let outline_changed = target.as_ref().map(|target| target.position)
            != self.target.as_ref().map(|target| target.position);
        let changed = target != self.target;
        if changed {
            self.target = target;
            if outline_changed {
                self.update_target_scene();
            }
        }
        changed
    }

    fn primary_click(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        self.pointer_position = Some(position);
        if self.tool == World3dTool::Import
            && self
                .import_preview
                .as_ref()
                .is_some_and(|preview| preview.is_writing)
        {
            self.status = "粘贴正在写入，暂不能调整放置位置".into();
            self.status_kind = StatusKind::Warning;
            cx.notify();
            return;
        }
        if self.start_axis_drag(position, window) {
            cx.notify();
            return;
        }
        let Some(target) = self.raycast_at(position, window) else {
            if self.tool == World3dTool::Select {
                self.status = SharedString::from("未命中已加载方块");
                self.status_kind = StatusKind::Warning;
                cx.notify();
            }
            return;
        };
        match self.tool {
            World3dTool::Navigate => {
                self.status = SharedString::from(format!(
                    "目标 {} · {}, {}, {} · 面 {:?}",
                    target.block_name,
                    target.position[0],
                    target.position[1],
                    target.position[2],
                    target.normal
                ));
                self.status_kind = StatusKind::Info;
            }
            World3dTool::Select => {
                let previous = self.selection;
                self.selection = Some(World3dSelection::from_corners(
                    self.dimension,
                    target.position,
                    target.position,
                ));
                self.selection_drag = Some(SelectionDrag {
                    anchor: target.position,
                    last: target.position,
                    previous,
                });
                self.status = SharedString::from("拖动设置 3D 选区范围");
                self.status_kind = StatusKind::Info;
            }
            World3dTool::Import => {
                let position = target.face_adjacent();
                self.request_import_action(
                    World3dImportAction::MoveTo(
                        ChunkPos {
                            x: position[0].div_euclid(16),
                            z: position[2].div_euclid(16),
                            dimension: self.dimension,
                        },
                        position[1],
                    ),
                    cx,
                );
                self.status = SharedString::from("已请求将导入预览移到目标面；确认前不会写入世界");
                self.status_kind = StatusKind::Info;
            }
            World3dTool::Place => {
                let block = WORLD3D_BRUSHES[self.brush_index].block_name;
                let position = target.face_adjacent();
                self.request_edit(
                    World3dBlockEditKind::Fill {
                        block_name: block.to_string(),
                    },
                    position,
                    position,
                    cx,
                );
            }
            World3dTool::Fill => {
                if self.selection.is_some() {
                    self.fill_selection(cx);
                } else {
                    let block = WORLD3D_BRUSHES[self.brush_index].block_name.to_string();
                    self.request_edit(
                        World3dBlockEditKind::Fill { block_name: block },
                        target.position,
                        target.position,
                        cx,
                    );
                }
            }
            World3dTool::Clear => {
                if self.selection.is_some() {
                    self.clear_selection(cx);
                } else {
                    self.status = "请先使用选择工具建立选区，再清除".into();
                    self.status_kind = StatusKind::Warning;
                }
            }
        }
        self.target = Some(target);
        self.update_target_scene();
        cx.notify();
    }

    fn drag_selection_to(&mut self, position: Point<Pixels>, window: &Window) {
        let Some(drag) = self.selection_drag else {
            return;
        };
        let Some(target) = self.raycast_at(position, window) else {
            return;
        };
        if target.position == drag.last {
            return;
        }
        self.selection = Some(World3dSelection::from_corners(
            self.dimension,
            drag.anchor,
            target.position,
        ));
        self.selection_drag = Some(SelectionDrag {
            anchor: drag.anchor,
            last: target.position,
            previous: drag.previous,
        });
        self.update_target_scene();
    }

    fn fill_selection(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.selection else {
            self.status = SharedString::from("没有 3D 选区");
            self.status_kind = StatusKind::Warning;
            cx.notify();
            return;
        };
        let block = WORLD3D_BRUSHES[self.brush_index].block_name.to_string();
        self.request_edit(
            World3dBlockEditKind::Fill { block_name: block },
            selection.min,
            selection.max,
            cx,
        );
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.selection else {
            self.status = SharedString::from("没有 3D 选区");
            self.status_kind = StatusKind::Warning;
            cx.notify();
            return;
        };
        self.request_edit(
            World3dBlockEditKind::Clear,
            selection.min,
            selection.max,
            cx,
        );
    }

    fn request_edit(
        &mut self,
        kind: World3dBlockEditKind,
        min: [i32; 3],
        max: [i32; 3],
        cx: &mut Context<Self>,
    ) {
        if self.edit_busy {
            self.status = SharedString::from("已有 3D 编辑任务正在进行");
            self.status_kind = StatusKind::Warning;
            cx.notify();
            return;
        }
        let volume = World3dSelection {
            dimension: self.dimension,
            min,
            max,
        }
        .volume();
        if volume > WORLD3D_MAX_EDIT_BLOCKS as u64 {
            self.status = SharedString::from(format!(
                "编辑体积 {volume} 超过上限 {WORLD3D_MAX_EDIT_BLOCKS}"
            ));
            self.status_kind = StatusKind::Warning;
            cx.notify();
            return;
        }
        let Some(owner) = self.owner.clone() else {
            self.status = SharedString::from("没有关联的 2D 地图文档，无法写入世界");
            self.status_kind = StatusKind::Error;
            cx.notify();
            return;
        };
        self.edit_request_seq = self.edit_request_seq.saturating_add(1);
        let request = World3dBlockEditRequest {
            request_id: self.edit_request_seq,
            dimension: self.dimension,
            min: [min[0].min(max[0]), min[1].min(max[1]), min[2].min(max[2])],
            max: [min[0].max(max[0]), min[1].max(max[1]), min[2].max(max[2])],
            kind,
        };
        self.edit_busy = true;
        self.visibility_revision = self.visibility_revision.wrapping_add(1);
        self.update_section_visibility();
        self.status = SharedString::from("已提交世界写入任务，等待统一的 2D 历史系统确认");
        self.status_kind = StatusKind::Info;
        if owner
            .update(cx, |main, cx| {
                main.apply_world3d_edit_request(request, cx);
            })
            .is_err()
        {
            self.edit_busy = false;
            self.visibility_revision = self.visibility_revision.wrapping_add(1);
            self.update_section_visibility();
            self.status = "关联的 2D 文档已关闭，编辑未提交".into();
            self.status_kind = StatusKind::Error;
        }
        cx.notify();
    }

    pub(super) fn on_edit_finished(&mut self, outcome: World3dEditOutcome, cx: &mut Context<Self>) {
        if outcome.request_id != self.edit_request_seq {
            return;
        }
        self.edit_busy = false;
        self.visibility_revision = self.visibility_revision.wrapping_add(1);
        self.status = SharedString::from(outcome.message.clone());
        self.status_kind = if outcome.ok {
            StatusKind::Success
        } else {
            StatusKind::Error
        };
        if outcome.ok && !outcome.affected_chunks.is_empty() {
            self.invalidate_chunks_from_document(outcome.affected_chunks, cx);
        }
        cx.notify();
    }

    /// Invalidates streamed chunk data after a shared document write.
    pub(super) fn invalidate_chunks_from_document(
        &mut self,
        chunks: Vec<ChunkPos>,
        cx: &mut Context<Self>,
    ) {
        if chunks.is_empty() {
            return;
        }
        self.invalidated_visibility.extend(
            chunks.iter().copied().filter(|pos| pos.dimension == self.dimension),
        );
        self.visibility_revision = self.visibility_revision.wrapping_add(1);
        self.update_section_visibility();
        if let Some(service) = self.service.as_ref() {
            service.send(super::types::ChunkServiceCommand::Invalidate {
                generation: self.stream_generation,
                chunks: chunks.clone(),
            });
        }
        cx.notify();
    }

    fn on_closed(&mut self, cx: &mut Context<Self>) {
        unregister_world3d_editor(&self.world_key, self.session_id);
        if let Some(service) = self.service.as_mut() {
            service.shutdown();
        }
        self.poll_task.take();
        if let Some(owner) = self.owner.as_ref() {
            let owner = owner.clone();
            let key = self.world_key.clone();
            cx.defer(move |cx| {
                let _ = owner.update(cx, |main, cx| {
                    main.handle_world3d_editor_closed(&key, cx);
                });
            });
        }
    }

    // ---- rendering --------------------------------------------------------------------

    fn render_top_bar(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let title = SharedString::from(format!("{} · 3D 世界编辑器", self.world_name));
        let camera_summary = SharedString::from(format!(
            "{} · X {:.1} Y {:.1} Z {:.1} · UI 刷新 {:.0} 次/秒",
            dimension_label(self.dimension),
            self.camera.position[0],
            self.camera.position[1],
            self.camera.position[2],
            self.frame.fps.round()
        ));
        let mut color_buttons = div().flex().items_center().gap(px(4.0));
        for (dimension, label) in [
            (Dimension::Overworld, "主世界"),
            (Dimension::Nether, "下界"),
            (Dimension::End, "末地"),
        ] {
            let active = self.dimension == dimension;
            color_buttons =
                color_buttons.child(toolbar_button(colors, label, active).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, _window, cx| {
                        this.switch_dimension(dimension, cx);
                    }),
                ));
        }
        div()
            .h(px(TOP_BAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(12.0))
            .border_b_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w(px(0.0))
                    .flex_1()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(colors.text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(colors.text_muted)
                            .child(camera_summary),
                    ),
            )
            .child(color_buttons)
            .child(toolbar_button(colors, "在 2D 定位", false).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| this.locate_in_2d(cx)),
            ))
            .child(toolbar_button(colors, "复制选区", false).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| this.copy_selection_to_map(cx)),
            ))
            .child(toolbar_button(colors, "粘贴预览", false).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| this.start_chunk_paste(cx)),
            ))
            .child(toolbar_button(colors, "撤销", false).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    let Some(owner) = this.owner.as_ref() else {
                        return;
                    };
                    let _ = owner.update(cx, |main, cx| main.undo_map_edit(cx));
                    this.status = SharedString::from("已请求撤销（使用 2D 文档历史）");
                    this.status_kind = StatusKind::Info;
                    cx.notify();
                }),
            ))
            .child(toolbar_button(colors, "重做", false).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    let Some(owner) = this.owner.as_ref() else {
                        return;
                    };
                    let _ = owner.update(cx, |main, cx| main.redo_map_edit(cx));
                    this.status = SharedString::from("已请求重做（使用 2D 文档历史）");
                    this.status_kind = StatusKind::Info;
                    cx.notify();
                }),
            ))
            .child(
                toolbar_button(
                    colors,
                    if self.camera.projection == World3dProjectionKind::Perspective {
                        "透视"
                    } else {
                        "正交"
                    },
                    true,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.camera = this
                            .navigation
                            .update_camera(|camera| camera.toggle_projection());
                        this.publish_camera_snapshot(cx);
                        cx.notify();
                    }),
                ),
            )
            .child(
                toolbar_button(
                    colors,
                    if self.right_panel_open {
                        "收起面板"
                    } else {
                        "属性面板"
                    },
                    false,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.right_panel_open = !this.right_panel_open;
                        cx.notify();
                    }),
                ),
            )
    }

    fn render_left_rail(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let tools = [
            (World3dTool::Navigate, "移动"),
            (World3dTool::Select, "选择"),
            (World3dTool::Import, "导入"),
            (World3dTool::Place, "放置"),
            (World3dTool::Fill, "填充"),
            (World3dTool::Clear, "清除"),
        ];
        let mut rail = div()
            .w(px(LEFT_RAIL_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(5.0))
            .py(px(8.0))
            .px(px(6.0))
            .border_r_1()
            .border_color(colors.border)
            .bg(colors.surface);
        for (tool, label) in tools {
            if tool == World3dTool::Import && self.import_preview.is_none() {
                continue;
            }
            let active = self.tool == tool;
            rail = rail.child(
                div()
                    .h(px(44.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(1.0))
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .bg(if active {
                        colors.accent.alpha(0.18)
                    } else {
                        colors.surface
                    })
                    .border_1()
                    .border_color(if active { colors.accent } else { colors.border })
                    .text_size(px(10.5))
                    .text_color(if active {
                        colors.accent
                    } else {
                        colors.text_secondary
                    })
                    .hover(|style| style.bg(colors.surface_hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.finish_axis_drag(cx);
                            this.tool = tool;
                            this.selection_drag = None;
                            cx.notify();
                        }),
                    )
                    .child(label),
            );
        }
        rail.child(div().flex_1()).child(
            div()
                .text_size(px(9.5))
                .text_color(colors.text_muted)
                .child("F3 信息"),
        )
    }

    fn render_right_panel(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        let mut panel = div()
            .id("world3d-right-panel")
            .w(px(RIGHT_PANEL_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(10.0))
            .border_l_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .overflow_y_scroll()
            .text_size(px(11.0))
            .text_color(colors.text_secondary);
        panel = panel.child(section_title(colors, "相机"));
        panel = panel.child(info_row(
            colors,
            "位置",
            SharedString::from(format!(
                "{:.2}, {:.2}, {:.2}",
                self.camera.position[0], self.camera.position[1], self.camera.position[2]
            )),
        ));
        panel = panel.child(info_row(
            colors,
            "朝向",
            SharedString::from(format!(
                "yaw {:.1}° · pitch {:.1}°",
                self.camera.yaw.to_degrees(),
                self.camera.pitch.to_degrees()
            )),
        ));
        panel = panel.child(info_row(
            colors,
            "垂直视场",
            SharedString::from(format!("{:.0}°", self.camera.fov_y_radians.to_degrees())),
        ));
        panel = panel.child(info_row(
            colors,
            "速度",
            SharedString::from(format!("{:.1} 格/秒", self.camera.speed)),
        ));
        panel = panel.child(info_row(
            colors,
            "出生点",
            SharedString::from(if let Some(spawn) = self.spawn {
                format!(
                    "{:.1}, {:.1}, {:.1} · {}",
                    spawn[0], spawn[1], spawn[2], self.spawn_source
                )
            } else {
                format!("缺失（{}）", self.spawn_source)
            }),
        ));

        if let Some(preview) = self.import_preview.as_ref() {
            panel = panel.child(section_title(colors, "粘贴预览"));
            if preview.target_anchor.dimension == self.dimension {
                panel = panel.child(info_row(
                    colors,
                    "位置",
                    SharedString::from(format!(
                        "区块 {}, {}{}",
                        preview.target_anchor.x,
                        preview.target_anchor.z,
                        if preview.is_structure {
                            format!(" · Y {}", preview.origin_y)
                        } else {
                            String::new()
                        }
                    )),
                ));
                panel = panel.child(info_row(
                    colors,
                    if preview.is_structure {
                        "结构"
                    } else {
                        "区块"
                    },
                    SharedString::from(if preview.is_structure {
                        format!(
                            "{} × {} × {} 方块 · {}",
                            preview.size[0],
                            preview.height,
                            preview.size[1],
                            preview.transform.label()
                        )
                    } else {
                        format!(
                            "{} 个完整区块 · {}",
                            preview.chunk_count,
                            preview.transform.label()
                        )
                    }),
                ));
                panel = panel.child(
                    div()
                        .text_size(px(10.0))
                        .text_color(colors.text_muted)
                        .child("拖动红 X / 绿 Y / 蓝 Z 箭头；X/Z 吸附 16 格，Y 吸附 1 格。Esc 撤回拖拽；Enter 确认写入"),
                );
                let mut actions = div().flex().flex_wrap().gap(px(4.0));
                for (label, action) in [
                    ("X −16", World3dImportAction::Nudge(-1, 0)),
                    ("X +16", World3dImportAction::Nudge(1, 0)),
                    ("Z −16", World3dImportAction::Nudge(0, -1)),
                    ("Z +16", World3dImportAction::Nudge(0, 1)),
                    ("↺", World3dImportAction::Rotate(false)),
                    ("↻", World3dImportAction::Rotate(true)),
                    ("镜像 X", World3dImportAction::MirrorX),
                    ("镜像 Z", World3dImportAction::MirrorZ),
                ] {
                    actions = actions.child(toolbar_button(colors, label, false).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.request_import_action(action, cx);
                        }),
                    ));
                }
                if preview.is_structure {
                    for (label, action) in [
                        ("Y −1", World3dImportAction::Raise(-1)),
                        ("Y +1", World3dImportAction::Raise(1)),
                    ] {
                        actions =
                            actions.child(toolbar_button(colors, label, false).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.request_import_action(action, cx)
                                }),
                            ));
                    }
                }
                for (label, action) in [
                    ("确认写入", World3dImportAction::Confirm),
                    ("取消", World3dImportAction::Cancel),
                ] {
                    actions = actions.child(toolbar_button(colors, label, false).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.request_import_action(action, cx)
                        }),
                    ));
                }
                panel = panel.child(actions);
            } else {
                panel = panel.child(info_row(
                    colors,
                    "提示",
                    SharedString::from("切换到导入预览所在维度后可编辑"),
                ));
            }
        }

        panel = panel.child(section_title(colors, "目标方块"));
        if let Some(target) = self.target.as_ref() {
            panel = panel.child(info_row(
                colors,
                "方块",
                SharedString::from(target.block_name.clone()),
            ));
            panel = panel.child(info_row(
                colors,
                "坐标",
                SharedString::from(format!(
                    "{}, {}, {}",
                    target.position[0], target.position[1], target.position[2]
                )),
            ));
            panel = panel.child(info_row(
                colors,
                "命中面",
                SharedString::from(format!("{:?}", target.normal)),
            ));
            panel = panel.child(info_row(
                colors,
                "区块",
                SharedString::from(format!("{}, {}", target.chunk.x, target.chunk.z)),
            ));
            panel = panel.child(
                toolbar_button(colors, "在 2D 打开区块数据", false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| this.open_chunk_details_in_2d(cx)),
                ),
            );
        } else {
            panel = panel.child(info_row(
                colors,
                "方块",
                SharedString::from("将鼠标移到已加载区块上"),
            ));
        }
        panel = panel.child(section_title(colors, "3D 选区"));
        if let Some(selection) = self.selection {
            let size = selection.size();
            panel = panel.child(info_row(
                colors,
                "范围",
                SharedString::from(format!(
                    "{}x{}x{} · {} 方块",
                    size[0],
                    size[1],
                    size[2],
                    selection.volume()
                )),
            ));
            panel = panel.child(info_row(
                colors,
                "最小",
                SharedString::from(format!(
                    "{}, {}, {}",
                    selection.min[0], selection.min[1], selection.min[2]
                )),
            ));
            panel = panel.child(info_row(
                colors,
                "最大",
                SharedString::from(format!(
                    "{}, {}, {}",
                    selection.max[0], selection.max[1], selection.max[2]
                )),
            ));
            let mut selection_actions = div().flex().flex_wrap().gap(px(4.0));
            selection_actions = selection_actions
                .child(toolbar_button(colors, "填充", false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| this.fill_selection(cx)),
                ))
                .child(toolbar_button(colors, "清除", false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| this.clear_selection(cx)),
                ))
                .child(toolbar_button(colors, "复制含高度", false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| this.copy_selection_to_map(cx)),
                ))
                .child(toolbar_button(colors, "清空选区", false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.selection = None;
                        this.update_target_scene();
                        this.publish_selection_to_2d(cx);
                        cx.notify();
                    }),
                ));
            panel = panel.child(selection_actions);
        } else {
            panel = panel.child(info_row(
                colors,
                "提示",
                SharedString::from("选择工具下左键拖动建立选区"),
            ));
        }

        panel = panel.child(section_title(colors, "地图剪贴板"));
        panel = panel.child(info_row(
            colors,
            "区块",
            SharedString::from(format!("{} 个已复制", self.chunk_clipboard_count)),
        ));
        panel = panel.child(
            div()
                .text_size(px(10.0))
                .text_color(colors.text_muted)
                .child("3D 选区按 X/Y/Z 复制；完整区块仍按 2D 整列复制"),
        );
        panel = panel.child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.0))
                .child(toolbar_button(colors, "复制 3D 选区", false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| this.copy_selection_to_map(cx)),
                ))
                .child(toolbar_button(colors, "复制完整区块", false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| this.copy_chunks_to_map(cx)),
                ))
                .child(toolbar_button(colors, "粘贴预览", false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| this.start_chunk_paste(cx)),
                )),
        );

        panel = panel.child(section_title(colors, "画笔方块"));
        let mut brushes = div().flex().flex_wrap().gap(px(4.0));
        for (index, brush) in WORLD3D_BRUSHES.iter().enumerate() {
            let active = self.brush_index == index;
            brushes = brushes.child(
                div()
                    .px(px(6.0))
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .border_1()
                    .border_color(if active { colors.accent } else { colors.border })
                    .bg(if active {
                        colors.accent.alpha(0.18)
                    } else {
                        colors.surface_hover
                    })
                    .text_size(px(10.5))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.brush_index = index;
                            cx.notify();
                        }),
                    )
                    .child(brush.label),
            );
        }
        panel = panel.child(brushes);
        panel = panel.child(
            div()
                .text_size(px(10.0))
                .text_color(colors.text_muted)
                .child(format!(
                    "写入经 2D 文档事务与历史：{}",
                    if self.edit_busy {
                        "进行中"
                    } else {
                        "空闲"
                    }
                )),
        );

        panel = panel.child(self.render_rendering_controls(colors, cx));
        panel = panel.child(section_title(colors, "流式加载"));
        panel = panel.child(info_row(
            colors,
            "区块",
            SharedString::from(format!(
                "{} 已加载 · {} 缺失 · 半径 {}",
                self.worker_status.loaded_chunks,
                self.worker_status.missing_chunks,
                self.worker_status.radius
            )),
        ));
        let displayed_radius = self.pending_view_radius.unwrap_or(self.view_radius);
        let width = displayed_radius * 2 + 1;
        panel = panel.child(info_row(
            colors,
            "目标范围",
            SharedString::from(format!(
                "圆形 {} 列 · {} 方块宽",
                super::chunk_range::column_count(displayed_radius),
                width * 16
            )),
        ));
        panel = panel.child(info_row(
            colors,
            "渲染距离",
            SharedString::from(format!("{} Chunks", displayed_radius)),
        ));
        panel = panel.child(info_row(
            colors,
            "面数上限",
            SharedString::from(format!(
                "{} 面 · {} 三角形",
                self.worker_status.face_count,
                self.worker_status.face_count.saturating_mul(2)
            )),
        ));
        panel = panel.child(info_row(
            colors,
            "贴图",
            SharedString::from(format!(
                "{} 张 · {:.1} MiB",
                self.textures.len(),
                self.texture_bytes as f64 / 1048576.0
            )),
        ));
        let changed_view = cx.entity().downgrade();
        let committed_view = changed_view.clone();
        panel = panel.child(
            Slider::new(
                "world3d-view-radius",
                colors,
                1.0,
                WORLD3D_MAX_VIEW_RADIUS as f32,
                displayed_radius as f32,
                move |value, app| {
                    let next = value.round().clamp(1.0, WORLD3D_MAX_VIEW_RADIUS as f32) as i32;
                    let _ = changed_view.update(app, |this, cx| {
                        this.pending_view_radius = Some(next);
                        cx.notify();
                    });
                },
            )
            .width(px(270.0))
            .on_commit(move |value, app| {
                let next = value.round().clamp(1.0, WORLD3D_MAX_VIEW_RADIUS as f32) as i32;
                let _ = committed_view.update(app, |this, cx| {
                    this.set_view_radius(next, cx);
                });
            }),
        );
        panel = panel.child(
            div()
                .flex()
                .gap(px(4.0))
                .child(toolbar_button(colors, "视距 -", false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.set_view_radius(this.view_radius - 1, cx);
                    }),
                ))
                .child(toolbar_button(colors, "视距 +", false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.set_view_radius(this.view_radius + 1, cx);
                    }),
                )),
        );
        panel
    }

    fn render_hud(&self, colors: &ThemeColors) -> Div {
        if !self.hud_visible {
            return div();
        }
        let mut hud = div()
            .absolute()
            .top(px(10.0))
            .left(px(10.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(9.0))
            .py(px(7.0))
            .rounded(px(5.0))
            .bg(Hsla {
                a: 0.52,
                ..Hsla::black()
            })
            .text_size(px(11.0))
            .text_color(Hsla::white())
            .child(format!(
                "{} · {}",
                self.world_name,
                dimension_label(self.dimension)
            ))
            .child(format!(
                "XYZ {:.1} / {:.1} / {:.1}",
                self.camera.position[0], self.camera.position[1], self.camera.position[2]
            ))
            .child(format!(
                "Chunk {} {} · yaw {:.0}° pitch {:.0}°",
                (self.camera.position[0] / 16.0).floor() as i32,
                (self.camera.position[2] / 16.0).floor() as i32,
                self.camera.yaw.to_degrees(),
                self.camera.pitch.to_degrees()
            ));
        if let Some(target) = self.target.as_ref() {
            hud = hud.child(format!(
                "目标 {} · {},{},{}",
                target.block_name, target.position[0], target.position[1], target.position[2]
            ));
        }
        if let Some(selection) = self.selection {
            let size = selection.size();
            hud = hud.child(format!(
                "选区 {}x{}x{} · {} 方块",
                size[0],
                size[1],
                size[2],
                selection.volume()
            ));
        }
        hud.child(format!(
            "UI 刷新 {:.0} 次/秒 · {:.1} ms · {} 区块 · 缓存 {:.1} MiB",
            self.frame.fps.round(),
            self.frame.last_frame_ms,
            self.worker_status.loaded_chunks,
            self.worker_status.cache_bytes as f64 / 1048576.0
        ))
        .text_color(Hsla {
            a: 0.95,
            ..colors.text_primary
        })
    }

    fn sample_gpu_diagnostics(&mut self, now: Instant, window: &Window) {
        if !(self.hud_debug || super::chunk_service::trace_timings())
            || self
                .last_gpu_sample_at
                .is_some_and(|last| now.saturating_duration_since(last) < Duration::from_secs(1))
        {
            return;
        }
        self.last_gpu_sample_at = Some(now);
        let metrics = gpui::performance_metrics_snapshot();
        let window_id = window.window_handle().window_id().as_u64();
        let window_metrics = metrics
            .window_metrics
            .iter()
            .find(|sample| sample.window_id == window_id);
        let owner = gpui::gpu_owner_metrics_snapshot()
            .windows
            .into_iter()
            .find(|sample| sample.window_id == Some(window_id));
        let owner_stats = owner
            .as_ref()
            .filter(|sample| sample.sample_age_ms.is_some_and(|age| age < 1_000))
            .map(|sample| {
                gpu_submit_stats(
                    &gpui::gpu_owner_samples_since(sample.owner_id, 0),
                    sample.sample_age_ms,
                )
            });
        let memory = gpui::memory_profile_snapshot();
        let device = memory
            .devices
            .iter()
            .find(|device| device.adapter_name == metrics.gpu_adapter_name);
        self.gpu_diagnostics = format!(
            "GPU 实际提交 {} 次/秒 · {} / {} · {}",
            owner_stats
                .as_ref()
                .map_or_else(|| "待采样".to_string(), |stats| stats.submitted.to_string()),
            device.map_or("未知", |device| device.backend.as_str()),
            metrics.gpu_adapter_name,
            metrics.gpu_surface_present_mode
        );
        self.frame_diagnostics = owner.as_ref().zip(owner_stats.as_ref()).map_or_else(
            || "GPU 提交时序：待采样".to_string(),
            |(owner, stats)| {
                format!(
                    "GPU 间隔 p95 {:.1} ms · 任务 p95 {:.1} ms · 排队 p95 {:.1} ms · 延期 {} · 门控 {} · 合并 {}",
                    stats.interval_p95_us as f64 / 1000.0,
                    owner.owner_job_duration.p95_us as f64 / 1000.0,
                    owner.queue_wait.p95_us as f64 / 1000.0,
                    stats.deferred,
                    stats.gated,
                    stats.coalesced,
                )
            },
        );
        self.owner_stage_diagnostics = owner_stats.as_ref().map_or_else(
            || "Owner阶段：待采样".to_string(),
            |stats| {
                let Some(overall) = stats.p95_job.as_ref() else {
                    return "Owner阶段：待采样".to_string();
                };
                let paths = stats
                    .p95_by_kind
                    .iter()
                    .flatten()
                    .map(owner_path_summary)
                    .collect::<Vec<_>>()
                    .join(" · ");
                format!("{} · 路径 p95 {paths}", owner_phase_summary(overall))
            },
        );
        self.gpu_stage_diagnostics = window_metrics.map_or_else(
            || "3D 呈现阶段：待采样".to_string(),
            |sample| {
                let stage = &sample.extension_present;
                if stage.sample_count == 0 {
                    return "3D 呈现阶段：待采样".to_string();
                }
                format!(
                    "3D 帧 p95 总 {:.1} ms · 场景准备 {:.1} ms (扩展 {:.1} ms) · 提交预备 {:.1} ms · 保留资源 {:.1} ms · 帧上传 {:.1} ms · 后端呈现 {:.1} ms · 其他 {:.1} ms / {} 帧",
                    stage.total_p95_micros as f64 / 1000.0,
                    stage.scene_prepare_p95_micros as f64 / 1000.0,
                    stage.extension_prepare_p95_micros as f64 / 1000.0,
                    stage.submission_prepare_p95_micros as f64 / 1000.0,
                    stage.retained_prepare_p95_micros as f64 / 1000.0,
                    stage.frame_upload_p95_micros as f64 / 1000.0,
                    stage.backend_present_p95_micros as f64 / 1000.0,
                    stage.other_p95_micros as f64 / 1000.0,
                    stage.sample_count,
                )
            },
        );
        self.schedule_diagnostics = window_metrics.map_or_else(
            || "调度时序：待采样".to_string(),
            |sample| {
                format!(
                    "UI 绘制 p95 {:.1} ms · UI 帧计数间隔 p95 {:.1} ms · 唤醒 p95 {:.1} ms · DWM 周期 {:.1} ms · 窗口尝试 {}",
                    sample.frame_duration_p95_micros as f64 / 1000.0,
                    sample.present_interval_p95_micros as f64 / 1000.0,
                    sample.native_vsync.wake_interval_p95_micros as f64 / 1000.0,
                    sample.native_vsync.refresh_period_p50_micros as f64 / 1000.0,
                    sample.native_vsync.active_presentation_attempt_count,
                )
            },
        );
        let mesh_bytes = self.chunk_meshes.values().fold(0usize, |total, chunk| {
            total.saturating_add(chunk.mesh.geometry_storage_bytes())
        });
        let gpu_bytes = device.and_then(|device| device.allocated_bytes);
        self.memory_diagnostics = format!(
            "CPU 网格约 {:.1} MiB · 解码区块约 {:.1} MiB · 贴图像素约 {:.1} MiB · GPU 已分配 {} (设备)",
            mesh_bytes as f64 / 1048576.0,
            self.worker_status.cache_bytes as f64 / 1048576.0,
            self.worker_status.texture_bytes as f64 / 1048576.0,
            gpu_bytes.map_or_else(
                || "未知".to_string(),
                |bytes| format!("{:.1} MiB", bytes as f64 / 1048576.0)
            )
        );
        if super::chunk_service::trace_timings() {
            eprintln!(
                "world3d ui ts={} gen={} camera={:.1},{:.1},{:.1} yaw_deg={:.1} pitch_deg={:.1} radius={} refresh_fps={:.1} gpu_submits_1s={} ui_draw_p95_ms={:.1} ui_frame_gap_p95_ms={:.1} poll_count={} poll_max_ms={:.1} scene_build_count={} scene_build_max_ms={:.1} nodes={} lod_chunks={} cpu_mesh_mib={:.1} decoded_mib={:.1} gpu_mib={:.1}",
                super::chunk_service::trace_timestamp_ms(),
                self.stream_generation,
                self.camera.position[0],
                self.camera.position[1],
                self.camera.position[2],
                self.camera.yaw.to_degrees(),
                self.camera.pitch.to_degrees(),
                self.view_radius,
                self.frame.fps,
                owner_stats.as_ref().map_or(0, |stats| stats.submitted),
                window_metrics.map_or(0.0, |sample| sample.frame_duration_p95_micros as f64
                    / 1000.0),
                window_metrics.map_or(0.0, |sample| sample.present_interval_p95_micros as f64
                    / 1000.0),
                self.trace_poll_count,
                self.trace_poll_max_us as f64 / 1000.0,
                self.trace_scene_build_count,
                self.trace_scene_build_max_us as f64 / 1000.0,
                self.chunk_meshes.len(),
                self.worker_status.coarse_chunks,
                mesh_bytes as f64 / 1048576.0,
                self.worker_status.cache_bytes as f64 / 1048576.0,
                gpu_bytes.unwrap_or_default() as f64 / 1048576.0,
            );
            self.trace_poll_count = 0;
            self.trace_poll_max_us = 0;
            self.trace_scene_build_count = 0;
            self.trace_scene_build_max_us = 0;
        }
    }

    fn render_debug_hud(&self) -> Div {
        if !self.hud_debug {
            return div();
        }
        let mut panel = div()
            .absolute()
            .top(px(10.0))
            .right(px(10.0))
            .w(px(320.0))
            .flex()
            .flex_col()
            .gap(px(1.5))
            .px(px(9.0))
            .py(px(7.0))
            .rounded(px(5.0))
            .bg(Hsla {
                a: 0.72,
                ..Hsla::black()
            })
            .text_size(px(10.5))
            .text_color(Hsla::white())
            .child("F3 · 3D 世界编辑器诊断")
            .child(format!("配置后端: {} · 流式区块", self.backend_label))
            .child(self.gpu_diagnostics.clone())
            .child(self.frame_diagnostics.clone())
            .child(self.owner_stage_diagnostics.clone())
            .child(self.gpu_stage_diagnostics.clone())
            .child(self.schedule_diagnostics.clone())
            .child(self.memory_diagnostics.clone())
            .child(format!(
                "UI 刷新 {:.1} 次/秒 · 间隔 {:.2} ms · 场景节点 {} · 场景构建 {} ms",
                self.frame.fps,
                self.frame.average_frame_ms,
                self.chunk_meshes.len(),
                self.last_scene_build_ms,
            ))
            .child(format!(
                "区块 已加载 {} · 远景 LOD {} · 缺失 {} · 失败 {} · 面 {}",
                self.worker_status.loaded_chunks,
                self.worker_status.coarse_chunks,
                self.worker_status.missing_chunks,
                self.worker_status.failed_chunks,
                self.worker_status.face_count
            ))
            .child(format!(
                "解码区块缓存 {:.2} MiB · 贴图 {:.2} MiB / {} 张",
                self.worker_status.cache_bytes as f64 / 1048576.0,
                self.worker_status.texture_bytes as f64 / 1048576.0,
                self.worker_status.texture_count
            ))
            .child(format!(
                "资源包根 {} · 方块模型 {} · 解析错误 {}",
                self.worker_info.resource_pack_count,
                self.worker_info.block_model_count,
                self.worker_info.texture_resolver_errors
            ))
            .child(format!(
                "读取批次 {} ms · 世界查询 {} ms · 方块收集 {} ms",
                self.worker_status.last_chunk_ms,
                self.worker_status.last_query_ms,
                self.worker_status.last_collect_ms,
            ))
            .child(format!(
                "DB {} ms · 解码 {} ms · 键 {}/{} · 线程 {}",
                self.worker_status.last_load_stats.db_read_ms,
                self.worker_status.last_load_stats.decode_ms,
                self.worker_status.last_load_stats.keys_found,
                self.worker_status.last_load_stats.keys_requested,
                self.worker_status.last_load_stats.worker_threads,
            ))
            .child(format!(
                "数据准备 {} ms · 面提取 {} ms · 网格转换 {} ms / {} 目标区块",
                self.worker_status.last_stage_ms,
                self.worker_status.last_face_ms,
                self.worker_status.last_convert_ms,
                self.worker_status.last_mesh_targets,
            ))
            .child(format!(
                "维度 {} · 流代次 {} · 渲染原点 {}, {}",
                dimension_label(self.dimension),
                self.stream_generation,
                self.render_origin[0],
                self.render_origin[2]
            ));
        if let Some(error) = self.worker_status.last_error.as_deref() {
            panel = panel.child(format!("最近错误：{error}"));
        }
        panel
    }

    fn render_viewport(&self, colors: &ThemeColors) -> Div {
        let mut viewport = div()
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_hidden()
            .bg(self.render_sky_color());
        if let Some(scene_view) = self.scene_view.as_ref() {
            let camera = self.camera.gpu_camera(self.render_origin);
            let fog_color: Rgba = self.render_sky_color().into();
            let fog_end = (self.view_radius.max(1) * 16) as f32;
            let fog = gpui_3d::DistanceFog::new(
                [fog_color.r, fog_color.g, fog_color.b],
                fog_end * 0.65,
                fog_end,
            );
            let fog = if self.render_settings.fog { fog } else { None };
            let snapshot = scene_view
                .with_camera(camera)
                .with_fog(fog)
                .with_hidden_nodes(Arc::clone(&self.hidden_draw_nodes));
            let snapshot = match snapshot
                .with_voxel_shading(self.render_settings.shading_for_dimension(self.dimension))
            {
                Ok(view) => view,
                Err(error) => {
                    eprintln!("world3d shading: {error}");
                    snapshot
                }
            };
            let snapshot = snapshot
                .with_culling_bounds(Some(self.scene_culling_bounds()))
                .unwrap_or(snapshot);
            let mut settings = self.render_settings.effects(
                self.worker_info
                    .cloud_texture
                    .as_ref()
                    .map(|texture| texture.id()),
                self.render_origin,
                self.dimension == Dimension::Overworld,
            );
            if let Some(settings) = settings.as_mut() {
                let flipbook = |index: usize| {
                    self.worker_info.water_textures[index].as_ref().map(|v| {
                        gpui_3d::FlipbookTexture {
                            texture: v.asset.id(),
                            seconds_per_frame: v.seconds_per_frame,
                        }
                    })
                };
                settings.water_texture = flipbook(0);
                settings.water_flow_texture = flipbook(1);
            }
            if self.dimension != Dimension::Overworld
                && let Some(settings) = settings.as_mut()
            {
                settings.background_color = Some(super::rendering::linear_rgb([
                    fog_color.r,
                    fog_color.g,
                    fog_color.b,
                ]));
            }
            let snapshot = match snapshot.with_screen_effects(settings) {
                Ok(view) => view,
                Err(error) => {
                    eprintln!("world3d effects: {error}");
                    snapshot
                }
            };
            let navigation = self.navigation.clone();
            let snapshot =
                Arc::new(snapshot.with_camera_sampler(move |now| navigation.sample_camera(now)));
            viewport = viewport.child(
                div()
                    .absolute()
                    .inset_0()
                    .child(gpui_3d::scene_view(snapshot)),
            );
        } else {
            viewport = viewport.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.0))
                    .text_color(colors.text_muted)
                    .child("正在准备世界网格..."),
            );
        }
        if (self.target.is_some() || self.selection.is_some())
            && let Some(scene_view) = self.target_scene_view.as_ref()
        {
            let camera = self.camera.gpu_camera(self.render_origin);
            let navigation = self.navigation.clone();
            viewport = viewport.child(
                div()
                    .absolute()
                    .inset_0()
                    .child(gpui_3d::scene_view(Arc::new(
                        scene_view
                            .with_camera(camera)
                            .with_camera_sampler(move |now| navigation.sample_camera(now)),
                    ))),
            );
        }
        viewport
            .child(self.render_axis_gizmo())
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(crosshair()),
            )
            .child(self.render_hud(colors))
            .child(self.render_debug_hud())
            .child(
                div()
                    .absolute()
                    .bottom(px(8.0))
                    .right(px(10.0))
                    .px(px(7.0))
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .bg(Hsla {
                        a: 0.45,
                        ..Hsla::black()
                    })
                    .text_size(px(10.0))
                    .text_color(Hsla::white())
                    .child(status_hint(self.tool)),
            )
    }

    fn render_status_bar(&self, colors: &ThemeColors) -> Div {
        let status_color = match self.status_kind {
            StatusKind::Info => colors.text_muted,
            StatusKind::Success => colors.stat_green_text,
            StatusKind::Warning => colors.stat_orange_text,
            StatusKind::Error => colors.danger,
        };
        div()
            .h(px(STATUS_BAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(10.0))
            .border_t_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .text_size(px(10.5))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(status_color)
                    .child(self.status.clone()),
            )
            .child(div().text_color(colors.text_muted).child(format!(
                "{} · X {} Y {} Z {} · 区块 {} · {} 区块 · UI 刷新 {:.0} 次/秒",
                dimension_label(self.dimension),
                self.camera.position[0].floor() as i32,
                self.camera.position[1].floor() as i32,
                self.camera.position[2].floor() as i32,
                (self.camera.position[0] / 16.0).floor() as i32,
                self.worker_status.loaded_chunks,
                self.frame.fps.round()
            )))
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.to_ascii_lowercase();
        if key == "f3" {
            self.hud_debug = !self.hud_debug;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if key == "f1" {
            self.hud_visible = !self.hud_visible;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if key == "enter" && self.tool == World3dTool::Import && self.import_preview.is_some() {
            self.finish_axis_drag(cx);
            self.request_import_action(World3dImportAction::Confirm, cx);
            cx.stop_propagation();
            return;
        }
        if key == "escape" {
            if self.cancel_axis_drag(cx) {
                cx.stop_propagation();
                return;
            }
            self.look_drag = None;
            if let Some(drag) = self.selection_drag.take() {
                self.selection = drag.previous;
                self.update_target_scene();
            }
            self.navigation.clear_keys();
            if self.tool == World3dTool::Import && self.import_preview.is_some() {
                self.request_import_action(World3dImportAction::Cancel, cx);
            }
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if self.focus.is_focused(window)
            && self.axis_drag.is_none()
            && matches!(key.as_str(), "w" | "a" | "s" | "d" | "space")
            && self.navigation.set_key(&key, true)
        {
            cx.stop_propagation();
        }
    }

    fn handle_key_up(&mut self, event: &KeyUpEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.to_ascii_lowercase();
        if self.navigation.set_key(&key, false) {
            cx.stop_propagation();
        }
    }

    fn handle_modifiers(
        &mut self,
        event: &ModifiersChangedEvent,
        window: &Window,
        _cx: &mut Context<Self>,
    ) {
        if self.axis_drag.is_some() {
            return;
        }
        if !self.focus.is_focused(window) {
            return;
        }
        if event.modifiers.shift {
            self.navigation.set_key("shift", true);
        } else {
            self.navigation.set_key("shift", false);
        }
    }

    fn stop_navigation(&mut self, cx: &mut Context<Self>) {
        self.cancel_axis_drag(cx);
        self.navigation.clear_keys();
        self.look_drag = None;
        if self.selection_drag.take().is_some() {
            self.publish_selection_to_2d(cx);
        }
        cx.notify();
    }
}

impl Render for World3dEditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = window.animation_time();
        self.tick_lighting(now, window);
        self.frame.record(now);
        self.sample_gpu_diagnostics(now, window);
        self.update_stage_size(window);
        let pick_camera = self.camera_snapshot();
        let pick_pending = self.target_pick_dirty || self.last_pick_camera != Some(pick_camera);
        if pick_pending {
            if is_pick_due(self.last_pick_at, now) {
                self.last_pick_at = Some(now);
                self.target_pick_dirty = false;
                self.pointer_pick_dirty = false;
                self.last_pick_camera = Some(pick_camera);
                if let Some(position) = self.pointer_position {
                    self.update_target_at(position, window);
                }
            } else {
                window.request_animation_frame();
            }
        }
        let desired_origin = [
            (self.camera.position[0] / RENDER_ORIGIN_GRID).round() as i32
                * RENDER_ORIGIN_GRID as i32,
            0,
            (self.camera.position[2] / RENDER_ORIGIN_GRID).round() as i32
                * RENDER_ORIGIN_GRID as i32,
        ];
        if desired_origin != self.render_origin {
            self.scene_dirty = true;
        }
        if self.scene_dirty {
            self.ensure_scene(cx);
        }
        self.publish_camera_snapshot(cx);
        let colors = theme_colors(now, cx);
        let viewport = self.render_viewport(&colors);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .track_focus(&self.focus)
            .key_context("World3dEditor")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key_down(event, window, cx);
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _window, cx| {
                this.handle_key_up(event, cx);
            }))
            .on_modifiers_changed(
                cx.listener(|this, event: &ModifiersChangedEvent, window, cx| {
                    this.handle_modifiers(event, window, cx);
                }),
            )
            .child(self.render_top_bar(&colors, cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .child(if self.left_rail_open {
                        self.render_left_rail(&colors, cx)
                    } else {
                        div()
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                                    this.primary_click(event.position, window, cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                                    if this.axis_drag.is_some() || this.selection_drag.is_some() {
                                        return;
                                    }
                                    this.focus.focus(window, cx);
                                    this.look_drag = Some(LookDrag {
                                        last: event.position,
                                    });
                                    cx.stop_propagation();
                                }),
                            )
                            .on_mouse_move(cx.listener(
                                |this, event: &MouseMoveEvent, window, cx| {
                                    this.pointer_position = Some(event.position);
                                    if this.axis_drag.is_some() {
                                        if event.pressed_button == Some(MouseButton::Left) {
                                            this.drag_axis_to(event.position, cx);
                                        } else {
                                            this.finish_axis_drag(cx);
                                        }
                                        return;
                                    }
                                    if event.pressed_button.is_none() {
                                        this.look_drag = None;
                                        if this.selection_drag.take().is_some() {
                                            this.publish_selection_to_2d(cx);
                                        }
                                    }
                                    if let Some(drag) = this.look_drag {
                                        let dx = (event.position.x - drag.last.x) / px(1.0);
                                        let dy = (event.position.y - drag.last.y) / px(1.0);
                                        if dx.abs() + dy.abs() > 0.0 {
                                            this.camera = this.navigation.update_camera(|camera| {
                                                camera.rotate_look(dx * 0.0035, dy * 0.0035);
                                            });
                                            this.navigation_ui_dirty = true;
                                            this.look_drag = Some(LookDrag {
                                                last: event.position,
                                            });
                                        }
                                    }
                                    if this.selection_drag.is_some() {
                                        this.drag_selection_to(event.position, window);
                                        cx.notify();
                                    } else if event.pressed_button.is_none() {
                                        this.pointer_position = Some(event.position);
                                        this.target_pick_dirty = true;
                                        this.pointer_pick_dirty = true;
                                    }
                                },
                            ))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.finish_axis_drag(cx);
                                    let had_selection_drag = this.selection_drag.is_some();
                                    this.selection_drag = None;
                                    if had_selection_drag {
                                        this.publish_selection_to_2d(cx);
                                    }
                                    cx.notify();
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Right,
                                cx.listener(|this, _event, _window, _cx| {
                                    this.look_drag = None;
                                }),
                            )
                            .on_mouse_up_out(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.finish_axis_drag(cx);
                                    if this.selection_drag.take().is_some() {
                                        this.publish_selection_to_2d(cx);
                                    }
                                    cx.notify();
                                }),
                            )
                            .on_mouse_up_out(
                                MouseButton::Right,
                                cx.listener(|this, _event, _window, cx| {
                                    this.look_drag = None;
                                    cx.notify();
                                }),
                            )
                            .on_scroll_wheel(cx.listener(
                                |this, event: &ScrollWheelEvent, _window, cx| {
                                    if this.axis_drag.is_some() {
                                        return;
                                    }
                                    let delta = event.delta.pixel_delta(px(48.0));
                                    this.camera = this.navigation.update_camera(|camera| {
                                        if event.modifiers.alt {
                                            let direction =
                                                if delta.y > px(0.0) { 1.0 } else { -1.0 };
                                            camera.add_fov(direction * 2.0_f32.to_radians());
                                        } else if event.modifiers.shift {
                                            let factor = if delta.y > px(0.0) { 1.1 } else { 0.9 };
                                            camera.add_ortho_height(factor);
                                        } else {
                                            let factor =
                                                if delta.y > px(0.0) { 1.15 } else { 0.87 };
                                            camera.add_speed(factor);
                                        }
                                    });
                                    this.publish_camera_snapshot(cx);
                                    cx.notify();
                                },
                            ))
                            .child(viewport),
                    )
                    .when(self.right_panel_open, |this| {
                        this.child(self.render_right_panel(&colors, cx))
                    }),
            )
            .child(self.render_status_bar(&colors))
    }
}

// ---- small UI helpers -------------------------------------------------------------------

fn texture_storage_bytes(texture: &TextureAsset) -> usize {
    (0..texture.mip_level_count())
        .filter_map(|level| texture.mip_pixels(level))
        .map(<[u8]>::len)
        .sum()
}

fn restore_retained_sections(
    meshes: &mut BTreeMap<(ChunkPos, i32), Arc<World3dChunkMesh>>,
    changed: &mut BTreeSet<(ChunkPos, i32)>,
    textures: &mut HashMap<gpui_3d::TextureAssetId, Arc<TextureAsset>>,
    texture_bytes: &mut usize,
    retained: Vec<Arc<World3dChunkMesh>>,
) -> bool {
    let mut restored = false;
    for mesh in retained {
        let key = (mesh.pos, mesh.section_y);
        if meshes.contains_key(&key) {
            continue;
        }
        for texture in mesh.textures.iter() {
            if let std::collections::hash_map::Entry::Vacant(entry) = textures.entry(texture.id()) {
                *texture_bytes = texture_bytes.saturating_add(texture_storage_bytes(texture));
                entry.insert(texture.clone());
            }
        }
        meshes.insert(key, mesh);
        changed.insert(key);
        restored = true;
    }
    restored
}

fn retain_chunk_sections(
    meshes: &mut BTreeMap<(ChunkPos, i32), Arc<World3dChunkMesh>>,
    changed: &mut BTreeSet<(ChunkPos, i32)>,
    pos: ChunkPos,
    sections: &[i32],
) -> bool {
    let stale = meshes
        .range((pos, i32::MIN)..=(pos, i32::MAX))
        .filter_map(|(key, _)| (!sections.contains(&key.1)).then_some(*key))
        .collect::<Vec<_>>();
    for key in &stale {
        meshes.remove(key);
        changed.insert(*key);
    }
    !stale.is_empty()
}

fn retain_chunk_textures(
    meshes: &BTreeMap<(ChunkPos, i32), Arc<World3dChunkMesh>>,
    textures: &mut HashMap<gpui_3d::TextureAssetId, Arc<TextureAsset>>,
) -> usize {
    let used = meshes
        .values()
        .flat_map(|mesh| mesh.textures.iter().map(|texture| texture.id()))
        .collect::<HashSet<_>>();
    textures.retain(|id, _| used.contains(id));
    textures.values().fold(0usize, |bytes, texture| {
        bytes.saturating_add(texture_storage_bytes(texture))
    })
}

#[cfg(test)]
mod texture_retention_tests {
    use super::{
        Arc, BTreeMap, BTreeSet, ChunkPos, Dimension, GpuOwnerJobKind, GpuOwnerJobOutcome,
        GpuOwnerJobSample, HashMap, Mesh, TextureAsset, World3dChunkMesh, gpu_submit_stats,
        restore_retained_sections, retain_chunk_sections, retain_chunk_textures,
    };

    #[test]
    fn gpu_submit_stats_counts_recent_completed_gpu_work() {
        let sample = |completed_at_us, kind, outcome, coalesced_count| GpuOwnerJobSample {
            owner_id: 1,
            window_id: Some(1),
            job_id: 0,
            kind,
            outcome,
            started_at_us: completed_at_us,
            completed_at_us,
            queue_wait_us: Some(0),
            pending_age_us: Some(0),
            coalesced_count,
            owner_job_duration_us: completed_at_us,
            owner_blocking_wait_us: 0,
            phases: gpui::GpuOwnerJobPhases::default(),
            schedule_lateness_us: None,
        };
        let samples = [
            sample(
                900_000,
                GpuOwnerJobKind::Draw,
                GpuOwnerJobOutcome::Submitted,
                0,
            ),
            sample(
                1_500_000,
                GpuOwnerJobKind::Draw,
                GpuOwnerJobOutcome::Submitted,
                0,
            ),
            sample(
                1_508_000,
                GpuOwnerJobKind::Continue,
                GpuOwnerJobOutcome::Submitted,
                0,
            ),
            sample(
                1_516_000,
                GpuOwnerJobKind::Continue,
                GpuOwnerJobOutcome::Deferred,
                2,
            ),
            sample(
                2_000_000,
                GpuOwnerJobKind::Tick,
                GpuOwnerJobOutcome::Control,
                0,
            ),
        ];
        let recent = gpu_submit_stats(&samples, Some(300));
        assert_eq!(recent.submitted, 2);
        assert_eq!(recent.deferred, 1);
        assert_eq!(recent.gated, 1);
        assert_eq!(recent.coalesced, 2);
        assert_eq!(recent.interval_p95_us, 8_000);
        assert_eq!(
            recent.p95_job.as_ref().map(|job| job.completed_at_us),
            Some(2_000_000)
        );
        assert_eq!(
            recent.p95_by_kind[0]
                .as_ref()
                .map(|job| job.completed_at_us),
            Some(1_500_000)
        );
        assert_eq!(
            recent.p95_by_kind[1]
                .as_ref()
                .map(|job| job.completed_at_us),
            Some(1_516_000)
        );
        assert_eq!(
            recent.p95_by_kind[2]
                .as_ref()
                .map(|job| job.completed_at_us),
            Some(2_000_000)
        );
        assert_eq!(gpu_submit_stats(&samples, Some(1_000)).submitted, 0);
    }

    #[test]
    fn hover_pick_refresh_waits_for_pick_interval() {
        let last = std::time::Instant::now();
        assert!(super::is_pick_due(None, last));
        assert!(!super::is_pick_due(
            Some(last),
            last + super::PICK_INTERVAL - std::time::Duration::from_micros(1)
        ));
        assert!(super::is_pick_due(Some(last), last + super::PICK_INTERVAL));
    }

    #[test]
    fn streaming_scene_coalescing_has_fixed_latency() {
        assert_eq!(
            super::STREAM_SCENE_INTERVAL,
            std::time::Duration::from_millis(50)
        );
    }

    #[test]
    fn retained_sections_restore_missing_mesh_without_replacing_newer_mesh() {
        let texture = Arc::new(TextureAsset::rgba8(1, 1, Arc::<[u8]>::from([1, 2, 3, 4])).unwrap());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let old = Arc::new(World3dChunkMesh {
            occluders: Arc::from([]),
            pos,
            section_y: 0,
            origin: [0; 3],
            mesh: Arc::new(Mesh::cube()),
            materials: Arc::from([]),
            textures: Arc::from(vec![texture.clone()]),
            triangles: 0,
            faces: 0,
            missing_texture_faces: 0,
        });
        let newer = Arc::new(World3dChunkMesh {
            occluders: Arc::from([]),
            faces: 1,
            ..old.as_ref().clone()
        });
        let mut meshes = BTreeMap::new();
        let mut textures = HashMap::new();
        let mut changed = BTreeSet::new();
        let mut bytes = 0;
        assert!(restore_retained_sections(
            &mut meshes,
            &mut changed,
            &mut textures,
            &mut bytes,
            vec![old.clone()]
        ));
        assert!(Arc::ptr_eq(&meshes[&(pos, 0)], &old));
        assert_eq!(bytes, 4);
        assert!(changed.contains(&(pos, 0)));
        meshes.insert((pos, 0), newer.clone());
        assert!(!restore_retained_sections(
            &mut meshes,
            &mut changed,
            &mut textures,
            &mut bytes,
            vec![old]
        ));
        assert!(Arc::ptr_eq(&meshes[&(pos, 0)], &newer));
        assert_eq!(bytes, 4);
    }

    #[test]
    fn unloaded_chunk_textures_are_released() {
        let used = Arc::new(TextureAsset::rgba8(1, 1, Arc::<[u8]>::from([1, 2, 3, 4])).unwrap());
        let stale = Arc::new(TextureAsset::rgba8(1, 1, Arc::<[u8]>::from([5, 6, 7, 8])).unwrap());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let mesh = Arc::new(World3dChunkMesh {
            occluders: Arc::from([]),
            pos,
            section_y: 0,
            origin: [0; 3],
            mesh: Arc::new(Mesh::cube()),
            materials: Arc::from([]),
            textures: Arc::from(vec![used.clone()]),
            triangles: 0,
            faces: 0,
            missing_texture_faces: 0,
        });
        let mut meshes = BTreeMap::from([((pos, 0), mesh)]);
        let mut textures = HashMap::from([(used.id(), used.clone()), (stale.id(), stale.clone())]);

        assert_eq!(retain_chunk_textures(&meshes, &mut textures), 4);
        assert!(textures.contains_key(&used.id()));
        assert!(!textures.contains_key(&stale.id()));

        meshes.clear();
        assert_eq!(retain_chunk_textures(&meshes, &mut textures), 0);
        assert!(textures.is_empty());
    }

    #[test]
    fn completed_chunk_removes_only_sections_missing_from_new_mesh() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let neighbor = ChunkPos { x: 1, ..pos };
        let mesh = Arc::new(World3dChunkMesh {
            occluders: Arc::from([]),
            pos,
            section_y: 0,
            origin: [0; 3],
            mesh: Arc::new(Mesh::cube()),
            materials: Arc::from([]),
            textures: Arc::from([]),
            triangles: 0,
            faces: 0,
            missing_texture_faces: 0,
        });
        let mut meshes = BTreeMap::from([
            ((pos, 0), mesh.clone()),
            ((pos, 1), mesh.clone()),
            ((neighbor, 0), mesh),
        ]);
        let mut changed = BTreeSet::new();
        assert!(retain_chunk_sections(&mut meshes, &mut changed, pos, &[1]));
        assert_eq!(meshes.len(), 2);
        assert_eq!(changed, BTreeSet::from([(pos, 0)]));
        assert!(meshes.contains_key(&(pos, 1)));
        assert!(meshes.contains_key(&(neighbor, 0)));
        assert!(!retain_chunk_sections(&mut meshes, &mut changed, pos, &[1]));
    }
}

fn toolbar_button(colors: &ThemeColors, label: &str, active: bool) -> Div {
    div()
        .h(px(26.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .border_1()
        .border_color(if active { colors.accent } else { colors.border })
        .bg(if active {
            colors.accent.alpha(0.16)
        } else {
            colors.surface_hover
        })
        .text_size(px(10.5))
        .text_color(if active {
            colors.accent
        } else {
            colors.text_secondary
        })
        .hover(|style| style.bg(colors.surface_hover))
        .child(SharedString::from(label))
}

fn section_title(colors: &ThemeColors, label: &str) -> Div {
    div()
        .mt(px(3.0))
        .text_size(px(11.0))
        .text_color(colors.text_primary)
        .child(SharedString::from(label))
}

fn info_row(colors: &ThemeColors, label: &str, value: SharedString) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(8.0))
        .child(
            div()
                .w(px(52.0))
                .flex_none()
                .text_color(colors.text_muted)
                .child(SharedString::from(label)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_color(colors.text_secondary)
                .child(value),
        )
}

fn crosshair() -> Div {
    div()
        .relative()
        .w(px(14.0))
        .h(px(14.0))
        .child(
            div()
                .absolute()
                .left(px(6.0))
                .top(px(0.0))
                .w(px(1.5))
                .h(px(14.0))
                .bg(Hsla {
                    a: 0.75,
                    ..Hsla::white()
                }),
        )
        .child(
            div()
                .absolute()
                .left(px(0.0))
                .top(px(6.0))
                .w(px(14.0))
                .h(px(1.5))
                .bg(Hsla {
                    a: 0.75,
                    ..Hsla::white()
                }),
        )
}

fn status_hint(tool: World3dTool) -> SharedString {
    match tool {
        World3dTool::Navigate => {
            SharedString::from("WASD 移动 · Space/Shift 上下 · 右键旋转视角 · 滚轮调速")
        }
        World3dTool::Select => {
            SharedString::from("左键拖动建立选区 · 拖动 X/Y/Z 箭头拉伸边界 · Esc 撤回拖拽")
        }
        World3dTool::Import => {
            SharedString::from("点击地形定位 · 拖动 X/Y/Z 箭头调整 · Enter 确认 · Esc 取消")
        }
        World3dTool::Place => SharedString::from("左键在目标面放置画笔方块"),
        World3dTool::Fill => SharedString::from("左键填充选区 · 无选区时填充目标方块"),
        World3dTool::Clear => SharedString::from("左键清空选区为空气"),
    }
}

fn sky_color(dimension: Dimension) -> Hsla {
    match dimension {
        Dimension::Overworld => rgb(0x87b9e8).into(),
        Dimension::Nether => rgb(0x3a1410).into(),
        Dimension::End => rgb(0x17121f).into(),
        Dimension::Unknown(_) => rgb(0x202020).into(),
    }
}

fn theme_colors(now: Instant, cx: &App) -> ThemeColors {
    let theme = cx.global::<ThemeState>();
    lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        theme.factor(now),
        theme.accent,
    )
}

fn selection_material(alpha_mode: AlphaMode) -> Arc<Material> {
    let mut material = Material::new();
    material.shading_model = ShadingModel::Unlit;
    material.alpha_mode = alpha_mode;
    material.double_sided = true;
    material.base_color = [0.22, 0.62, 1.0, 0.24];
    Arc::new(material)
}

fn selection_outline_mesh(selection: World3dSelection) -> Option<Mesh> {
    let size = [
        (selection.max[0] - selection.min[0] + 1) as f32,
        (selection.max[1] - selection.min[1] + 1) as f32,
        (selection.max[2] - selection.min[2] + 1) as f32,
    ];
    unit_cube_outline(size)
}

fn block_outline_mesh() -> Option<Mesh> {
    unit_cube_outline([1.0, 1.0, 1.0])
}

fn unit_cube_outline(size: [f32; 3]) -> Option<Mesh> {
    let [sx, sy, sz] = size;
    let t = (sx.min(sy).min(sz) * 0.03).clamp(0.02, 0.14);
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut push_box = |min: [f32; 3], max: [f32; 3]| -> Option<()> {
        let base = u32::try_from(vertices.len()).ok()?;
        for position in [
            [min[0], min[1], min[2]],
            [max[0], min[1], min[2]],
            [max[0], min[1], max[2]],
            [min[0], min[1], max[2]],
            [min[0], max[1], min[2]],
            [max[0], max[1], min[2]],
            [max[0], max[1], max[2]],
            [min[0], max[1], max[2]],
        ] {
            vertices.push(gpui_3d::Vertex {
                position: Vec3::new(position[0], position[1], position[2]),
                normal: Vec3::Y,
                uv: gpui_3d::Vec2::ZERO,
                color: [1.0, 1.0, 1.0, 1.0],
            });
        }
        for [a, b, c, d] in [
            [0, 1, 2, 3],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [1, 2, 6, 5],
            [2, 3, 7, 6],
            [3, 0, 4, 7],
        ] {
            indices.extend([base + a, base + b, base + c, base + a, base + c, base + d]);
        }
        Some(())
    };
    let edges: [([f32; 3], [f32; 3]); 12] = [
        ([0.0, 0.0, 0.0], [sx, t, t]),
        ([sx - t, 0.0, 0.0], [sx, sy, t]),
        ([sx - t, sy - t, 0.0], [sx, sy, t]),
        ([0.0, sy - t, 0.0], [t, sy, t]),
        ([0.0, 0.0, sz - t], [sx, t, sz]),
        ([sx - t, 0.0, sz - t], [sx, sy, sz]),
        ([sx - t, sy - t, sz - t], [sx, sy, sz]),
        ([0.0, sy - t, sz - t], [t, sy, sz]),
        ([0.0, 0.0, 0.0], [t, t, sz]),
        ([sx - t, 0.0, 0.0], [sx, t, sz]),
        ([sx - t, sy - t, 0.0], [sx, sy, sz]),
        ([0.0, sy - t, 0.0], [t, sy, sz]),
    ];
    for (min, max) in edges {
        push_box(min, max)?;
    }
    Mesh::new(vertices, indices).ok()
}

fn next_session_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Drop for World3dEditorView {
    fn drop(&mut self) {
        unregister_world3d_editor(&self.world_key, self.session_id);
        if let Some(service) = self.service.as_mut() {
            service.shutdown();
        }
        self.poll_task.take();
    }
}

// ---- window entry ------------------------------------------------------------------------

/// Opens a new independent 3D world editor window.
pub fn open_world3d_editor_window(init: World3dEditorInit, cx: &mut App) {
    let title = SharedString::from(format!("3D 世界编辑器 · {}", init.world_name));
    let options = world3d_window_options(cx, title.clone());
    let slot: std::rc::Rc<
        std::cell::RefCell<Option<(World3dWorldKey, u64, WeakEntity<World3dEditorView>)>>,
    > = std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot_for_window = slot.clone();
    let result = cx.open_window(options, move |window, cx| {
        window.set_title(&title);
        window.activate_window();
        let view = cx.new(|cx| World3dEditorView::new(init, window, cx));
        let key = view.read(cx).world_key.clone();
        let session_id = view.read(cx).session_id;
        let weak = view.downgrade();
        *slot_for_window.borrow_mut() = Some((key.clone(), session_id, weak.clone()));
        let window_handle = window.any_window_handle();
        register_world3d_editor(
            key.clone(),
            World3dRegistryEntry {
                session_id,
                editor: weak.clone(),
                window: Some(window_handle),
                world_name: view.read(cx).world_name.clone(),
            },
        );
        set_world3d_editor_window(&key, window_handle);
        let close_view = weak.clone();
        window.on_window_should_close(cx, move |window, cx| {
            if let Some(view) = close_view.upgrade() {
                let _ = view.update(cx, |this, cx| this.on_closed(cx));
            }
            window.remove_window();
            true
        });
        cx.new(|cx| crate::ui::runtime::root_view::RootView::new(view, window, cx))
    });
    match result {
        Ok(handle) => {
            let any_handle: AnyWindowHandle = handle.into();
            if let Some((key, _session_id, weak)) = slot.borrow().clone() {
                set_world3d_editor_window(&key, any_handle);
                let _ = weak.update(cx, |editor, cx| {
                    editor.window_handle = Some(any_handle);
                    editor.publish_camera_snapshot(cx);
                });
            }
        }
        Err(error) => {
            eprintln!("Failed to open 3D world editor window: {error:?}");
        }
    }
}

fn world3d_window_options(cx: &mut App, title: SharedString) -> WindowOptions {
    let window_size = size(px(1360.0), px(860.0));
    let mut options = WindowOptions::default();
    options.window_bounds = Some(WindowBounds::centered(window_size, cx));
    options.window_min_size = Some(size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT)));
    options.is_resizable = true;
    options.is_minimizable = true;
    options.is_movable = true;
    #[cfg(windows)]
    {
        options.titlebar = Some(TitlebarOptions {
            title: Some(title),
            appears_transparent: false,
            ..Default::default()
        });
        options.window_background = WindowBackgroundAppearance::Opaque;
    }
    options
}
