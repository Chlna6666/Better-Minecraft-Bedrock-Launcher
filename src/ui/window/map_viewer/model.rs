use super::editor::*;
use super::helpers::*;
use super::panels::*;
use super::players::*;
use super::prelude::*;
use super::tile_state::*;
use super::viewport::*;

pub(super) const CHUNKS_PER_REGION: u32 = 32;
pub(super) const CHUNKS_PER_TILE: u32 = 8;
pub(super) const TILE_WORLD_BLOCKS: i32 = 128;
pub(super) const UI_BLOCKS_PER_PIXEL: u32 = 1;
pub(super) const UI_PIXELS_PER_BLOCK: u32 = 4;
pub(super) const DEFAULT_TILE_SIZE: f32 = 512.0;
pub(super) const PREFETCH_RADIUS: i32 = 0;
pub(super) const RETAIN_RADIUS: i32 = 1;
pub(super) const DRAG_RETAIN_RADIUS: i32 = 3;
pub(super) const DRAG_PREFETCH_RADIUS: i32 = 1;
pub(super) const DRAG_CANVAS_SYNC_INTERVAL: Duration = Duration::from_millis(8);
pub(super) const DRAG_VIEWPORT_TILE_SYNC_INTERVAL: Duration = Duration::from_millis(16);
pub(super) const DRAG_RENDER_IMAGE_EVICTION_DELAY: Duration = Duration::from_millis(400);
pub(super) const DRAG_RENDER_IMAGE_EVICTION_FLUSH_LIMIT: usize = 8;
pub(super) const MAX_TILE_SPAN_PER_AXIS: i32 = 64;
pub(super) const DEFAULT_CPU_PERCENT: u8 = 60;
pub(super) const MIN_CPU_PERCENT: u8 = 10;
pub(super) const MAX_CPU_PERCENT: u8 = 90;
pub(super) const CPU_PERCENT_STEP: u8 = 5;
pub(super) const RENDER_PIPELINE_DEPTH: usize = 32;
pub(super) const MIN_UI_TILE_MEMORY_BUDGET_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_UI_TILE_MEMORY_BUDGET_BYTES: usize = 160 * 1024 * 1024;
pub(super) const MIN_RENDER_MEMORY_BUDGET_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const MAX_RENDER_MEMORY_BUDGET_BYTES: u64 = 512 * 1024 * 1024;
pub(super) const MIN_RENDER_STAGING_POOL_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_RENDER_STAGING_POOL_BYTES: usize = 128 * 1024 * 1024;
pub(super) const RENDER_CPU_ENCODE_WORKERS: usize = 1;
pub(super) const RENDER_UI_BATCH_TILES: usize = 8;
pub(super) const MAX_CONCURRENT_RENDER_BATCHES: usize = 2;
pub(super) const RENDER_STREAM_GROUP_TILES: usize = 4;
pub(super) const TILE_MANIFEST_PROBE_BATCH_TILES: usize = 16;
pub(super) const MIN_VIEWPORT_SCALE: f32 = 0.03125;
pub(super) const MAX_VIEWPORT_SCALE: f32 = 8.0;
pub(super) const TILE_SEAM_BLEED_PX: f32 = 0.0;
pub(super) const TILE_READY_BATCH_LIMIT: usize = 16;
pub(super) const TILE_READY_BATCH_INTERVAL: Duration = Duration::from_millis(33);
pub(super) const CACHE_READY_BATCH_LIMIT: usize = 4;
pub(super) const CACHE_READY_BATCH_INTERVAL: Duration = Duration::from_millis(8);
pub(super) const FIRST_REVEAL_READY_BATCH_LIMIT: usize = 4;
pub(super) const FIRST_REVEAL_READY_BATCH_INTERVAL: Duration = Duration::from_millis(16);
pub(super) const FIRST_VISIBLE_BATCH_LIMIT: usize = 4;
pub(super) const OVERVIEW_VISIBLE_TILE_THRESHOLD: usize = 256;
pub(super) const OVERVIEW_VISIBLE_BATCH_LIMIT: usize = 24;
pub(super) const OVERVIEW_FIRST_VISIBLE_BATCH_LIMIT: usize = 16;

pub(super) type TileChunkPositions = Arc<[ChunkPos]>;
pub(super) type TileChunkIndex = BTreeMap<(i32, i32), TileChunkPositions>;
pub(super) const DRAG_VISIBLE_BATCH_LIMIT: usize = 4;
pub(super) const VIEWPORT_TILE_SYNC_INTERVAL: Duration = Duration::from_millis(80);
pub(super) const VISIBLE_TILE_LOG_INTERVAL: Duration = Duration::from_millis(250);
pub(super) const TILE_MEMORY_TRIM_INTERVAL: Duration = Duration::from_millis(250);
pub(super) const CHUNK_TRANSFER_FINISHED_RETENTION: Duration = Duration::from_secs(6);
pub(super) const MAP_CLICK_DRAG_THRESHOLD_PX: f32 = 4.0;

#[derive(Clone)]
pub struct MapViewerWindowInit {
    pub version: ManagedVersionEntry,
    pub asset: ManageAssetEntry,
    pub world_path: SharedString,
    pub initial_mode: RenderMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewerMode {
    Surface,
    Biome,
    Height,
    Layer,
    Cave,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RenderCpuBudget {
    pub(super) percent: u8,
}

impl Default for RenderCpuBudget {
    fn default() -> Self {
        Self {
            percent: DEFAULT_CPU_PERCENT,
        }
    }
}

impl RenderCpuBudget {
    pub(super) fn set_percent(&mut self, percent: u8) {
        self.percent = percent.clamp(MIN_CPU_PERCENT, MAX_CPU_PERCENT);
    }

    pub(super) fn step(&mut self, delta: i8) {
        let next = if delta.is_negative() {
            self.percent
                .saturating_sub(delta.unsigned_abs().saturating_mul(CPU_PERCENT_STEP))
        } else {
            self.percent
                .saturating_add((delta as u8).saturating_mul(CPU_PERCENT_STEP))
        };
        self.set_percent(next);
    }

    pub(super) fn available_threads() -> usize {
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .max(1)
    }

    pub(super) fn thread_count(self) -> usize {
        let available = Self::available_threads();
        let requested = available
            .saturating_mul(usize::from(self.percent))
            .saturating_add(99)
            / 100;
        let interactive_cap = available.saturating_sub(1).clamp(1, 8);
        requested.clamp(1, interactive_cap)
    }

    pub(super) fn tile_batch_size(self) -> usize {
        self.thread_count().saturating_mul(4).clamp(1, 64)
    }

    pub(super) fn render_threading(self, work_items: usize) -> RenderThreadingOptions {
        let threads = RenderThreadingOptions::Auto
            .resolve_for_profile_with_limits(
                RenderExecutionProfile::Interactive,
                work_items.max(1),
                Some(self.thread_count()),
                1,
            )
            .unwrap_or_else(|_| self.thread_count().min(work_items.max(1)).max(1));
        if threads <= 1 {
            RenderThreadingOptions::Single
        } else {
            RenderThreadingOptions::Fixed(threads)
        }
    }

    pub(super) fn render_cpu_pipeline(self, work_items: usize) -> RenderCpuPipelineOptions {
        let workers = self.thread_count().min(work_items.max(1)).max(1);
        RenderCpuPipelineOptions {
            queue_depth: workers
                .saturating_mul(2)
                .max(work_items.min(RENDER_PIPELINE_DEPTH).max(1)),
            chunk_batch_size: render_cpu_chunk_batch_size(workers).min(RENDER_STREAM_GROUP_TILES),
            encode_workers: RENDER_CPU_ENCODE_WORKERS,
            max_total_threads: workers,
            max_db_workers: workers.min(2).max(1),
            max_bake_workers: workers.min(2).max(1),
            max_compose_workers: workers.min(1),
            max_in_flight_regions: workers.min(2).max(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MapViewport {
    pub(super) offset_x: f32,
    pub(super) offset_y: f32,
    pub(super) scale: f32,
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) initialized: bool,
}

impl MapViewport {
    pub(super) fn new(window_size: Size<Pixels>) -> Self {
        let width = (window_size.width / px(1.0)).max(1.0);
        let height = (window_size.height / px(1.0)).max(1.0);
        Self {
            offset_x: width / 2.0,
            offset_y: height / 2.0,
            scale: 1.0,
            width,
            height,
            initialized: false,
        }
    }

    pub(super) fn set_size(&mut self, size: Size<Pixels>) -> bool {
        let width = (size.width / px(1.0)).max(1.0);
        let height = (size.height / px(1.0)).max(1.0);
        if (self.width - width).abs() < f32::EPSILON && (self.height - height).abs() < f32::EPSILON
        {
            return false;
        }
        let center_map_x = (self.width * 0.5 - self.offset_x) / self.scale;
        let center_map_z = (self.height * 0.5 - self.offset_y) / self.scale;
        self.width = width;
        self.height = height;
        self.offset_x = self.width * 0.5 - center_map_x * self.scale;
        self.offset_y = self.height * 0.5 - center_map_z * self.scale;
        true
    }

    pub(super) fn center_on_block(&mut self, block_x: i32, block_z: i32, layout: RenderLayout) {
        let map_x = block_to_map_pixel(block_x, layout);
        let map_z = block_to_map_pixel(block_z, layout);
        self.offset_x = self.width / 2.0 - map_x * self.scale;
        self.offset_y = self.height / 2.0 - map_z * self.scale;
        self.initialized = true;
    }

    pub(super) fn zoom_at(&mut self, position: Point<Pixels>, factor: f32) {
        let screen_x = position.x / px(1.0);
        let screen_y = position.y / px(1.0);
        let map_x = (screen_x - self.offset_x) / self.scale;
        let map_y = (screen_y - self.offset_y) / self.scale;
        self.scale = (self.scale * factor).clamp(MIN_VIEWPORT_SCALE, MAX_VIEWPORT_SCALE);
        self.offset_x = screen_x - map_x * self.scale;
        self.offset_y = screen_y - map_y * self.scale;
    }

    pub(super) fn screen_to_block(
        &self,
        position: Point<Pixels>,
        layout: RenderLayout,
    ) -> (i32, i32) {
        let map_x = ((position.x / px(1.0)) - self.offset_x) / self.scale;
        let map_z = ((position.y / px(1.0)) - self.offset_y) / self.scale;
        (
            map_pixel_to_block(map_x, layout),
            map_pixel_to_block(map_z, layout),
        )
    }

    pub(super) fn center_tile(&self, layout: RenderLayout) -> (i32, i32) {
        let tile_size = layout_tile_size(layout);
        let map_x = (self.width / 2.0 - self.offset_x) / self.scale;
        let map_z = (self.height / 2.0 - self.offset_y) / self.scale;
        (
            (map_x / tile_size).floor() as i32,
            (map_z / tile_size).floor() as i32,
        )
    }

    pub(super) fn screen_to_tile(
        &self,
        position: Point<Pixels>,
        layout: RenderLayout,
    ) -> (i32, i32) {
        let tile_size = layout_tile_size(layout);
        let map_x = ((position.x / px(1.0)) - self.offset_x) / self.scale;
        let map_z = ((position.y / px(1.0)) - self.offset_y) / self.scale;
        (
            (map_x / tile_size).floor() as i32,
            (map_z / tile_size).floor() as i32,
        )
    }

    pub(super) fn center_block(&self, layout: RenderLayout) -> (i32, i32) {
        let map_x = (self.width / 2.0 - self.offset_x) / self.scale;
        let map_z = (self.height / 2.0 - self.offset_y) / self.scale;
        (
            map_pixel_to_block(map_x, layout),
            map_pixel_to_block(map_z, layout),
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct DragState {
    pub(super) start: Point<Pixels>,
    pub(super) offset_x: f32,
    pub(super) offset_y: f32,
    pub(super) moved: bool,
    pub(super) last_position: Point<Pixels>,
    pub(super) last_movement_x: f32,
    pub(super) last_movement_y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PastePreviewAutoPan {
    pub(super) velocity_x: f32,
    pub(super) velocity_y: f32,
    pub(super) local_position: Point<Pixels>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct PointerCaptureRelease {
    pub(super) map_drag: bool,
    pub(super) right_selection: bool,
    pub(super) preview_3d_drag: bool,
    pub(super) dock_drag: bool,
}

impl PointerCaptureRelease {
    pub(super) const fn changed(self) -> bool {
        self.map_drag || self.right_selection || self.preview_3d_drag || self.dock_drag
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CanvasPointerMoveAction {
    UpdateMapPointer,
    UpdateRightSelection,
    Ignore,
    ReleaseStaleCaptures,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ContextMenuState {
    pub(super) position: Point<Pixels>,
    pub(super) block_x: i32,
    pub(super) block_z: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Marker {
    pub(super) x: i32,
    pub(super) z: i32,
    pub(super) label: SharedString,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct OverlayOptions {
    pub(super) axis: bool,
    pub(super) dense_grid: bool,
    pub(super) ruler: bool,
    pub(super) slime_chunks: bool,
    pub(super) entities: bool,
    pub(super) block_entities: bool,
    pub(super) villages: bool,
    pub(super) hardcoded_spawn_areas: bool,
}

impl Default for OverlayOptions {
    fn default() -> Self {
        Self {
            axis: true,
            dense_grid: false,
            ruler: true,
            slime_chunks: false,
            entities: false,
            block_entities: false,
            villages: false,
            hardcoded_spawn_areas: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SlimeQueryWindowSize {
    Three,
    Five,
    Seven,
}

impl Default for SlimeQueryWindowSize {
    fn default() -> Self {
        Self::Three
    }
}

impl SlimeQueryWindowSize {
    pub(super) const fn value(self) -> u8 {
        match self {
            Self::Three => 3,
            Self::Five => 5,
            Self::Seven => 7,
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Three => "3x3",
            Self::Five => "5x5",
            Self::Seven => "7x7",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum ProfessionalDetail {
    BlockTip {
        title: SharedString,
        json: SharedString,
    },
    Chunk {
        title: SharedString,
        json: SharedString,
    },
    Selection {
        title: SharedString,
        json: SharedString,
    },
    Editor {
        target: EditTarget,
        title: SharedString,
        sections: Vec<EditSection>,
        json: SharedString,
    },
}

impl ProfessionalDetail {
    pub(super) fn title(&self) -> SharedString {
        match self {
            Self::BlockTip { title, .. }
            | Self::Chunk { title, .. }
            | Self::Selection { title, .. }
            | Self::Editor { title, .. } => title.clone(),
        }
    }

    pub(super) fn json(&self) -> SharedString {
        match self {
            Self::BlockTip { json, .. }
            | Self::Chunk { json, .. }
            | Self::Selection { json, .. }
            | Self::Editor { json, .. } => json.clone(),
        }
    }

    pub(super) fn editor_sections(&self) -> Option<&[EditSection]> {
        match self {
            Self::Editor { sections, .. } => Some(sections),
            Self::BlockTip { .. } | Self::Chunk { .. } | Self::Selection { .. } => None,
        }
    }

    pub(super) fn edit_target(&self) -> Option<EditTarget> {
        match self {
            Self::Editor { target, .. } => Some(target.clone()),
            Self::BlockTip { .. } | Self::Chunk { .. } | Self::Selection { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum EditTarget {
    MapRecord(MapRecordId),
    GlobalRecord(GlobalRecordKind),
    Player(PlayerId),
    HsaChunk(ChunkPos),
    BlockEntities(ChunkPos),
    BlockEntityAt { chunk: ChunkPos, block: BlockPos },
    Actors(ChunkPos),
    HeightMap(ChunkPos),
    BiomeStorage(ChunkPos),
}

impl EditTarget {
    pub(super) fn operation_label(&self) -> String {
        match self {
            Self::MapRecord(id) => format!("edit map record {}", id.as_str()),
            Self::GlobalRecord(kind) => format!("edit global record {}", global_kind_label(kind)),
            Self::Player(id) => format!("edit player {}", player_id_label(id)),
            Self::HsaChunk(pos) => format!("edit HSA chunk {},{}", pos.x, pos.z),
            Self::BlockEntities(pos) => format!("edit block entities chunk {},{}", pos.x, pos.z),
            Self::BlockEntityAt { block, .. } => {
                format!("edit block entity {},{},{}", block.x, block.y, block.z)
            }
            Self::Actors(pos) => format!("edit actors chunk {},{}", pos.x, pos.z),
            Self::HeightMap(pos) => format!("edit heightmap chunk {},{}", pos.x, pos.z),
            Self::BiomeStorage(pos) => format!("edit biome storage chunk {},{}", pos.x, pos.z),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct EditSection {
    pub(super) title: SharedString,
    pub(super) rows: Vec<EditRow>,
}

#[derive(Clone, Debug)]
pub(super) struct EditRow {
    pub(super) label: SharedString,
    pub(super) value: SharedString,
    pub(super) editable: bool,
}

#[derive(Clone, Debug)]
pub(super) struct PendingEditConfirmation {
    pub(super) target: EditTarget,
    pub(super) action: EditAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum EditAction {
    Save,
    Delete,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum PasteRotation {
    NoRotation,
    Clockwise90,
    Rotate180,
    CounterClockwise90,
}

impl PasteRotation {
    pub(super) const ALL: [Self; 4] = [
        Self::NoRotation,
        Self::Clockwise90,
        Self::Rotate180,
        Self::CounterClockwise90,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::NoRotation => "不旋转",
            Self::Clockwise90 => "顺时针 90 度",
            Self::Rotate180 => "旋转 180 度",
            Self::CounterClockwise90 => "逆时针 90 度",
        }
    }

    pub(super) const fn rotate_delta(self, delta_x: i32, delta_z: i32) -> (i32, i32) {
        match self {
            Self::NoRotation => (delta_x, delta_z),
            Self::Clockwise90 => (-delta_z, delta_x),
            Self::Rotate180 => (-delta_x, -delta_z),
            Self::CounterClockwise90 => (delta_z, -delta_x),
        }
    }

    pub(super) const fn rotate_clockwise(self) -> Self {
        match self {
            Self::NoRotation => Self::Clockwise90,
            Self::Clockwise90 => Self::Rotate180,
            Self::Rotate180 => Self::CounterClockwise90,
            Self::CounterClockwise90 => Self::NoRotation,
        }
    }

    pub(super) const fn rotate_counter_clockwise(self) -> Self {
        match self {
            Self::NoRotation => Self::CounterClockwise90,
            Self::CounterClockwise90 => Self::Rotate180,
            Self::Rotate180 => Self::Clockwise90,
            Self::Clockwise90 => Self::NoRotation,
        }
    }

    pub(super) const fn is_default(self) -> bool {
        matches!(self, Self::NoRotation)
    }
}

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub(super) struct PasteTransform {
    pub(super) rotation: PasteRotation,
    pub(super) mirror_x: bool,
    pub(super) mirror_z: bool,
}

impl PasteTransform {
    pub(super) const fn from_rotation(rotation: PasteRotation) -> Self {
        Self {
            rotation,
            mirror_x: false,
            mirror_z: false,
        }
    }

    pub(super) const fn is_default(self) -> bool {
        self.rotation.is_default() && !self.mirror_x && !self.mirror_z
    }

    pub(super) const fn transform_chunk_delta(self, delta_x: i32, delta_z: i32) -> (i32, i32) {
        let delta_x = if self.mirror_x {
            delta_x.saturating_neg()
        } else {
            delta_x
        };
        let delta_z = if self.mirror_z {
            delta_z.saturating_neg()
        } else {
            delta_z
        };
        self.rotation.rotate_delta(delta_x, delta_z)
    }

    pub(super) const fn rotate_clockwise(self) -> Self {
        Self {
            rotation: self.rotation.rotate_clockwise(),
            ..self
        }
    }

    pub(super) const fn rotate_counter_clockwise(self) -> Self {
        Self {
            rotation: self.rotation.rotate_counter_clockwise(),
            ..self
        }
    }

    pub(super) const fn toggle_mirror_x(self) -> Self {
        Self {
            mirror_x: !self.mirror_x,
            ..self
        }
    }

    pub(super) const fn toggle_mirror_z(self) -> Self {
        Self {
            mirror_z: !self.mirror_z,
            ..self
        }
    }

    pub(super) fn label(self) -> String {
        let mut parts = Vec::new();
        if !self.rotation.is_default() {
            parts.push(self.rotation.label());
        }
        if self.mirror_x {
            parts.push("镜像 X");
        }
        if self.mirror_z {
            parts.push("镜像 Z");
        }
        if parts.is_empty() {
            "不变换".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

impl From<PasteRotation> for PasteTransform {
    fn from(rotation: PasteRotation) -> Self {
        Self::from_rotation(rotation)
    }
}

impl Default for PasteRotation {
    fn default() -> Self {
        Self::NoRotation
    }
}

pub(super) fn snapped_paste_rotation(display_degrees: f32) -> PasteRotation {
    let normalized = display_degrees.rem_euclid(360.0);
    if normalized < 45.0 || normalized > 315.0 {
        PasteRotation::NoRotation
    } else if normalized < 135.0 {
        PasteRotation::Clockwise90
    } else if normalized < 225.0 {
        PasteRotation::Rotate180
    } else {
        PasteRotation::CounterClockwise90
    }
}

pub(super) const fn paste_rotation_degrees(rotation: PasteRotation) -> f32 {
    match rotation {
        PasteRotation::NoRotation => 0.0,
        PasteRotation::Clockwise90 => 90.0,
        PasteRotation::Rotate180 => 180.0,
        PasteRotation::CounterClockwise90 => 270.0,
    }
}

pub(super) fn paste_rotation_radians(rotation: PasteRotation) -> f32 {
    paste_rotation_degrees(rotation).to_radians()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum QuickWriteAction {
    DeleteCurrentChunk(ChunkPos),
    ResetCurrentChunk(ChunkPos),
    DeleteCurrentChunkBlockEntities(ChunkPos),
    DeleteCurrentChunkActors(ChunkPos),
    PasteCopiedChunk {
        source: ChunkPos,
        target: ChunkPos,
        transform: PasteTransform,
    },
    PasteCopiedChunks {
        source_anchor: ChunkPos,
        target_anchor: ChunkPos,
        chunk_count: usize,
        transform: PasteTransform,
    },
    PasteImportedStructure {
        source_anchor: ChunkPos,
        target_anchor: ChunkPos,
        chunk_count: usize,
        transform: PasteTransform,
    },
}

impl QuickWriteAction {
    pub(super) fn label(&self) -> String {
        match self {
            Self::DeleteCurrentChunk(chunk) => {
                format!("删除当前 chunk {},{}", chunk.x, chunk.z)
            }
            Self::ResetCurrentChunk(chunk) => {
                format!("重置当前 chunk {},{}", chunk.x, chunk.z)
            }
            Self::DeleteCurrentChunkBlockEntities(chunk) => {
                format!("删除 chunk {},{} 方块实体", chunk.x, chunk.z)
            }
            Self::DeleteCurrentChunkActors(chunk) => {
                format!("删除 chunk {},{} 实体", chunk.x, chunk.z)
            }
            Self::PasteCopiedChunk {
                source,
                target,
                transform,
            } => {
                if transform.is_default() {
                    format!(
                        "粘贴 chunk {},{} 到 {},{}",
                        source.x, source.z, target.x, target.z
                    )
                } else {
                    format!(
                        "粘贴 chunk {},{} 到 {},{}（{}）",
                        source.x,
                        source.z,
                        target.x,
                        target.z,
                        transform.label()
                    )
                }
            }
            Self::PasteCopiedChunks {
                source_anchor,
                target_anchor,
                chunk_count,
                transform,
            } => {
                if transform.is_default() {
                    format!(
                        "粘贴 {chunk_count} 个 chunk（{},{} 到 {},{}）",
                        source_anchor.x, source_anchor.z, target_anchor.x, target_anchor.z
                    )
                } else {
                    format!(
                        "粘贴 {chunk_count} 个 chunk（{},{} 到 {},{}，{}）",
                        source_anchor.x,
                        source_anchor.z,
                        target_anchor.x,
                        target_anchor.z,
                        transform.label()
                    )
                }
            }
            Self::PasteImportedStructure {
                source_anchor,
                target_anchor,
                chunk_count,
                transform,
            } => {
                if transform.is_default() {
                    format!(
                        "粘贴结构 {chunk_count} 个 chunk（{},{} 到 {},{}）",
                        source_anchor.x, source_anchor.z, target_anchor.x, target_anchor.z
                    )
                } else {
                    format!(
                        "粘贴结构 {chunk_count} 个 chunk（{},{} 到 {},{}，{}）",
                        source_anchor.x,
                        source_anchor.z,
                        target_anchor.x,
                        target_anchor.z,
                        transform.label()
                    )
                }
            }
        }
    }

    pub(super) const fn chunk(&self) -> ChunkPos {
        match self {
            Self::DeleteCurrentChunk(chunk)
            | Self::ResetCurrentChunk(chunk)
            | Self::DeleteCurrentChunkBlockEntities(chunk)
            | Self::DeleteCurrentChunkActors(chunk) => *chunk,
            Self::PasteCopiedChunk { target, .. }
            | Self::PasteCopiedChunks {
                target_anchor: target,
                ..
            }
            | Self::PasteImportedStructure {
                target_anchor: target,
                ..
            } => *target,
        }
    }

    pub(super) const fn progress_seed(&self) -> Option<(&'static str, usize)> {
        match self {
            Self::DeleteCurrentChunk(_) => Some(("删除区块", 1)),
            Self::ResetCurrentChunk(_) => Some(("重置区块", 1)),
            Self::DeleteCurrentChunkBlockEntities(_) => Some(("删除方块实体", 1)),
            Self::DeleteCurrentChunkActors(_) => Some(("删除实体", 1)),
            Self::PasteCopiedChunks { chunk_count, .. } => Some(("粘贴区块", *chunk_count)),
            Self::PasteImportedStructure { chunk_count, .. } => Some(("粘贴结构", *chunk_count)),
            Self::PasteCopiedChunk { .. } => Some(("粘贴区块", 1)),
        }
    }

    pub(super) const fn is_paste(&self) -> bool {
        matches!(
            self,
            Self::PasteCopiedChunk { .. }
                | Self::PasteCopiedChunks { .. }
                | Self::PasteImportedStructure { .. }
        )
    }

    pub(super) const fn prioritizes_tile_refresh(&self) -> bool {
        matches!(
            self,
            Self::DeleteCurrentChunk(_)
                | Self::ResetCurrentChunk(_)
                | Self::PasteCopiedChunk { .. }
                | Self::PasteCopiedChunks { .. }
                | Self::PasteImportedStructure { .. }
        )
    }

    pub(super) const fn reuses_known_tile_index_after_write(&self) -> bool {
        matches!(
            self,
            Self::DeleteCurrentChunkBlockEntities(_) | Self::DeleteCurrentChunkActors(_)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ChunkTransferProgress {
    pub(super) phase: SharedString,
    pub(super) completed: usize,
    pub(super) total: usize,
}

impl ChunkTransferProgress {
    pub(super) fn label(&self) -> SharedString {
        SharedString::from(format!("{} {}/{}", self.phase, self.completed, self.total))
    }

    pub(super) fn ratio(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.completed as f32 / self.total as f32).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Debug)]
pub(super) struct CopiedChunkSnapshot {
    pub(super) chunk: ChunkPos,
    pub(super) records: Vec<ChunkRecord>,
    pub(super) block_entities: Vec<ParsedBlockEntity>,
    pub(super) hardcoded_spawn_areas: Vec<ParsedHardcodedSpawnArea>,
}

#[derive(Clone, Debug)]
pub(super) struct CopiedChunkData {
    pub(super) source: ChunkPos,
    pub(super) chunks: Vec<CopiedChunkSnapshot>,
}

impl CopiedChunkData {
    pub(super) fn from_single_chunk(
        source: ChunkPos,
        records: Vec<ChunkRecord>,
        block_entities: Vec<ParsedBlockEntity>,
        hardcoded_spawn_areas: Vec<ParsedHardcodedSpawnArea>,
    ) -> Self {
        Self {
            source,
            chunks: vec![CopiedChunkSnapshot {
                chunk: source,
                records,
                block_entities,
                hardcoded_spawn_areas,
            }],
        }
    }

    pub(super) const fn anchor_chunk(&self) -> ChunkPos {
        self.source
    }

    pub(super) fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
}

#[derive(Clone, Debug)]
pub(super) struct ImportedStructureData {
    pub(super) structure: Arc<bedrock_world::McStructureFile>,
    pub(super) source_anchor: ChunkPos,
    pub(super) origin_y: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PastePreview {
    pub(super) source_anchor: ChunkPos,
    pub(super) target_anchor: ChunkPos,
    pub(super) rotation: PasteRotation,
    pub(super) transform: PasteTransform,
    pub(super) display_degrees: f32,
    pub(super) drag: Option<PastePreviewDrag>,
    pub(super) targets: Vec<ChunkPos>,
    pub(super) tools_expanded: bool,
    pub(super) auto_pan: Option<PastePreviewAutoPan>,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum PastePreviewDrag {
    Move,
}

#[derive(Clone)]
pub(super) struct PastePreviewImage {
    pub(super) target: ChunkPos,
    pub(super) image: Arc<RenderImage>,
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Clone)]
pub(super) struct CopiedChunkPreviewImage {
    pub(super) chunk: ChunkPos,
    pub(super) image: Arc<RenderImage>,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl std::fmt::Debug for CopiedChunkPreviewImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CopiedChunkPreviewImage")
            .field("chunk", &self.chunk)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ManifestProbeDiagnostics {
    pub(super) last_edit_serial: u64,
    pub(super) last_edit_label: SharedString,
    pub(super) probe_starts_since_last_edit: u64,
    pub(super) recent_events: Vec<SharedString>,
}

impl Default for ManifestProbeDiagnostics {
    fn default() -> Self {
        Self {
            last_edit_serial: 0,
            last_edit_label: SharedString::from("无"),
            probe_starts_since_last_edit: 0,
            recent_events: Vec::new(),
        }
    }
}

impl ManifestProbeDiagnostics {
    pub(super) fn record_edit(&mut self, label: impl Into<String>) {
        self.last_edit_serial = self.last_edit_serial.saturating_add(1);
        self.last_edit_label = SharedString::from(label.into());
        self.probe_starts_since_last_edit = 0;
        self.push_event(format!(
            "edit #{} {}",
            self.last_edit_serial, self.last_edit_label
        ));
    }

    pub(super) fn record_probe_start(&mut self, tile_count: usize, center_tile: (i32, i32)) {
        self.probe_starts_since_last_edit = self.probe_starts_since_last_edit.saturating_add(1);
        self.push_event(format!(
            "probe_start tiles={tile_count} center={},{}",
            center_tile.0, center_tile.1
        ));
    }

    fn push_event(&mut self, event: impl Into<String>) {
        self.recent_events.push(SharedString::from(event.into()));
        const MAX_EVENTS: usize = 8;
        if self.recent_events.len() > MAX_EVENTS {
            let overflow = self.recent_events.len().saturating_sub(MAX_EVENTS);
            self.recent_events.drain(0..overflow);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct ProfessionalQueryState {
    pub(super) overlay_bounds: Option<SlimeChunkBounds>,
    pub(super) overlays: Option<RegionOverlayQuery>,
    pub(super) overlay_paint: Option<Arc<ProfessionalOverlayPaintCache>>,
    pub(super) overlay_loading: bool,
    pub(super) overlay_generation: u64,
    pub(super) overlay_cancel: Option<CancelFlag>,
    pub(super) pending_overlay_refresh: bool,
    pub(super) last_overlay_request_bounds: Option<SlimeChunkBounds>,
    pub(super) last_overlay_request_options: Option<RegionOverlayQueryOptions>,
    pub(super) village_index: Option<Arc<VillageOverlayIndex>>,
    pub(super) village_index_loading: bool,
    pub(super) village_index_generation: u64,
    pub(super) village_index_cancel: Option<CancelFlag>,
    pub(super) slime_overlay_runs: Option<Arc<SlimeOverlayRunCache>>,
    pub(super) slime_window_candidates: Option<SlimeWindowCandidateCache>,
    pub(super) selection: Option<ChunkSelection>,
    pub(super) highlighted_window: Option<SlimeChunkWindow>,
    pub(super) selection_stats: Option<SelectionStats>,
    pub(super) detail: Option<ProfessionalDetail>,
    pub(super) write_mode: bool,
    pub(super) pending_delete_confirmation: bool,
    pub(super) pending_edit_confirmation: Option<PendingEditConfirmation>,
    pub(super) pending_quick_write_confirmation: Option<QuickWriteAction>,
    pub(super) copied_chunk: Option<CopiedChunkData>,
    pub(super) imported_region_package: bool,
    pub(super) imported_structure: Option<ImportedStructureData>,
    pub(super) copied_chunk_preview_images: BTreeMap<ChunkPos, CopiedChunkPreviewImage>,
    pub(super) paste_preview: Option<PastePreview>,
    pub(super) chunk_transfer_progress: Option<ChunkTransferProgress>,
    pub(super) last_chunk_transfer_progress: Option<ChunkTransferProgress>,
    pub(super) last_chunk_transfer_finished_at: Option<Instant>,
    pub(super) edit_loading: bool,
    pub(super) edit_generation: u64,
}

#[derive(Clone, Debug)]
pub(super) struct PlayerSummary {
    pub(super) id: PlayerId,
    pub(super) label: SharedString,
}

#[derive(Clone, Debug)]
pub(super) struct PlayerDetail {
    pub(super) id: PlayerId,
    pub(super) unique_id: Option<i64>,
    pub(super) position: Option<[f64; 3]>,
    pub(super) dimension_id: Option<i32>,
    pub(super) item_count: usize,
    pub(super) items: Vec<bedrock_world::ItemStack>,
    pub(super) nbt: NbtTag,
    pub(super) json: SharedString,
}

#[derive(Clone, Debug, Default)]
pub(super) struct PlayerPanelState {
    pub(super) players: Vec<PlayerSummary>,
    pub(super) selected: Option<PlayerId>,
    pub(super) detail: Option<PlayerDetail>,
    pub(super) loading: bool,
    pub(super) saving: bool,
    pub(super) generation: u64,
    pub(super) error: Option<SharedString>,
    pub(super) pending_save_confirmation: Option<PlayerQuickEdit>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum PlayerQuickEdit {
    MoveToMapCenter,
    SetDimension(Dimension),
    ClearInventory,
}

impl PlayerQuickEdit {
    pub(super) fn label(&self) -> String {
        match self {
            Self::MoveToMapCenter => "移动到地图中心".to_string(),
            Self::SetDimension(dimension) => format!("设置维度为 {}", dimension_label(*dimension)),
            Self::ClearInventory => "清空背包物品".to_string(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct ProfessionalOverlayPaintCache {
    pub(super) hardcoded_spawn_rects: Vec<BlockOverlayRect>,
    pub(super) village_rects: Vec<ChunkOverlayRect>,
    pub(super) entity_points: Vec<BlockOverlayPoint>,
    pub(super) block_entity_points: Vec<BlockOverlayPoint>,
    pub(super) entity_chunk_markers: Vec<ChunkOverlayMarker>,
    pub(super) block_entity_chunk_markers: Vec<ChunkOverlayMarker>,
}

impl ProfessionalOverlayPaintCache {
    pub(super) fn from_query(query: &RegionOverlayQuery) -> Self {
        let mut entity_chunks = BTreeMap::<(i32, i32), usize>::new();
        let mut block_entity_chunks = BTreeMap::<(i32, i32), usize>::new();
        let mut cache = Self {
            hardcoded_spawn_rects: query
                .hardcoded_spawn_areas
                .iter()
                .map(|area| BlockOverlayRect {
                    min_block_x: area.area.min[0],
                    min_block_z: area.area.min[2],
                    max_block_x: area.area.max[0].saturating_add(1),
                    max_block_z: area.area.max[2].saturating_add(1),
                })
                .collect(),
            village_rects: query
                .villages
                .iter()
                .filter_map(|village| village.bounds)
                .map(|bounds| ChunkOverlayRect {
                    min_chunk_x: bounds.min_chunk_x,
                    min_chunk_z: bounds.min_chunk_z,
                    max_chunk_x: bounds.max_chunk_x,
                    max_chunk_z: bounds.max_chunk_z,
                })
                .collect(),
            entity_points: Vec::with_capacity(query.entities.len()),
            block_entity_points: Vec::with_capacity(query.block_entities.len()),
            entity_chunk_markers: Vec::new(),
            block_entity_chunk_markers: Vec::new(),
        };
        for entity in &query.entities {
            cache.entity_points.push(BlockOverlayPoint {
                block_x: entity.position[0] as f32,
                block_z: entity.position[2] as f32,
            });
            *entity_chunks
                .entry((entity.chunk.x, entity.chunk.z))
                .or_default() += 1;
        }
        for block_entity in &query.block_entities {
            cache.block_entity_points.push(BlockOverlayPoint {
                block_x: block_entity.position[0] as f32,
                block_z: block_entity.position[2] as f32,
            });
            *block_entity_chunks
                .entry((block_entity.chunk.x, block_entity.chunk.z))
                .or_default() += 1;
        }
        cache.entity_chunk_markers = entity_chunks
            .into_iter()
            .map(|((chunk_x, chunk_z), count)| ChunkOverlayMarker {
                chunk_x,
                chunk_z,
                count,
            })
            .collect();
        cache.block_entity_chunk_markers = block_entity_chunks
            .into_iter()
            .map(|((chunk_x, chunk_z), count)| ChunkOverlayMarker {
                chunk_x,
                chunk_z,
                count,
            })
            .collect();
        cache
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BlockOverlayRect {
    pub(super) min_block_x: i32,
    pub(super) min_block_z: i32,
    pub(super) max_block_x: i32,
    pub(super) max_block_z: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ChunkOverlayRect {
    pub(super) min_chunk_x: i32,
    pub(super) min_chunk_z: i32,
    pub(super) max_chunk_x: i32,
    pub(super) max_chunk_z: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BlockOverlayPoint {
    pub(super) block_x: f32,
    pub(super) block_z: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ChunkOverlayMarker {
    pub(super) chunk_x: i32,
    pub(super) chunk_z: i32,
    pub(super) count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SlimeOverlayRunCache {
    pub(super) bounds: SlimeChunkBounds,
    pub(super) runs: Vec<ChunkOverlayRect>,
}

impl SlimeOverlayRunCache {
    pub(super) fn build(bounds: SlimeChunkBounds) -> Option<Self> {
        if bounds.dimension != Dimension::Overworld || bounds.chunk_count() > 20_000 {
            return None;
        }
        let mut runs = Vec::new();
        for chunk_z in bounds.min_chunk_z..=bounds.max_chunk_z {
            let mut run_start = None;
            for chunk_x in bounds.min_chunk_x..=bounds.max_chunk_x {
                let is_slime = is_slime_chunk(ChunkPos {
                    x: chunk_x,
                    z: chunk_z,
                    dimension: bounds.dimension,
                });
                match (run_start, is_slime) {
                    (None, true) => run_start = Some(chunk_x),
                    (Some(start), false) => {
                        runs.push(ChunkOverlayRect {
                            min_chunk_x: start,
                            min_chunk_z: chunk_z,
                            max_chunk_x: chunk_x.saturating_sub(1),
                            max_chunk_z: chunk_z,
                        });
                        run_start = None;
                    }
                    _ => {}
                }
            }
            if let Some(start) = run_start {
                runs.push(ChunkOverlayRect {
                    min_chunk_x: start,
                    min_chunk_z: chunk_z,
                    max_chunk_x: bounds.max_chunk_x,
                    max_chunk_z: chunk_z,
                });
            }
        }
        Some(Self { bounds, runs })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SlimeWindowCandidateCache {
    pub(super) bounds: SlimeChunkBounds,
    pub(super) size: SlimeQueryWindowSize,
    pub(super) windows: Vec<SlimeChunkWindow>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ToolbarState {
    pub(super) expanded: bool,
    pub(super) diagnostics_open: bool,
}

impl Default for ToolbarState {
    fn default() -> Self {
        Self {
            expanded: false,
            diagnostics_open: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum MapInputField {
    CenterX,
    CenterZ,
    ZoomPercent,
    DimensionId,
}

#[derive(Clone, Debug, Default)]
pub(super) struct InputValidationState {
    pub(super) invalid_field: Option<MapInputField>,
    pub(super) message: Option<SharedString>,
}

pub(super) struct MapInputFields {
    pub(super) center_x: Entity<InputState>,
    pub(super) center_z: Entity<InputState>,
    pub(super) zoom_percent: Entity<InputState>,
    pub(super) dimension_id: Entity<InputState>,
    pub(super) focused_field: Option<MapInputField>,
    pub(super) dirty_fields: BTreeSet<MapInputField>,
    pub(super) validation: InputValidationState,
}

impl MapInputFields {
    pub(super) fn new(window: &mut Window, cx: &mut Context<MapViewerWindowView>) -> Self {
        Self {
            center_x: map_input_state(window, cx, "X"),
            center_z: map_input_state(window, cx, "Z"),
            zoom_percent: map_input_state(window, cx, "Zoom %"),
            dimension_id: map_input_state(window, cx, "Dim ID"),
            focused_field: None,
            dirty_fields: BTreeSet::new(),
            validation: InputValidationState::default(),
        }
    }

    pub(super) fn entity(&self, field: MapInputField) -> &Entity<InputState> {
        match field {
            MapInputField::CenterX => &self.center_x,
            MapInputField::CenterZ => &self.center_z,
            MapInputField::ZoomPercent => &self.zoom_percent,
            MapInputField::DimensionId => &self.dimension_id,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct FrameStats {
    pub(super) window_started_at: Instant,
    pub(super) frames_in_window: u32,
    pub(super) fps: f32,
}

impl Default for FrameStats {
    fn default() -> Self {
        Self {
            window_started_at: Instant::now(),
            frames_in_window: 0,
            fps: 0.0,
        }
    }
}

impl FrameStats {
    pub(super) fn record_frame(&mut self) {
        self.frames_in_window = self.frames_in_window.saturating_add(1);
        let elapsed = self.window_started_at.elapsed();
        if elapsed >= Duration::from_millis(500) {
            self.fps = self.frames_in_window as f32 / elapsed.as_secs_f32().max(0.001);
            self.frames_in_window = 0;
            self.window_started_at = Instant::now();
        }
    }
}

pub(super) struct TileManifestProbeResult {
    pub(super) requested_tiles: Vec<(i32, i32)>,
    pub(super) tile_chunk_index: TileChunkIndex,
    pub(super) bounds: Option<ChunkBounds>,
    pub(super) center_block_x: Option<i32>,
    pub(super) center_block_z: Option<i32>,
}

pub(super) enum TileRenderEvent {
    ReadyBatch {
        tiles: Vec<ReadyTile>,
    },
    Empty {
        coord: (i32, i32),
        message: String,
    },
    Failed {
        coord: (i32, i32),
        message: String,
    },
    Complete {
        requested_tiles: Vec<(i32, i32)>,
        diagnostics: RenderDiagnostics,
        stats: RenderPipelineStats,
    },
}

pub(super) enum Preview3dLoadEvent {
    Chunk {
        mesh: Arc<Preview3dMesh>,
        status: Preview3dBuildStatus,
    },
    Complete(Result<Arc<Preview3dMesh>, String>),
}

#[derive(Clone, PartialEq)]
pub(super) struct MapCanvasSnapshotKey {
    pub(super) viewport: MapViewport,
    pub(super) layout: RenderLayout,
    pub(super) colors: ThemeColors,
    pub(super) dragging: bool,
    pub(super) overlays: OverlayOptions,
    pub(super) tile_generation: u64,
    pub(super) overlay_generation: u64,
    pub(super) overlay_paint_ptr: Option<usize>,
    pub(super) slime_runs_ptr: Option<usize>,
    pub(super) selection: Option<ChunkSelection>,
    pub(super) paste_preview: Option<PastePreview>,
    pub(super) paste_preview_images_generation: u64,
    pub(super) highlighted_window: Option<SlimeChunkWindow>,
    pub(super) markers_generation: u64,
    pub(super) hover_block_x: i32,
    pub(super) hover_block_z: i32,
}

#[derive(Clone, PartialEq)]
pub(super) struct TileLayerSnapshotKey {
    pub(super) viewport: MapViewport,
    pub(super) layout: RenderLayout,
    pub(super) colors: ThemeColors,
    pub(super) dragging: bool,
    pub(super) tile_generation: u64,
}

pub struct MapViewerWindowView {
    pub(super) version: ManagedVersionEntry,
    pub(super) asset: ManageAssetEntry,
    pub(super) world_path: PathBuf,
    pub(super) mode: ViewerMode,
    pub(super) dimension: Dimension,
    pub(super) custom_dimension_id: i32,
    pub(super) y_layer: i32,
    pub(super) active_layout: RenderLayout,
    pub(super) viewport: MapViewport,
    pub(super) window_width: f32,
    pub(super) window_height: f32,
    pub(super) cpu_budget: RenderCpuBudget,
    pub(super) render_backend: RenderBackend,
    pub(super) render_gpu_backend: RenderGpuBackend,
    pub(super) overlay_options: OverlayOptions,
    pub(super) slime_query_window_size: SlimeQueryWindowSize,
    pub(super) professional: ProfessionalQueryState,
    pub(super) history: MapHistoryState,
    pub(super) players: PlayerPanelState,
    pub(super) preview_3d: Preview3dState,
    pub(super) map_focus_handle: FocusHandle,
    pub(super) preview_3d_focus_handle: FocusHandle,
    pub(super) edit_toast_id: Option<toast::ToastId>,
    pub(super) toolbar_state: ToolbarState,
    pub(super) input_fields: MapInputFields,
    pub(super) ui_state: MapViewerUiState,
    pub(super) top_bar_view: Entity<MapTopBarView>,
    pub(super) tool_stripe_view: Entity<MapToolStripeView>,
    pub(super) menu_overlay_view: Entity<MapMenuOverlayView>,
    pub(super) canvas_view: Entity<MapCanvasView>,
    pub(super) editor_document: EditorDocument<EditTarget>,
    pub(super) editor_state: Entity<CodeEditorState>,
    pub(super) db_tree: DbTreeState,
    pub(super) task_snapshots: HashMap<Arc<str>, Arc<TaskSnapshot>>,
    pub(super) task_updates_task: Option<Task<anyhow::Result<()>>>,
    pub(super) frame_stats: FrameStats,
    pub(super) tile_reveal_state: TileRevealState,
    pub(super) available_tiles: BTreeSet<(i32, i32)>,
    pub(super) tile_chunk_index: TileChunkIndex,
    pub(super) chunk_bounds: Option<ChunkBounds>,
    pub(super) tile_manager: RegionManager,
    pub(super) canvas_tile_snapshot: Arc<TilePaintSnapshot>,
    pub(super) canvas_tile_generation: u64,
    pub(super) paste_preview_images: Arc<Vec<PastePreviewImage>>,
    pub(super) paste_preview_images_generation: u64,
    pub(super) last_synced_canvas_snapshot_key: Option<MapCanvasSnapshotKey>,
    pub(super) last_synced_tile_layer_snapshot_key: Option<TileLayerSnapshotKey>,
    pub(super) render_session: Option<Arc<MapRenderSession>>,
    pub(super) markers: BTreeMap<Dimension, Vec<Marker>>,
    pub(super) markers_generation: u64,
    pub(super) context_menu: Option<ContextMenuState>,
    pub(super) drag: Option<DragState>,
    pub(super) right_selection_drag: Option<RightSelectionDrag>,
    pub(super) hover_block_x: i32,
    pub(super) hover_block_z: i32,
    pub(super) recenter_on_next_metadata: bool,
    pub(super) pending_center_block: Option<(i32, i32)>,
    pub(super) bypass_cache_active: bool,
    pub(super) metadata_loading: bool,
    pub(super) metadata_index_ready: bool,
    pub(super) manifest_probe_in_flight: bool,
    pub(super) manifest_probe_diagnostics: ManifestProbeDiagnostics,
    pub(super) manifest_scanned_tiles: BTreeSet<(i32, i32)>,
    pub(super) session_loading: bool,
    pub(super) render_batch_active: bool,
    pub(super) request_id: u64,
    pub(super) metadata_generation: u64,
    pub(super) session_generation: u64,
    pub(super) render_generation: u64,
    pub(super) metadata_cancel: Option<RenderTaskControl>,
    pub(super) manifest_probe_cancel: Option<RenderTaskControl>,
    pub(super) render_cancels: BTreeMap<u64, RenderCancelFlag>,
    pub(super) active_render_tiles: BTreeSet<(i32, i32)>,
    pub(super) active_render_center_tile: Option<(i32, i32)>,
    pub(super) pending_viewport_refresh: bool,
    pub(super) viewport_idle_generation: u64,
    pub(super) last_viewport_tile_sync: Option<Instant>,
    pub(super) last_drag_canvas_snapshot_sync: Option<Instant>,
    pub(super) last_visible_tile_log: Option<Instant>,
    pub(super) last_tile_memory_trim: Option<Instant>,
    pub(super) pending_render_image_evictions: Vec<(Instant, Arc<RenderImage>)>,
    pub(super) pending_render_image_eviction_generation: u64,
    pub(super) last_visible_tile_signature: Option<ViewportTileSignature>,
    pub(super) last_ready_status_update: Option<Instant>,
    pub(super) status: SharedString,
    pub(super) diagnostics: RenderDiagnostics,
    pub(super) render_stats: RenderPipelineStats,
    pub(super) refresh_rendered_tiles: usize,
    pub(super) partial_refreshed_chunks: usize,
    pub(super) cold_rendered_tiles: usize,
    pub(super) last_queue_distance_squared: i64,
    pub(super) last_visible_error: Option<SharedString>,
    pub(super) _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ViewportTileSignature {
    pub(super) visible: Vec<(i32, i32)>,
    pub(super) prefetch: Vec<(i32, i32)>,
    pub(super) retain: Vec<(i32, i32)>,
    pub(super) center: (i32, i32),
    pub(super) metadata_loading: bool,
    pub(super) metadata_index_ready: bool,
}

pub(super) struct ViewportTilePlan {
    pub(super) visible: Vec<(i32, i32)>,
    pub(super) prefetch: Vec<(i32, i32)>,
    pub(super) retain: BTreeSet<(i32, i32)>,
    pub(super) center: (i32, i32),
    pub(super) is_dragging: bool,
    pub(super) prefetch_radius: i32,
}
