use super::prelude::*;
use sha2::Digest as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const HISTORY_MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60;
const HISTORY_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const HISTORY_ENTRY_FILE: &str = "entry.json";
const HISTORY_DELTA_FILE: &str = "delta.zst";
const HISTORY_OBJECT_STORE_DIR: &str = "objects";
const HISTORY_OBJECT_MIN_BYTES: usize = 128;
const HISTORY_STORAGE_INLINE_ZSTD: &str = "inlineZstd";
const HISTORY_STORAGE_OBJECT_STORE_V1: &str = "objectStoreV1";
const HISTORY_APPLY_BATCH_RECORDS: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum MapHistoryEntryStatus {
    Success,
    Failed,
    Undone,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum MapHistoryEntryKind {
    ChunkDelete,
    ChunkReset,
    ChunkPaste,
    RecordSave,
    RecordDelete,
    PlayerEdit,
    LevelDatSave,
    RestorePoint,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MapHistoryEntry {
    pub(super) id: String,
    pub(super) timestamp_secs: u64,
    pub(super) kind: MapHistoryEntryKind,
    pub(super) label: String,
    pub(super) message: String,
    pub(super) world_path: String,
    pub(super) chunks: Vec<ChunkPos>,
    pub(super) raw_delta_count: usize,
    pub(super) raw_delta_bytes: u64,
    #[serde(default)]
    pub(super) stored_bytes: u64,
    #[serde(default)]
    pub(super) stored_object_count: usize,
    #[serde(default)]
    pub(super) reused_object_count: usize,
    #[serde(default = "default_history_storage_format")]
    pub(super) storage_format: String,
    pub(super) level_dat_changed: bool,
    pub(super) status: MapHistoryEntryStatus,
    pub(super) error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MapHistoryChange {
    pub(super) raw_records: Vec<RawRecordDelta>,
    pub(super) level_dat: Option<LevelDatDelta>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawRecordDelta {
    pub(super) key: Vec<u8>,
    pub(super) before: Option<Vec<u8>>,
    pub(super) after: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LevelDatDelta {
    pub(super) before: Option<Vec<u8>>,
    pub(super) after: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MapHistoryStoredChange {
    #[serde(default = "default_history_storage_format")]
    storage_format: String,
    raw_records: Vec<StoredRawRecordDelta>,
    level_dat: Option<StoredLevelDatDelta>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredRawRecordDelta {
    key: Vec<u8>,
    before: Option<StoredHistoryValue>,
    after: Option<StoredHistoryValue>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredLevelDatDelta {
    before: Option<StoredHistoryValue>,
    after: Option<StoredHistoryValue>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum StoredHistoryValue {
    Blob(HistoryBlobRef),
    Inline(Vec<u8>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryBlobRef {
    sha256: String,
    size: u64,
}

#[derive(Clone, Debug, Default)]
struct HistoryStorageStats {
    stored_bytes: u64,
    stored_object_count: usize,
    reused_object_count: usize,
}

const HISTORY_VISUAL_COLUMN_SIDE: usize = 16;
const HISTORY_VISUAL_COLUMN_COUNT: usize = HISTORY_VISUAL_COLUMN_SIDE * HISTORY_VISUAL_COLUMN_SIDE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MapHistoryChunkVisualKind {
    Added,
    Removed,
    Modified,
    Mixed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MapHistoryVisualFilterKind {
    Added,
    Removed,
    Modified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MapHistoryVisualFilter {
    pub(super) show_added: bool,
    pub(super) show_removed: bool,
    pub(super) show_modified: bool,
}

impl Default for MapHistoryVisualFilter {
    fn default() -> Self {
        Self {
            show_added: true,
            show_removed: true,
            show_modified: true,
        }
    }
}

impl MapHistoryVisualFilter {
    pub(super) const fn includes(self, kind: MapHistoryVisualFilterKind) -> bool {
        match kind {
            MapHistoryVisualFilterKind::Added => self.show_added,
            MapHistoryVisualFilterKind::Removed => self.show_removed,
            MapHistoryVisualFilterKind::Modified => self.show_modified,
        }
    }

    pub(super) fn toggle(&mut self, kind: MapHistoryVisualFilterKind) {
        match kind {
            MapHistoryVisualFilterKind::Added => self.show_added = !self.show_added,
            MapHistoryVisualFilterKind::Removed => self.show_removed = !self.show_removed,
            MapHistoryVisualFilterKind::Modified => {
                self.show_modified = !self.show_modified;
            }
        }
    }

    pub(super) const fn any_enabled(self) -> bool {
        self.show_added || self.show_removed || self.show_modified
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct MapHistoryColumnVisual {
    pub(super) added_blocks: u16,
    pub(super) removed_blocks: u16,
    pub(super) modified_blocks: u16,
}

impl MapHistoryColumnVisual {
    pub(super) const fn total_blocks(self) -> u32 {
        self.added_blocks as u32 + self.removed_blocks as u32 + self.modified_blocks as u32
    }

    pub(super) const fn filtered_counts(self, filter: MapHistoryVisualFilter) -> (u32, u32, u32) {
        (
            if filter.show_added {
                self.added_blocks as u32
            } else {
                0
            },
            if filter.show_removed {
                self.removed_blocks as u32
            } else {
                0
            },
            if filter.show_modified {
                self.modified_blocks as u32
            } else {
                0
            },
        )
    }

    fn add(&mut self, kind: MapHistoryVisualFilterKind) {
        match kind {
            MapHistoryVisualFilterKind::Added => {
                self.added_blocks = self.added_blocks.saturating_add(1);
            }
            MapHistoryVisualFilterKind::Removed => {
                self.removed_blocks = self.removed_blocks.saturating_add(1);
            }
            MapHistoryVisualFilterKind::Modified => {
                self.modified_blocks = self.modified_blocks.saturating_add(1);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct MapHistoryChunkVisual {
    pub(super) pos: ChunkPos,
    pub(super) kind: MapHistoryChunkVisualKind,
    pub(super) added_records: usize,
    pub(super) removed_records: usize,
    pub(super) modified_records: usize,
    pub(super) added_blocks: u32,
    pub(super) removed_blocks: u32,
    pub(super) modified_blocks: u32,
    pub(super) columns: [MapHistoryColumnVisual; HISTORY_VISUAL_COLUMN_COUNT],
    pub(super) max_column_blocks: u16,
    pub(super) precise_subchunks: usize,
    pub(super) unresolved_terrain_records: usize,
    pub(super) terrain_records: usize,
    pub(super) block_entity_records: usize,
    pub(super) entity_records: usize,
    pub(super) metadata_records: usize,
}

impl MapHistoryChunkVisual {
    pub(super) const fn total_blocks(&self) -> u32 {
        self.added_blocks
            .saturating_add(self.removed_blocks)
            .saturating_add(self.modified_blocks)
    }

    pub(super) const fn total_records(&self) -> usize {
        self.added_records
            .saturating_add(self.removed_records)
            .saturating_add(self.modified_records)
    }

    pub(super) const fn filtered_block_counts(
        &self,
        filter: MapHistoryVisualFilter,
    ) -> (u32, u32, u32) {
        (
            if filter.show_added {
                self.added_blocks
            } else {
                0
            },
            if filter.show_removed {
                self.removed_blocks
            } else {
                0
            },
            if filter.show_modified {
                self.modified_blocks
            } else {
                0
            },
        )
    }

    pub(super) const fn filtered_record_counts(
        &self,
        filter: MapHistoryVisualFilter,
    ) -> (usize, usize, usize) {
        (
            if filter.show_added {
                self.added_records
            } else {
                0
            },
            if filter.show_removed {
                self.removed_records
            } else {
                0
            },
            if filter.show_modified {
                self.modified_records
            } else {
                0
            },
        )
    }

    pub(super) fn filtered_total(&self, filter: MapHistoryVisualFilter) -> u64 {
        let (added, removed, modified) = self.filtered_block_counts(filter);
        let block_total = u64::from(added)
            .saturating_add(u64::from(removed))
            .saturating_add(u64::from(modified));
        if block_total > 0 {
            return block_total;
        }
        let (added, removed, modified) = self.filtered_record_counts(filter);
        (added as u64)
            .saturating_add(removed as u64)
            .saturating_add(modified as u64)
    }

    pub(super) fn filtered_kind(
        &self,
        filter: MapHistoryVisualFilter,
    ) -> Option<MapHistoryChunkVisualKind> {
        let (mut added, mut removed, mut modified) = self.filtered_block_counts(filter);
        if added == 0 && removed == 0 && modified == 0 {
            let records = self.filtered_record_counts(filter);
            added = u32::try_from(records.0).unwrap_or(u32::MAX);
            removed = u32::try_from(records.1).unwrap_or(u32::MAX);
            modified = u32::try_from(records.2).unwrap_or(u32::MAX);
        }
        history_visual_kind(added, removed, modified)
    }

    pub(super) fn has_kind(&self, kind: MapHistoryVisualFilterKind) -> bool {
        match kind {
            MapHistoryVisualFilterKind::Added => self.added_blocks > 0 || self.added_records > 0,
            MapHistoryVisualFilterKind::Removed => {
                self.removed_blocks > 0 || self.removed_records > 0
            }
            MapHistoryVisualFilterKind::Modified => {
                self.modified_blocks > 0 || self.modified_records > 0
            }
        }
    }

    pub(super) const fn precise_block_diff(&self) -> bool {
        self.precise_subchunks > 0
    }

    pub(super) const fn record_only_diff(&self) -> bool {
        self.precise_subchunks == 0
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct MapHistoryVisualization {
    pub(super) chunks: Vec<MapHistoryChunkVisual>,
    pub(super) added_records: usize,
    pub(super) removed_records: usize,
    pub(super) modified_records: usize,
    pub(super) added_blocks: u64,
    pub(super) removed_blocks: u64,
    pub(super) modified_blocks: u64,
    pub(super) changed_subchunks: usize,
    pub(super) precise_chunks: usize,
    pub(super) partial_chunks: usize,
    pub(super) record_only_chunks: usize,
    pub(super) mixed_chunks: usize,
    pub(super) terrain_records: usize,
    pub(super) block_entity_records: usize,
    pub(super) entity_records: usize,
    pub(super) metadata_records: usize,
    pub(super) unmapped_records: usize,
    pub(super) level_dat_changed: bool,
}

impl MapHistoryVisualization {
    pub(super) const fn total_blocks(&self) -> u64 {
        self.added_blocks
            .saturating_add(self.removed_blocks)
            .saturating_add(self.modified_blocks)
    }

    pub(super) const fn total_records(&self) -> usize {
        self.added_records
            .saturating_add(self.removed_records)
            .saturating_add(self.modified_records)
    }

    pub(super) const fn kind_blocks(&self, kind: MapHistoryVisualFilterKind) -> u64 {
        match kind {
            MapHistoryVisualFilterKind::Added => self.added_blocks,
            MapHistoryVisualFilterKind::Removed => self.removed_blocks,
            MapHistoryVisualFilterKind::Modified => self.modified_blocks,
        }
    }

    pub(super) const fn kind_records(&self, kind: MapHistoryVisualFilterKind) -> usize {
        match kind {
            MapHistoryVisualFilterKind::Added => self.added_records,
            MapHistoryVisualFilterKind::Removed => self.removed_records,
            MapHistoryVisualFilterKind::Modified => self.modified_records,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct MapHistoryState {
    pub(super) entries: Arc<Vec<MapHistoryEntry>>,
    pub(super) selected_entry_id: Option<String>,
    pub(super) loading: bool,
    pub(super) applying: bool,
    pub(super) error: Option<SharedString>,
    pub(super) visualization: Arc<MapHistoryVisualization>,
    pub(super) visualization_loading: bool,
    pub(super) visualization_enabled: bool,
    pub(super) visualization_filter: MapHistoryVisualFilter,
    pub(super) visualization_error: Option<SharedString>,
}

impl Default for MapHistoryState {
    fn default() -> Self {
        Self {
            entries: Arc::new(Vec::new()),
            selected_entry_id: None,
            loading: false,
            applying: false,
            error: None,
            visualization: Arc::new(MapHistoryVisualization::default()),
            visualization_loading: false,
            visualization_enabled: true,
            visualization_filter: MapHistoryVisualFilter::default(),
            visualization_error: None,
        }
    }
}

fn default_history_storage_format() -> String {
    HISTORY_STORAGE_INLINE_ZSTD.to_string()
}

#[derive(Clone, Debug)]
pub(crate) struct MapHistoryCapture {
    id: String,
    timestamp_secs: u64,
    kind: MapHistoryEntryKind,
    label: String,
    world_path: PathBuf,
    history_dir: PathBuf,
    chunks: BTreeSet<ChunkPos>,
    raw_keys: BTreeSet<Vec<u8>>,
    before_raw_records: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    before_level_dat: Option<Option<Vec<u8>>>,
}

#[derive(Clone, Debug)]
pub(crate) struct MapHistoryCaptureSpec {
    pub(crate) kind: MapHistoryEntryKind,
    pub(crate) label: String,
    pub(crate) world_path: PathBuf,
    pub(crate) chunks: BTreeSet<ChunkPos>,
    pub(crate) raw_keys: BTreeSet<Vec<u8>>,
    pub(crate) include_level_dat: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct MapHistoryApplyOutcome {
    pub(super) affected_chunks: BTreeSet<ChunkPos>,
    pub(super) refresh_all_tiles: bool,
    pub(super) level_dat_changed: bool,
    pub(super) message: String,
}

#[derive(Clone, Debug, Default)]
struct MapHistoryAppliedChange {
    affected_chunks: BTreeSet<ChunkPos>,
    refresh_all_tiles: bool,
    level_dat_changed: bool,
}

#[derive(Clone, Debug)]
pub(super) struct MapHistoryApplyProgress {
    pub(super) phase: SharedString,
    pub(super) completed: usize,
    pub(super) total: usize,
}

impl MapHistoryEntry {
    pub(super) fn short_status(&self) -> &'static str {
        match self.status {
            MapHistoryEntryStatus::Success => "成功",
            MapHistoryEntryStatus::Failed => "失败",
            MapHistoryEntryStatus::Undone => "已撤回",
        }
    }

    pub(super) fn kind_label(&self) -> &'static str {
        match self.kind {
            MapHistoryEntryKind::ChunkDelete => "删除区块",
            MapHistoryEntryKind::ChunkReset => "重置区块",
            MapHistoryEntryKind::ChunkPaste => "粘贴区块",
            MapHistoryEntryKind::RecordSave => "保存记录",
            MapHistoryEntryKind::RecordDelete => "删除记录",
            MapHistoryEntryKind::PlayerEdit => "玩家编辑",
            MapHistoryEntryKind::LevelDatSave => "level.dat",
            MapHistoryEntryKind::RestorePoint => "回档保护点",
        }
    }
}

pub(crate) fn capture_before(spec: MapHistoryCaptureSpec) -> Result<MapHistoryCapture, String> {
    capture_before_with_progress(spec, |_| {})
}

pub(crate) fn capture_before_with_progress(
    spec: MapHistoryCaptureSpec,
    progress: impl FnMut(MapHistoryApplyProgress),
) -> Result<MapHistoryCapture, String> {
    let world = open_world_readonly(&spec.world_path)?;
    capture_before_with_world_and_progress(spec, &world, progress)
}

pub(crate) fn capture_before_with_world_and_progress(
    spec: MapHistoryCaptureSpec,
    world: &BedrockWorld,
    mut progress: impl FnMut(MapHistoryApplyProgress),
) -> Result<MapHistoryCapture, String> {
    let mut raw_keys = spec.raw_keys.clone();
    let chunk_total = spec.chunks.len().max(1);
    let phase = SharedString::from("创建写入前快照");
    progress(MapHistoryApplyProgress {
        phase: phase.clone(),
        completed: 0,
        total: chunk_total,
    });
    for (index, chunk) in spec.chunks.iter().enumerate() {
        let keys = collect_chunk_raw_keys(world, *chunk)?;
        raw_keys.extend(keys);
        progress(MapHistoryApplyProgress {
            phase: phase.clone(),
            completed: index.saturating_add(1),
            total: chunk_total,
        });
    }

    let history_dir = history_dir_for_world(&spec.world_path);
    let mut before_raw_records = BTreeMap::new();
    for key in &raw_keys {
        let value = world
            .storage()
            .get(key)
            .map_err(|error| format!("读取历史原始记录失败: {error}"))?
            .map(|bytes| bytes.to_vec());
        before_raw_records.insert(key.clone(), value);
    }

    let before_level_dat = if spec.include_level_dat {
        Some(read_optional_file(spec.world_path.join("level.dat"))?)
    } else {
        None
    };

    Ok(MapHistoryCapture {
        id: new_history_id(),
        timestamp_secs: now_secs(),
        kind: spec.kind,
        label: spec.label,
        world_path: spec.world_path,
        history_dir,
        chunks: spec.chunks,
        raw_keys,
        before_raw_records,
        before_level_dat,
    })
}

pub(crate) fn complete_after(
    capture: MapHistoryCapture,
    message: impl Into<String>,
) -> Result<MapHistoryEntry, String> {
    complete_after_with_progress(capture, message, |_| {})
}

pub(crate) fn complete_after_with_progress(
    capture: MapHistoryCapture,
    message: impl Into<String>,
    progress: impl FnMut(MapHistoryApplyProgress),
) -> Result<MapHistoryEntry, String> {
    let world = open_world_readonly(&capture.world_path)?;
    complete_after_with_world_and_progress(capture, &world, message, progress)
}

pub(crate) fn complete_after_with_world_and_progress(
    capture: MapHistoryCapture,
    world: &BedrockWorld,
    message: impl Into<String>,
    mut progress: impl FnMut(MapHistoryApplyProgress),
) -> Result<MapHistoryEntry, String> {
    fs::create_dir_all(&capture.history_dir)
        .map_err(|error| format!("创建历史目录失败: {error}"))?;
    let mut raw_keys = capture.raw_keys.clone();
    let chunk_total = capture.chunks.len().max(1);
    let phase = SharedString::from("保存写入历史");
    progress(MapHistoryApplyProgress {
        phase: phase.clone(),
        completed: 0,
        total: chunk_total,
    });
    for (index, chunk) in capture.chunks.iter().enumerate() {
        raw_keys.extend(collect_chunk_raw_keys(world, *chunk)?);
        progress(MapHistoryApplyProgress {
            phase: phase.clone(),
            completed: index.saturating_add(1),
            total: chunk_total,
        });
    }
    let write_phase = SharedString::from("写入历史文件");
    progress(MapHistoryApplyProgress {
        phase: write_phase.clone(),
        completed: 0,
        total: 1,
    });
    let (raw_records, raw_delta_bytes) =
        build_raw_record_deltas(&capture.before_raw_records, &raw_keys, |key| {
            world
                .storage()
                .get(key)
                .map_err(|error| format!("读取历史写入后记录失败: {error}"))
                .map(|value| value.map(|bytes| bytes.to_vec()))
        })?;

    let level_dat = if let Some(before) = capture.before_level_dat {
        let after = read_optional_file(capture.world_path.join("level.dat"))?;
        (before != after).then_some(LevelDatDelta { before, after })
    } else {
        None
    };

    let change = MapHistoryChange {
        raw_records,
        level_dat,
    };
    let mut entry = MapHistoryEntry {
        id: capture.id,
        timestamp_secs: capture.timestamp_secs,
        kind: capture.kind,
        label: capture.label,
        message: message.into(),
        world_path: capture.world_path.to_string_lossy().to_string(),
        chunks: capture.chunks.iter().copied().collect(),
        raw_delta_count: change.raw_records.len(),
        raw_delta_bytes,
        stored_bytes: 0,
        stored_object_count: 0,
        reused_object_count: 0,
        storage_format: HISTORY_STORAGE_OBJECT_STORE_V1.to_string(),
        level_dat_changed: change.level_dat.is_some(),
        status: MapHistoryEntryStatus::Success,
        error: None,
    };
    write_history_entry(&capture.history_dir, &mut entry, &change)?;
    prune_history(
        &capture.history_dir,
        HISTORY_MAX_AGE_SECS,
        HISTORY_MAX_BYTES,
    )?;
    progress(MapHistoryApplyProgress {
        phase: write_phase,
        completed: 1,
        total: 1,
    });
    Ok(entry)
}

pub(crate) fn complete_failed(
    capture: MapHistoryCapture,
    error: impl Into<String>,
) -> Result<MapHistoryEntry, String> {
    fs::create_dir_all(&capture.history_dir)
        .map_err(|error| format!("创建历史目录失败: {error}"))?;
    let error = error.into();
    let mut entry = MapHistoryEntry {
        id: capture.id,
        timestamp_secs: capture.timestamp_secs,
        kind: capture.kind,
        label: capture.label,
        message: error.clone(),
        world_path: capture.world_path.to_string_lossy().to_string(),
        chunks: capture.chunks.iter().copied().collect(),
        raw_delta_count: 0,
        raw_delta_bytes: 0,
        stored_bytes: 0,
        stored_object_count: 0,
        reused_object_count: 0,
        storage_format: HISTORY_STORAGE_OBJECT_STORE_V1.to_string(),
        level_dat_changed: false,
        status: MapHistoryEntryStatus::Failed,
        error: Some(error),
    };
    write_history_entry(
        &capture.history_dir,
        &mut entry,
        &MapHistoryChange {
            raw_records: Vec::new(),
            level_dat: None,
        },
    )?;
    Ok(entry)
}

pub(super) fn list_history(world_path: &Path) -> Result<Vec<MapHistoryEntry>, String> {
    let history_dir = history_dir_for_world(world_path);
    if !history_dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&history_dir).map_err(|error| format!("读取历史目录失败: {error}"))?
    {
        let entry = entry.map_err(|error| format!("读取历史项失败: {error}"))?;
        let path = entry.path().join(HISTORY_ENTRY_FILE);
        if !path.exists() {
            continue;
        }
        let raw = fs::read_to_string(&path)
            .map_err(|error| format!("读取历史索引失败 {}: {error}", path.display()))?;
        let parsed = serde_json::from_str::<MapHistoryEntry>(&raw)
            .map_err(|error| format!("解析历史索引失败 {}: {error}", path.display()))?;
        entries.push(parsed);
    }
    entries.sort_by(|left, right| right.timestamp_secs.cmp(&left.timestamp_secs));
    Ok(entries)
}

#[derive(Clone, Copy, Debug)]
struct HistoryChunkDeltaStats {
    added_records: usize,
    removed_records: usize,
    modified_records: usize,
    added_blocks: u32,
    removed_blocks: u32,
    modified_blocks: u32,
    columns: [MapHistoryColumnVisual; HISTORY_VISUAL_COLUMN_COUNT],
    precise_subchunks: usize,
    unresolved_terrain_records: usize,
    terrain_records: usize,
    block_entity_records: usize,
    entity_records: usize,
    metadata_records: usize,
}

impl Default for HistoryChunkDeltaStats {
    fn default() -> Self {
        Self {
            added_records: 0,
            removed_records: 0,
            modified_records: 0,
            added_blocks: 0,
            removed_blocks: 0,
            modified_blocks: 0,
            columns: [MapHistoryColumnVisual::default(); HISTORY_VISUAL_COLUMN_COUNT],
            precise_subchunks: 0,
            unresolved_terrain_records: 0,
            terrain_records: 0,
            block_entity_records: 0,
            entity_records: 0,
            metadata_records: 0,
        }
    }
}

impl HistoryChunkDeltaStats {
    fn add_record(&mut self, kind: MapHistoryVisualFilterKind) {
        match kind {
            MapHistoryVisualFilterKind::Added => {
                self.added_records = self.added_records.saturating_add(1);
            }
            MapHistoryVisualFilterKind::Removed => {
                self.removed_records = self.removed_records.saturating_add(1);
            }
            MapHistoryVisualFilterKind::Modified => {
                self.modified_records = self.modified_records.saturating_add(1);
            }
        }
    }

    fn add_block(&mut self, local_x: u8, local_z: u8, kind: MapHistoryVisualFilterKind) {
        match kind {
            MapHistoryVisualFilterKind::Added => {
                self.added_blocks = self.added_blocks.saturating_add(1);
            }
            MapHistoryVisualFilterKind::Removed => {
                self.removed_blocks = self.removed_blocks.saturating_add(1);
            }
            MapHistoryVisualFilterKind::Modified => {
                self.modified_blocks = self.modified_blocks.saturating_add(1);
            }
        }
        let index = usize::from(local_z)
            .saturating_mul(HISTORY_VISUAL_COLUMN_SIDE)
            .saturating_add(usize::from(local_x));
        if let Some(column) = self.columns.get_mut(index) {
            column.add(kind);
        }
    }

    fn add_record_category(&mut self, tag: ChunkRecordTag) {
        match tag {
            ChunkRecordTag::SubChunkPrefix
            | ChunkRecordTag::LegacyTerrain
            | ChunkRecordTag::Data3D
            | ChunkRecordTag::Data2D
            | ChunkRecordTag::Data2DLegacy
            | ChunkRecordTag::BiomeState => {
                self.terrain_records = self.terrain_records.saturating_add(1);
            }
            ChunkRecordTag::BlockEntity => {
                self.block_entity_records = self.block_entity_records.saturating_add(1);
            }
            ChunkRecordTag::Entity => {
                self.entity_records = self.entity_records.saturating_add(1);
            }
            _ => {
                self.metadata_records = self.metadata_records.saturating_add(1);
            }
        }
    }
}

pub(super) fn load_history_visualization(
    world_path: &Path,
    id: &str,
) -> Result<MapHistoryVisualization, String> {
    let history_dir = history_dir_for_world(world_path);
    let entry_path = history_dir.join(id).join(HISTORY_ENTRY_FILE);
    let entry_raw = fs::read_to_string(&entry_path)
        .map_err(|error| format!("读取历史索引失败 {}: {error}", entry_path.display()))?;
    let entry = serde_json::from_str::<MapHistoryEntry>(&entry_raw)
        .map_err(|error| format!("解析历史索引失败 {}: {error}", entry_path.display()))?;
    let change = read_history_change(&history_dir, id)?;
    let fallback_chunks = entry.chunks;
    let mut visualization = MapHistoryVisualization {
        level_dat_changed: change.level_dat.is_some(),
        ..MapHistoryVisualization::default()
    };
    let mut chunks = BTreeMap::<ChunkPos, HistoryChunkDeltaStats>::new();

    for delta in change.raw_records {
        let Some(change_kind) = history_raw_change_kind(&delta) else {
            continue;
        };
        history_add_visualization_record(&mut visualization, change_kind);

        let Ok(chunk_key) = ChunkKey::decode(&delta.key) else {
            visualization.unmapped_records = visualization.unmapped_records.saturating_add(1);
            visualization.metadata_records = visualization.metadata_records.saturating_add(1);
            continue;
        };

        let stats = chunks.entry(chunk_key.pos).or_default();
        stats.add_record(change_kind);
        stats.add_record_category(chunk_key.tag);
        history_add_visualization_category(&mut visualization, chunk_key.tag);

        match chunk_key.tag {
            ChunkRecordTag::SubChunkPrefix => {
                visualization.changed_subchunks = visualization.changed_subchunks.saturating_add(1);
                let Some(subchunk_y) = chunk_key.subchunk_y else {
                    stats.unresolved_terrain_records =
                        stats.unresolved_terrain_records.saturating_add(1);
                    continue;
                };
                if accumulate_subchunk_delta(
                    stats,
                    subchunk_y,
                    delta.before.as_ref(),
                    delta.after.as_ref(),
                ) {
                    stats.precise_subchunks = stats.precise_subchunks.saturating_add(1);
                } else {
                    stats.unresolved_terrain_records =
                        stats.unresolved_terrain_records.saturating_add(1);
                }
            }
            ChunkRecordTag::LegacyTerrain => {
                if accumulate_legacy_terrain_delta(
                    stats,
                    delta.before.as_ref(),
                    delta.after.as_ref(),
                ) {
                    stats.precise_subchunks = stats.precise_subchunks.saturating_add(1);
                } else {
                    stats.unresolved_terrain_records =
                        stats.unresolved_terrain_records.saturating_add(1);
                }
            }
            ChunkRecordTag::Data3D
            | ChunkRecordTag::Data2D
            | ChunkRecordTag::Data2DLegacy
            | ChunkRecordTag::BiomeState => {
                stats.unresolved_terrain_records =
                    stats.unresolved_terrain_records.saturating_add(1);
            }
            _ => {}
        }
    }

    if chunks.is_empty() && visualization.total_records() > 0 {
        for chunk in fallback_chunks {
            let stats = chunks.entry(chunk).or_default();
            stats.metadata_records = stats.metadata_records.saturating_add(1);
            stats.modified_records = stats.modified_records.saturating_add(1);
        }
    }

    visualization.chunks = chunks
        .into_iter()
        .map(|(pos, stats)| history_chunk_visual(pos, stats))
        .collect();

    for chunk in &visualization.chunks {
        visualization.added_blocks = visualization
            .added_blocks
            .saturating_add(u64::from(chunk.added_blocks));
        visualization.removed_blocks = visualization
            .removed_blocks
            .saturating_add(u64::from(chunk.removed_blocks));
        visualization.modified_blocks = visualization
            .modified_blocks
            .saturating_add(u64::from(chunk.modified_blocks));
        if chunk.kind == MapHistoryChunkVisualKind::Mixed {
            visualization.mixed_chunks = visualization.mixed_chunks.saturating_add(1);
        }
        if chunk.precise_subchunks > 0 && chunk.unresolved_terrain_records == 0 {
            visualization.precise_chunks = visualization.precise_chunks.saturating_add(1);
        } else if chunk.precise_subchunks > 0 {
            visualization.partial_chunks = visualization.partial_chunks.saturating_add(1);
        } else {
            visualization.record_only_chunks = visualization.record_only_chunks.saturating_add(1);
        }
    }

    Ok(visualization)
}

fn history_raw_change_kind(delta: &RawRecordDelta) -> Option<MapHistoryVisualFilterKind> {
    match (delta.before.as_ref(), delta.after.as_ref()) {
        (None, Some(_)) => Some(MapHistoryVisualFilterKind::Added),
        (Some(_), None) => Some(MapHistoryVisualFilterKind::Removed),
        (Some(before), Some(after)) if before != after => {
            Some(MapHistoryVisualFilterKind::Modified)
        }
        _ => None,
    }
}

fn history_add_visualization_record(
    visualization: &mut MapHistoryVisualization,
    kind: MapHistoryVisualFilterKind,
) {
    match kind {
        MapHistoryVisualFilterKind::Added => {
            visualization.added_records = visualization.added_records.saturating_add(1);
        }
        MapHistoryVisualFilterKind::Removed => {
            visualization.removed_records = visualization.removed_records.saturating_add(1);
        }
        MapHistoryVisualFilterKind::Modified => {
            visualization.modified_records = visualization.modified_records.saturating_add(1);
        }
    }
}

fn history_add_visualization_category(
    visualization: &mut MapHistoryVisualization,
    tag: ChunkRecordTag,
) {
    match tag {
        ChunkRecordTag::SubChunkPrefix
        | ChunkRecordTag::LegacyTerrain
        | ChunkRecordTag::Data3D
        | ChunkRecordTag::Data2D
        | ChunkRecordTag::Data2DLegacy
        | ChunkRecordTag::BiomeState => {
            visualization.terrain_records = visualization.terrain_records.saturating_add(1);
        }
        ChunkRecordTag::BlockEntity => {
            visualization.block_entity_records =
                visualization.block_entity_records.saturating_add(1);
        }
        ChunkRecordTag::Entity => {
            visualization.entity_records = visualization.entity_records.saturating_add(1);
        }
        _ => {
            visualization.metadata_records = visualization.metadata_records.saturating_add(1);
        }
    }
}

fn history_chunk_visual(pos: ChunkPos, stats: HistoryChunkDeltaStats) -> MapHistoryChunkVisual {
    let kind = history_visual_kind(
        if stats.added_blocks > 0 {
            stats.added_blocks
        } else {
            u32::try_from(stats.added_records).unwrap_or(u32::MAX)
        },
        if stats.removed_blocks > 0 {
            stats.removed_blocks
        } else {
            u32::try_from(stats.removed_records).unwrap_or(u32::MAX)
        },
        if stats.modified_blocks > 0 {
            stats.modified_blocks
        } else {
            u32::try_from(stats.modified_records).unwrap_or(u32::MAX)
        },
    )
    .unwrap_or(MapHistoryChunkVisualKind::Modified);
    let max_column_blocks = stats
        .columns
        .iter()
        .map(|column| column.total_blocks())
        .max()
        .and_then(|count| u16::try_from(count).ok())
        .unwrap_or(u16::MAX);

    MapHistoryChunkVisual {
        pos,
        kind,
        added_records: stats.added_records,
        removed_records: stats.removed_records,
        modified_records: stats.modified_records,
        added_blocks: stats.added_blocks,
        removed_blocks: stats.removed_blocks,
        modified_blocks: stats.modified_blocks,
        columns: stats.columns,
        max_column_blocks,
        precise_subchunks: stats.precise_subchunks,
        unresolved_terrain_records: stats.unresolved_terrain_records,
        terrain_records: stats.terrain_records,
        block_entity_records: stats.block_entity_records,
        entity_records: stats.entity_records,
        metadata_records: stats.metadata_records,
    }
}

fn history_visual_kind(
    added: u32,
    removed: u32,
    modified: u32,
) -> Option<MapHistoryChunkVisualKind> {
    let kinds = [added > 0, removed > 0, modified > 0]
        .into_iter()
        .filter(|present| *present)
        .count();
    match kinds {
        0 => None,
        1 if added > 0 => Some(MapHistoryChunkVisualKind::Added),
        1 if removed > 0 => Some(MapHistoryChunkVisualKind::Removed),
        1 => Some(MapHistoryChunkVisualKind::Modified),
        _ => Some(MapHistoryChunkVisualKind::Mixed),
    }
}

fn accumulate_subchunk_delta(
    stats: &mut HistoryChunkDeltaStats,
    subchunk_y: i8,
    before: Option<&Vec<u8>>,
    after: Option<&Vec<u8>>,
) -> bool {
    let (before, before_decoded) = decode_history_subchunk(subchunk_y, before);
    let (after, after_decoded) = decode_history_subchunk(subchunk_y, after);
    if !before_decoded || !after_decoded {
        return false;
    }

    for local_x in 0_u8..16 {
        for local_z in 0_u8..16 {
            for local_y in 0_u8..16 {
                let Some(kind) = classify_history_subchunk_block(
                    before.as_ref(),
                    after.as_ref(),
                    local_x,
                    local_y,
                    local_z,
                ) else {
                    continue;
                };
                stats.add_block(local_x, local_z, kind);
            }
        }
    }
    true
}

fn decode_history_subchunk(
    y: i8,
    bytes: Option<&Vec<u8>>,
) -> (Option<bedrock_world::SubChunk>, bool) {
    let Some(bytes) = bytes else {
        return (None, true);
    };
    let Ok(subchunk) = bedrock_world::chunk::parse_subchunk_with_mode(
        y,
        bytes::Bytes::copy_from_slice(bytes),
        bedrock_world::SubChunkDecodeMode::PackedIndices,
    ) else {
        return (None, false);
    };
    let decoded = matches!(
        &subchunk.format,
        bedrock_world::SubChunkFormat::Paletted { .. }
            | bedrock_world::SubChunkFormat::LegacySubChunk(_)
    );
    if decoded {
        (Some(subchunk), true)
    } else {
        (None, false)
    }
}

fn classify_history_subchunk_block(
    before: Option<&bedrock_world::SubChunk>,
    after: Option<&bedrock_world::SubChunk>,
    local_x: u8,
    local_y: u8,
    local_z: u8,
) -> Option<MapHistoryVisualFilterKind> {
    let before_air = history_subchunk_block_is_air(before, local_x, local_y, local_z)?;
    let after_air = history_subchunk_block_is_air(after, local_x, local_y, local_z)?;
    if before_air && after_air {
        return None;
    }
    if before_air {
        return Some(MapHistoryVisualFilterKind::Added);
    }
    if after_air {
        return Some(MapHistoryVisualFilterKind::Removed);
    }
    if let (Some(before), Some(after)) = (before, after)
        && history_subchunk_blocks_equal(before, after, local_x, local_y, local_z) == Some(true)
    {
        return None;
    }
    Some(MapHistoryVisualFilterKind::Modified)
}

fn history_block_name_is_air(name: &str) -> bool {
    matches!(
        name,
        "air"
            | "cave_air"
            | "void_air"
            | "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "minecraft:structure_void"
            | "minecraft:light_block"
            | "minecraft:light"
    )
}

fn history_subchunk_block_is_air(
    subchunk: Option<&bedrock_world::SubChunk>,
    local_x: u8,
    local_y: u8,
    local_z: u8,
) -> Option<bool> {
    let Some(subchunk) = subchunk else {
        return Some(true);
    };
    match &subchunk.format {
        bedrock_world::SubChunkFormat::Paletted { storages, .. } => {
            if storages.is_empty() {
                return None;
            }
            let mut all_air = true;
            for storage in storages {
                let state = storage.block_state_at(local_x, local_y, local_z)?;
                if !history_block_name_is_air(&state.name) {
                    all_air = false;
                }
            }
            Some(all_air)
        }
        bedrock_world::SubChunkFormat::LegacySubChunk(_) => subchunk
            .legacy_block_id_at(local_x, local_y, local_z)
            .map(|id| id == 0),
        _ => None,
    }
}

fn history_subchunk_blocks_equal(
    before: &bedrock_world::SubChunk,
    after: &bedrock_world::SubChunk,
    local_x: u8,
    local_y: u8,
    local_z: u8,
) -> Option<bool> {
    match (&before.format, &after.format) {
        (
            bedrock_world::SubChunkFormat::Paletted {
                storages: before_storages,
                ..
            },
            bedrock_world::SubChunkFormat::Paletted {
                storages: after_storages,
                ..
            },
        ) => {
            if before_storages.len() != after_storages.len() {
                return Some(false);
            }
            for (before_storage, after_storage) in before_storages.iter().zip(after_storages.iter())
            {
                let before_state = before_storage.block_state_at(local_x, local_y, local_z)?;
                let after_state = after_storage.block_state_at(local_x, local_y, local_z)?;
                if before_state != after_state {
                    return Some(false);
                }
            }
            Some(true)
        }
        (
            bedrock_world::SubChunkFormat::LegacySubChunk(_),
            bedrock_world::SubChunkFormat::LegacySubChunk(_),
        ) => Some(
            before.legacy_block_id_at(local_x, local_y, local_z)?
                == after.legacy_block_id_at(local_x, local_y, local_z)?
                && before.legacy_block_data_at(local_x, local_y, local_z)
                    == after.legacy_block_data_at(local_x, local_y, local_z),
        ),
        _ => {
            let before_air =
                history_subchunk_block_is_air(Some(before), local_x, local_y, local_z)?;
            let after_air = history_subchunk_block_is_air(Some(after), local_x, local_y, local_z)?;
            Some(before_air && after_air)
        }
    }
}

fn accumulate_legacy_terrain_delta(
    stats: &mut HistoryChunkDeltaStats,
    before: Option<&Vec<u8>>,
    after: Option<&Vec<u8>>,
) -> bool {
    let (before, before_decoded) = decode_history_legacy_terrain(before);
    let (after, after_decoded) = decode_history_legacy_terrain(after);
    if !before_decoded || !after_decoded {
        return false;
    }

    for local_x in 0_u8..16 {
        for local_z in 0_u8..16 {
            for local_y in 0_u8..128 {
                let before_id = before
                    .as_ref()
                    .and_then(|terrain| terrain.block_id_at(local_x, local_y, local_z))
                    .unwrap_or(0);
                let after_id = after
                    .as_ref()
                    .and_then(|terrain| terrain.block_id_at(local_x, local_y, local_z))
                    .unwrap_or(0);
                let before_data = before
                    .as_ref()
                    .and_then(|terrain| terrain.block_data_at(local_x, local_y, local_z))
                    .unwrap_or(0);
                let after_data = after
                    .as_ref()
                    .and_then(|terrain| terrain.block_data_at(local_x, local_y, local_z))
                    .unwrap_or(0);
                let kind = if before_id == 0 && after_id == 0 {
                    None
                } else if before_id == 0 {
                    Some(MapHistoryVisualFilterKind::Added)
                } else if after_id == 0 {
                    Some(MapHistoryVisualFilterKind::Removed)
                } else if before_id != after_id || before_data != after_data {
                    Some(MapHistoryVisualFilterKind::Modified)
                } else {
                    None
                };
                if let Some(kind) = kind {
                    stats.add_block(local_x, local_z, kind);
                }
            }
        }
    }
    true
}

fn decode_history_legacy_terrain(
    bytes: Option<&Vec<u8>>,
) -> (Option<bedrock_world::LegacyTerrain>, bool) {
    let Some(bytes) = bytes else {
        return (None, true);
    };
    match bedrock_world::LegacyTerrain::parse(bytes::Bytes::copy_from_slice(bytes)) {
        Ok(terrain) => (Some(terrain), true),
        Err(_) => (None, false),
    }
}

pub(super) fn apply_undo_with_progress(
    world_path: &Path,
    progress: impl FnMut(MapHistoryApplyProgress),
) -> Result<MapHistoryApplyOutcome, String> {
    let Some(entry) = list_history(world_path)?
        .into_iter()
        .find(|entry| entry.status == MapHistoryEntryStatus::Success)
    else {
        return Err("没有可撤回的地图修改".to_string());
    };
    let applied_change = apply_history_entry(world_path, &entry, true, progress)?;
    mark_entry_status(world_path, &entry.id, MapHistoryEntryStatus::Undone, None)?;
    Ok(MapHistoryApplyOutcome {
        affected_chunks: applied_change.affected_chunks,
        refresh_all_tiles: applied_change.refresh_all_tiles,
        level_dat_changed: applied_change.level_dat_changed,
        message: format!("已撤回 {}", entry.label),
    })
}

pub(super) fn apply_redo_with_progress(
    world_path: &Path,
    progress: impl FnMut(MapHistoryApplyProgress),
) -> Result<MapHistoryApplyOutcome, String> {
    let Some(entry) = list_history(world_path)?
        .into_iter()
        .find(|entry| entry.status == MapHistoryEntryStatus::Undone)
    else {
        return Err("没有可重做的地图修改".to_string());
    };
    let applied_change = apply_history_entry(world_path, &entry, false, progress)?;
    mark_entry_status(world_path, &entry.id, MapHistoryEntryStatus::Success, None)?;
    Ok(MapHistoryApplyOutcome {
        affected_chunks: applied_change.affected_chunks,
        refresh_all_tiles: applied_change.refresh_all_tiles,
        level_dat_changed: applied_change.level_dat_changed,
        message: format!("已重做 {}", entry.label),
    })
}

pub(super) fn restore_history_entry_with_progress(
    world_path: &Path,
    entry_id: &str,
    progress: impl FnMut(MapHistoryApplyProgress),
) -> Result<MapHistoryApplyOutcome, String> {
    let entries = list_history(world_path)?;
    let Some(entry) = entries.into_iter().find(|entry| entry.id == entry_id) else {
        return Err("找不到要回档的历史项".to_string());
    };
    if entry.status == MapHistoryEntryStatus::Failed {
        return Err("失败的历史项不能回档".to_string());
    }
    let applied_change = apply_history_entry(world_path, &entry, true, progress)?;
    Ok(MapHistoryApplyOutcome {
        affected_chunks: applied_change.affected_chunks,
        refresh_all_tiles: applied_change.refresh_all_tiles,
        level_dat_changed: applied_change.level_dat_changed,
        message: format!("已回档到 {}", entry.label),
    })
}

pub(super) fn create_restore_protection_point(
    world_path: &Path,
    chunks: BTreeSet<ChunkPos>,
    label: impl Into<String>,
) -> Result<MapHistoryEntry, String> {
    let capture = capture_before(MapHistoryCaptureSpec {
        kind: MapHistoryEntryKind::RestorePoint,
        label: label.into(),
        world_path: world_path.to_path_buf(),
        chunks,
        raw_keys: BTreeSet::new(),
        include_level_dat: true,
    })?;
    complete_snapshot(capture, "回档前保护点")
}

pub(super) fn history_dir_for_world(world_path: &Path) -> PathBuf {
    crate::utils::file_ops::bmcbl_subdir("map_history").join(world_history_id(world_path))
}

pub(super) fn prune_history(
    history_dir: &Path,
    max_age_secs: u64,
    max_bytes: u64,
) -> Result<(), String> {
    if !history_dir.exists() {
        return Ok(());
    }
    let now = now_secs();
    let mut entries = Vec::new();
    for dir_entry in
        fs::read_dir(history_dir).map_err(|error| format!("读取历史目录失败: {error}"))?
    {
        let dir_entry = dir_entry.map_err(|error| format!("读取历史目录项失败: {error}"))?;
        let path = dir_entry.path();
        if !path.is_dir() || !path.join(HISTORY_ENTRY_FILE).exists() {
            continue;
        }
        let size = dir_size(&path)?;
        let entry = fs::read_to_string(path.join(HISTORY_ENTRY_FILE))
            .ok()
            .and_then(|raw| serde_json::from_str::<MapHistoryEntry>(&raw).ok());
        let timestamp = entry.as_ref().map_or(0, |entry| entry.timestamp_secs);
        entries.push((path, timestamp, size));
    }
    for (path, timestamp, _) in &entries {
        if timestamp.saturating_add(max_age_secs) < now {
            remove_dir_all_if_exists(path)?;
        }
    }
    entries.retain(|(path, timestamp, _)| {
        path.exists() && timestamp.saturating_add(max_age_secs) >= now
    });
    entries.sort_by_key(|(_, timestamp, _)| *timestamp);
    let mut total = entries
        .iter()
        .fold(0u64, |total, (_, _, size)| total.saturating_add(*size));
    for (path, _, size) in entries {
        if total <= max_bytes {
            break;
        }
        remove_dir_all_if_exists(&path)?;
        total = total.saturating_sub(size);
    }
    prune_unreferenced_history_objects(history_dir)?;
    Ok(())
}

fn apply_history_entry(
    world_path: &Path,
    entry: &MapHistoryEntry,
    undo: bool,
    mut progress: impl FnMut(MapHistoryApplyProgress),
) -> Result<MapHistoryAppliedChange, String> {
    let change = read_history_change(&history_dir_for_world(world_path), &entry.id)?;
    let applied_change = history_applied_change(entry.chunks.iter().copied(), &change);
    let raw_total = change.raw_records.len();
    let total = raw_total
        .saturating_add(usize::from(change.level_dat.is_some()))
        .max(1);
    let phase = SharedString::from(if undo {
        "应用历史回档"
    } else {
        "应用历史重做"
    });
    progress(MapHistoryApplyProgress {
        phase: phase.clone(),
        completed: 0,
        total,
    });
    let mut options = bedrock_world::BedrockWorldOpenOptions::default();
    options.read_only = false;
    let world = BedrockWorld::open_blocking(world_path, options)
        .map_err(|error| format!("打开世界失败: {error}"))?;
    let mut completed = 0usize;
    for batch in change.raw_records.chunks(HISTORY_APPLY_BATCH_RECORDS) {
        let mut storage_batch = bedrock_world::StorageBatch::new();
        for delta in batch {
            let value = if undo {
                delta.before.as_ref()
            } else {
                delta.after.as_ref()
            };
            apply_history_raw_delta(&mut storage_batch, delta.key.as_slice(), value)?;
        }
        world
            .storage()
            .write_batch(&storage_batch)
            .map_err(|error| {
                let first_key = batch
                    .first()
                    .map(|delta| history_key_label(&delta.key))
                    .unwrap_or_else(|| "<empty batch>".to_string());
                let last_key = batch
                    .last()
                    .map(|delta| history_key_label(&delta.key))
                    .unwrap_or_else(|| first_key.clone());
                format!(
                    "应用历史记录失败: {error}（批次 {}..{}，{} 条）",
                    first_key,
                    last_key,
                    batch.len()
                )
            })?;
        completed = completed.saturating_add(batch.len());
        progress(MapHistoryApplyProgress {
            phase: phase.clone(),
            completed,
            total,
        });
    }
    if let Some(delta) = change.level_dat {
        let value = if undo { delta.before } else { delta.after };
        write_optional_file(world_path.join("level.dat"), value)?;
        progress(MapHistoryApplyProgress {
            phase,
            completed: raw_total.saturating_add(1),
            total,
        });
    } else if raw_total == 0 {
        progress(MapHistoryApplyProgress {
            phase,
            completed: 1,
            total,
        });
    }
    Ok(applied_change)
}

fn history_applied_change(
    entry_chunks: impl IntoIterator<Item = ChunkPos>,
    change: &MapHistoryChange,
) -> MapHistoryAppliedChange {
    let mut affected_chunks = entry_chunks.into_iter().collect::<BTreeSet<_>>();
    let mut has_raw_records = false;
    let mut has_unmapped_raw_records = false;
    for delta in &change.raw_records {
        has_raw_records = true;
        if let Some(chunk) = history_delta_chunk(&delta.key) {
            affected_chunks.insert(chunk);
        } else {
            has_unmapped_raw_records = true;
        }
    }
    let refresh_all_tiles =
        has_raw_records && affected_chunks.is_empty() && has_unmapped_raw_records;
    MapHistoryAppliedChange {
        affected_chunks,
        refresh_all_tiles,
        level_dat_changed: change.level_dat.is_some(),
    }
}

fn history_delta_chunk(key: &[u8]) -> Option<ChunkPos> {
    match bedrock_world::BedrockDbKey::decode(key) {
        bedrock_world::BedrockDbKey::Chunk(chunk_key) => Some(chunk_key.pos),
        bedrock_world::BedrockDbKey::ActorDigest { pos } => Some(pos),
        _ => None,
    }
}

fn apply_history_raw_delta(
    storage_batch: &mut bedrock_world::StorageBatch,
    key: &[u8],
    value: Option<&Vec<u8>>,
) -> Result<(), String> {
    if key.is_empty() {
        return Err("应用历史记录失败: 历史项包含空 LevelDB key".to_string());
    }
    match value {
        Some(value) => {
            storage_batch.put(Bytes::copy_from_slice(key), Bytes::copy_from_slice(value));
            Ok(())
        }
        None => {
            storage_batch.delete(Bytes::copy_from_slice(key));
            Ok(())
        }
    }
}

fn history_key_label(key: &[u8]) -> String {
    let hex = hex::encode(key);
    if hex.len() > 48 {
        format!("{}...", &hex[..48])
    } else {
        hex
    }
}

fn build_raw_record_deltas(
    before_raw_records: &BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    raw_keys: &BTreeSet<Vec<u8>>,
    mut read_after: impl FnMut(&[u8]) -> Result<Option<Vec<u8>>, String>,
) -> Result<(Vec<RawRecordDelta>, u64), String> {
    let mut raw_records = Vec::new();
    let mut raw_delta_bytes = 0u64;
    for key in raw_keys {
        let after = read_after(key)?;
        let before = before_raw_records.get(key).cloned().unwrap_or(None);
        if before != after {
            raw_delta_bytes = raw_delta_bytes
                .saturating_add(key.len() as u64)
                .saturating_add(before.as_ref().map_or(0, |value| value.len() as u64))
                .saturating_add(after.as_ref().map_or(0, |value| value.len() as u64));
            raw_records.push(RawRecordDelta {
                key: key.clone(),
                before,
                after,
            });
        }
    }
    Ok((raw_records, raw_delta_bytes))
}

fn complete_snapshot(
    capture: MapHistoryCapture,
    message: impl Into<String>,
) -> Result<MapHistoryEntry, String> {
    fs::create_dir_all(&capture.history_dir)
        .map_err(|error| format!("创建历史目录失败: {error}"))?;
    let raw_delta_bytes = capture
        .before_raw_records
        .iter()
        .fold(0u64, |total, (key, value)| {
            total
                .saturating_add(key.len() as u64)
                .saturating_add(value.as_ref().map_or(0, |value| value.len() as u64))
                .saturating_add(value.as_ref().map_or(0, |value| value.len() as u64))
        });
    let raw_records = capture
        .before_raw_records
        .iter()
        .map(|(key, value)| RawRecordDelta {
            key: key.clone(),
            before: value.clone(),
            after: value.clone(),
        })
        .collect::<Vec<_>>();
    let level_dat = capture.before_level_dat.map(|value| LevelDatDelta {
        before: value.clone(),
        after: value,
    });
    let change = MapHistoryChange {
        raw_records,
        level_dat,
    };
    let mut entry = MapHistoryEntry {
        id: capture.id,
        timestamp_secs: capture.timestamp_secs,
        kind: capture.kind,
        label: capture.label,
        message: message.into(),
        world_path: capture.world_path.to_string_lossy().to_string(),
        chunks: capture.chunks.iter().copied().collect(),
        raw_delta_count: change.raw_records.len(),
        raw_delta_bytes,
        stored_bytes: 0,
        stored_object_count: 0,
        reused_object_count: 0,
        storage_format: HISTORY_STORAGE_OBJECT_STORE_V1.to_string(),
        level_dat_changed: change.level_dat.is_some(),
        status: MapHistoryEntryStatus::Success,
        error: None,
    };
    write_history_entry(&capture.history_dir, &mut entry, &change)?;
    prune_history(
        &capture.history_dir,
        HISTORY_MAX_AGE_SECS,
        HISTORY_MAX_BYTES,
    )?;
    Ok(entry)
}

fn collect_chunk_raw_keys(
    world: &BedrockWorld,
    chunk: ChunkPos,
) -> Result<BTreeSet<Vec<u8>>, String> {
    let mut keys = BTreeSet::new();
    let records = world
        .get_chunk_blocking(chunk)
        .map_err(|error| format!("读取 chunk 历史记录失败: {error}"))?
        .records;
    keys.extend(
        records
            .into_iter()
            .map(|record| record.key.encode().to_vec()),
    );

    let digest_key = ActorDigestKey::new(chunk).storage_key();
    keys.insert(digest_key.to_vec());
    for actor in world
        .actors_in_chunk_blocking(chunk)
        .map_err(|error| format!("读取实体历史记录失败: {error}"))?
    {
        if let Some(uid) = actor.uid {
            keys.insert(uid.storage_key().to_vec());
        }
    }
    Ok(keys)
}

fn open_world_readonly(world_path: &Path) -> Result<BedrockWorld, String> {
    let mut options = bedrock_world::BedrockWorldOpenOptions::default();
    options.read_only = true;
    BedrockWorld::open_blocking(world_path, options)
        .map_err(|error| format!("打开世界失败: {error}"))
}

fn write_history_entry(
    history_dir: &Path,
    entry: &mut MapHistoryEntry,
    change: &MapHistoryChange,
) -> Result<(), String> {
    let entry_dir = history_dir.join(&entry.id);
    fs::create_dir_all(&entry_dir).map_err(|error| format!("创建历史项目录失败: {error}"))?;
    let mut stats = HistoryStorageStats::default();
    let stored_change = store_history_change(history_dir, change, &mut stats)?;
    let change_json = serde_json::to_vec(&stored_change)
        .map_err(|error| format!("序列化历史变更失败: {error}"))?;
    let compressed = zstd::encode_all(change_json.as_slice(), 3)
        .map_err(|error| format!("压缩历史变更失败: {error}"))?;
    write_atomic(entry_dir.join(HISTORY_DELTA_FILE), &compressed)?;
    entry.stored_bytes = compressed
        .len()
        .try_into()
        .unwrap_or(u64::MAX)
        .saturating_add(stats.stored_bytes);
    entry.stored_object_count = stats.stored_object_count;
    entry.reused_object_count = stats.reused_object_count;
    entry.storage_format = HISTORY_STORAGE_OBJECT_STORE_V1.to_string();
    let entry_json =
        serde_json::to_vec_pretty(entry).map_err(|error| format!("序列化历史索引失败: {error}"))?;
    write_atomic(entry_dir.join(HISTORY_ENTRY_FILE), &entry_json)
}

fn read_history_change(history_dir: &Path, id: &str) -> Result<MapHistoryChange, String> {
    let compressed = fs::read(history_dir.join(id).join(HISTORY_DELTA_FILE))
        .map_err(|error| format!("读取历史变更失败: {error}"))?;
    let raw = zstd::decode_all(compressed.as_slice())
        .map_err(|error| format!("解压历史变更失败: {error}"))?;
    if let Ok(stored_change) = serde_json::from_slice::<MapHistoryStoredChange>(&raw) {
        return load_history_change(history_dir, stored_change);
    }
    serde_json::from_slice::<MapHistoryChange>(&raw)
        .map_err(|error| format!("解析历史变更失败: {error}"))
}

fn store_history_change(
    history_dir: &Path,
    change: &MapHistoryChange,
    stats: &mut HistoryStorageStats,
) -> Result<MapHistoryStoredChange, String> {
    let raw_records = change
        .raw_records
        .iter()
        .map(|delta| {
            Ok(StoredRawRecordDelta {
                key: delta.key.clone(),
                before: store_history_optional_value(history_dir, delta.before.as_deref(), stats)?,
                after: store_history_optional_value(history_dir, delta.after.as_deref(), stats)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let level_dat = change
        .level_dat
        .as_ref()
        .map(|delta| {
            Ok::<StoredLevelDatDelta, String>(StoredLevelDatDelta {
                before: store_history_optional_value(history_dir, delta.before.as_deref(), stats)?,
                after: store_history_optional_value(history_dir, delta.after.as_deref(), stats)?,
            })
        })
        .transpose()?;

    Ok(MapHistoryStoredChange {
        storage_format: HISTORY_STORAGE_OBJECT_STORE_V1.to_string(),
        raw_records,
        level_dat,
    })
}

fn load_history_change(
    history_dir: &Path,
    stored_change: MapHistoryStoredChange,
) -> Result<MapHistoryChange, String> {
    let raw_records = stored_change
        .raw_records
        .into_iter()
        .map(|delta| {
            Ok(RawRecordDelta {
                key: delta.key,
                before: load_history_optional_value(history_dir, delta.before)?,
                after: load_history_optional_value(history_dir, delta.after)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let level_dat = stored_change
        .level_dat
        .map(|delta| {
            Ok::<LevelDatDelta, String>(LevelDatDelta {
                before: load_history_optional_value(history_dir, delta.before)?,
                after: load_history_optional_value(history_dir, delta.after)?,
            })
        })
        .transpose()?;

    Ok(MapHistoryChange {
        raw_records,
        level_dat,
    })
}

fn store_history_optional_value(
    history_dir: &Path,
    value: Option<&[u8]>,
    stats: &mut HistoryStorageStats,
) -> Result<Option<StoredHistoryValue>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.len() < HISTORY_OBJECT_MIN_BYTES {
        return Ok(Some(StoredHistoryValue::Inline(value.to_vec())));
    }
    let reference = write_history_object(history_dir, value, stats)?;
    Ok(Some(StoredHistoryValue::Blob(reference)))
}

fn load_history_optional_value(
    history_dir: &Path,
    value: Option<StoredHistoryValue>,
) -> Result<Option<Vec<u8>>, String> {
    match value {
        Some(StoredHistoryValue::Blob(reference)) => {
            read_history_object(history_dir, &reference).map(Some)
        }
        Some(StoredHistoryValue::Inline(value)) => Ok(Some(value)),
        None => Ok(None),
    }
}

fn write_history_object(
    history_dir: &Path,
    value: &[u8],
    stats: &mut HistoryStorageStats,
) -> Result<HistoryBlobRef, String> {
    let sha256 = history_sha256_hex(value);
    let path = history_object_path(history_dir, &sha256);
    if path.exists() {
        stats.reused_object_count = stats.reused_object_count.saturating_add(1);
        return Ok(HistoryBlobRef {
            sha256,
            size: value.len().try_into().unwrap_or(u64::MAX),
        });
    }

    let compressed =
        zstd::encode_all(value, 3).map_err(|error| format!("压缩历史对象失败: {error}"))?;
    write_atomic(&path, &compressed)?;
    stats.stored_bytes = stats
        .stored_bytes
        .saturating_add(compressed.len().try_into().unwrap_or(u64::MAX));
    stats.stored_object_count = stats.stored_object_count.saturating_add(1);
    Ok(HistoryBlobRef {
        sha256,
        size: value.len().try_into().unwrap_or(u64::MAX),
    })
}

fn read_history_object(history_dir: &Path, reference: &HistoryBlobRef) -> Result<Vec<u8>, String> {
    let path = history_object_path(history_dir, &reference.sha256);
    let compressed =
        fs::read(&path).map_err(|error| format!("读取历史对象失败 {}: {error}", path.display()))?;
    let value = zstd::decode_all(compressed.as_slice())
        .map_err(|error| format!("解压历史对象失败 {}: {error}", path.display()))?;
    let actual_size: u64 = value.len().try_into().unwrap_or(u64::MAX);
    if actual_size != reference.size {
        return Err(format!(
            "历史对象大小校验失败 {}: expected {}, got {}",
            reference.sha256, reference.size, actual_size
        ));
    }
    let actual_hash = history_sha256_hex(&value);
    if actual_hash != reference.sha256 {
        return Err(format!(
            "历史对象 SHA-256 校验失败: expected {}, got {}",
            reference.sha256, actual_hash
        ));
    }
    Ok(value)
}

fn history_sha256_hex(value: &[u8]) -> String {
    let digest = sha2::Sha256::digest(value);
    hex::encode(digest)
}

fn history_object_path(history_dir: &Path, sha256: &str) -> PathBuf {
    let shard = sha256.get(..2).unwrap_or("00");
    history_dir
        .join(HISTORY_OBJECT_STORE_DIR)
        .join(shard)
        .join(format!("{sha256}.zst"))
}

fn prune_unreferenced_history_objects(history_dir: &Path) -> Result<(), String> {
    let object_dir = history_dir.join(HISTORY_OBJECT_STORE_DIR);
    if !object_dir.exists() {
        return Ok(());
    }

    let mut referenced = BTreeSet::new();
    for dir_entry in
        fs::read_dir(history_dir).map_err(|error| format!("读取历史目录失败: {error}"))?
    {
        let dir_entry = dir_entry.map_err(|error| format!("读取历史目录项失败: {error}"))?;
        let entry_dir = dir_entry.path();
        if !entry_dir.is_dir() || !entry_dir.join(HISTORY_ENTRY_FILE).exists() {
            continue;
        }
        let delta_path = entry_dir.join(HISTORY_DELTA_FILE);
        let Ok(compressed) = fs::read(delta_path) else {
            continue;
        };
        let Ok(raw) = zstd::decode_all(compressed.as_slice()) else {
            continue;
        };
        let Ok(stored_change) = serde_json::from_slice::<MapHistoryStoredChange>(&raw) else {
            continue;
        };
        collect_stored_change_hashes(&stored_change, &mut referenced);
    }

    remove_unreferenced_history_objects(&object_dir, &referenced)
}

fn collect_stored_change_hashes(
    stored_change: &MapHistoryStoredChange,
    hashes: &mut BTreeSet<String>,
) {
    for delta in &stored_change.raw_records {
        collect_stored_value_hash(delta.before.as_ref(), hashes);
        collect_stored_value_hash(delta.after.as_ref(), hashes);
    }
    if let Some(delta) = &stored_change.level_dat {
        collect_stored_value_hash(delta.before.as_ref(), hashes);
        collect_stored_value_hash(delta.after.as_ref(), hashes);
    }
}

fn collect_stored_value_hash(value: Option<&StoredHistoryValue>, hashes: &mut BTreeSet<String>) {
    if let Some(StoredHistoryValue::Blob(reference)) = value {
        hashes.insert(reference.sha256.clone());
    }
}

fn remove_unreferenced_history_objects(
    object_dir: &Path,
    referenced: &BTreeSet<String>,
) -> Result<(), String> {
    for shard_entry in
        fs::read_dir(object_dir).map_err(|error| format!("读取历史对象目录失败: {error}"))?
    {
        let shard_entry =
            shard_entry.map_err(|error| format!("读取历史对象目录项失败: {error}"))?;
        let shard_path = shard_entry.path();
        if !shard_path.is_dir() {
            continue;
        }
        for object_entry in
            fs::read_dir(&shard_path).map_err(|error| format!("读取历史对象分片失败: {error}"))?
        {
            let object_entry =
                object_entry.map_err(|error| format!("读取历史对象项失败: {error}"))?;
            let path = object_entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if !referenced.contains(stem) {
                fs::remove_file(&path)
                    .map_err(|error| format!("删除历史对象失败 {}: {error}", path.display()))?;
            }
        }
        if shard_path
            .read_dir()
            .map_err(|error| format!("读取历史对象分片失败: {error}"))?
            .next()
            .is_none()
        {
            fs::remove_dir(&shard_path).map_err(|error| {
                format!("删除历史对象分片失败 {}: {error}", shard_path.display())
            })?;
        }
    }
    Ok(())
}

fn mark_entry_status(
    world_path: &Path,
    id: &str,
    status: MapHistoryEntryStatus,
    error: Option<String>,
) -> Result<(), String> {
    let history_dir = history_dir_for_world(world_path);
    let path = history_dir.join(id).join(HISTORY_ENTRY_FILE);
    let raw = fs::read_to_string(&path).map_err(|error| format!("读取历史索引失败: {error}"))?;
    let mut entry = serde_json::from_str::<MapHistoryEntry>(&raw)
        .map_err(|error| format!("解析历史索引失败: {error}"))?;
    entry.status = status;
    entry.error = error;
    let encoded = serde_json::to_vec_pretty(&entry)
        .map_err(|error| format!("序列化历史索引失败: {error}"))?;
    write_atomic(path, &encoded)
}

fn read_optional_file(path: impl AsRef<Path>) -> Result<Option<Vec<u8>>, String> {
    let path = path.as_ref();
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("读取文件失败 {}: {error}", path.display())),
    }
}

fn write_optional_file(path: impl AsRef<Path>, value: Option<Vec<u8>>) -> Result<(), String> {
    let path = path.as_ref();
    match value {
        Some(bytes) => write_atomic(path, &bytes),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("删除文件失败 {}: {error}", path.display())),
        },
    }
}

fn write_atomic(path: impl AsRef<Path>, bytes: &[u8]) -> Result<(), String> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("创建目录失败 {}: {error}", parent.display()))?;
    }
    let temp = path.with_extension("tmp");
    fs::write(&temp, bytes)
        .map_err(|error| format!("写入临时文件失败 {}: {error}", temp.display()))?;
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("替换文件失败 {}: {error}", path.display())),
    }
    fs::rename(&temp, path).map_err(|error| format!("提交文件失败 {}: {error}", path.display()))
}

fn dir_size(path: &Path) -> Result<u64, String> {
    let mut total = 0u64;
    for entry in fs::read_dir(path).map_err(|error| format!("读取目录失败: {error}"))? {
        let entry = entry.map_err(|error| format!("读取目录项失败: {error}"))?;
        let metadata = entry
            .metadata()
            .map_err(|error| format!("读取文件信息失败: {error}"))?;
        if metadata.is_dir() {
            total = total.saturating_add(dir_size(&entry.path())?);
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn remove_dir_all_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("删除历史目录失败 {}: {error}", path.display())),
    }
}

fn world_history_id(world_path: &Path) -> String {
    let canonical = world_path
        .canonicalize()
        .unwrap_or_else(|_| world_path.to_path_buf());
    let mut hasher = RenderFingerprint::new();
    canonical.to_string_lossy().hash(&mut hasher);
    canonical
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.hex()
}

fn new_history_id() -> String {
    format!("{}-{}", now_secs(), uuid::Uuid::new_v4())
}

fn now_secs() -> u64 {
    UNIX_EPOCH.elapsed().map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::{
        HISTORY_OBJECT_STORE_DIR, MapHistoryCapture, MapHistoryChange, MapHistoryEntryKind,
        RawRecordDelta, build_raw_record_deltas, complete_snapshot, history_applied_change,
        now_secs, prune_history, read_history_change, world_history_id,
    };
    use bedrock_world::{ActorDigestKey, ChunkKey, ChunkPos, ChunkRecordTag, Dimension};
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::PathBuf;
    use std::time::UNIX_EPOCH;

    fn test_world_path(prefix: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("{prefix}-{nonce}"))
    }

    #[test]
    fn world_history_id_is_stable_when_level_dat_changes() {
        let world_path = test_world_path("map-history-id");
        fs::create_dir_all(&world_path).expect("create world");
        fs::write(world_path.join("level.dat"), b"before").expect("write level");
        let before = world_history_id(&world_path);
        fs::write(world_path.join("level.dat"), b"after").expect("write level again");

        assert_eq!(world_history_id(&world_path), before);
    }

    #[test]
    fn raw_record_deltas_represent_create_update_and_delete() {
        let created_key = b"created".to_vec();
        let updated_key = b"updated".to_vec();
        let deleted_key = b"deleted".to_vec();
        let unchanged_key = b"unchanged".to_vec();
        let raw_keys = [
            created_key.clone(),
            updated_key.clone(),
            deleted_key.clone(),
            unchanged_key.clone(),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        let before = [
            (updated_key.clone(), Some(b"old".to_vec())),
            (deleted_key.clone(), Some(b"gone".to_vec())),
            (unchanged_key.clone(), Some(b"same".to_vec())),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let after = [
            (created_key.clone(), Some(b"new".to_vec())),
            (updated_key.clone(), Some(b"fresh".to_vec())),
            (unchanged_key.clone(), Some(b"same".to_vec())),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        let (deltas, bytes) = build_raw_record_deltas(&before, &raw_keys, |key| {
            Ok(after.get(key).cloned().unwrap_or(None))
        })
        .expect("build deltas");

        assert!(bytes > 0);
        assert_eq!(deltas.len(), 3);
        assert!(deltas.iter().any(|delta| {
            delta.key == created_key
                && delta.before.is_none()
                && delta.after == Some(b"new".to_vec())
        }));
        assert!(deltas.iter().any(|delta| {
            delta.key == updated_key
                && delta.before == Some(b"old".to_vec())
                && delta.after == Some(b"fresh".to_vec())
        }));
        assert!(deltas.iter().any(|delta| {
            delta.key == deleted_key
                && delta.before == Some(b"gone".to_vec())
                && delta.after.is_none()
        }));
    }

    #[test]
    fn history_applied_change_decodes_raw_delta_chunks() {
        let chunk = ChunkPos {
            x: 12,
            z: -7,
            dimension: Dimension::Nether,
        };
        let digest_chunk = ChunkPos {
            x: -2,
            z: 5,
            dimension: Dimension::Overworld,
        };
        let change = MapHistoryChange {
            raw_records: vec![
                RawRecordDelta {
                    key: ChunkKey::new(chunk, ChunkRecordTag::Data2D)
                        .encode()
                        .to_vec(),
                    before: None,
                    after: Some(vec![1]),
                },
                RawRecordDelta {
                    key: ActorDigestKey::new(digest_chunk).storage_key().to_vec(),
                    before: Some(vec![2]),
                    after: Some(vec![3]),
                },
            ],
            level_dat: None,
        };

        let applied_change = history_applied_change(BTreeSet::new(), &change);

        assert_eq!(
            applied_change.affected_chunks,
            [chunk, digest_chunk].into_iter().collect()
        );
        assert!(!applied_change.refresh_all_tiles);
        assert!(!applied_change.level_dat_changed);
    }

    #[test]
    fn history_applied_change_refreshes_all_tiles_for_unmapped_raw_delta() {
        let change = MapHistoryChange {
            raw_records: vec![RawRecordDelta {
                key: b"~local_player".to_vec(),
                before: Some(vec![1]),
                after: Some(vec![2]),
            }],
            level_dat: None,
        };

        let applied_change = history_applied_change(BTreeSet::new(), &change);

        assert!(applied_change.affected_chunks.is_empty());
        assert!(applied_change.refresh_all_tiles);
    }

    #[test]
    fn restore_protection_point_writes_snapshot_delta() {
        let history_dir = test_world_path("map-history-snapshot");
        let capture = MapHistoryCapture {
            id: "snapshot".to_string(),
            timestamp_secs: now_secs(),
            kind: MapHistoryEntryKind::RestorePoint,
            label: "protect".to_string(),
            world_path: test_world_path("map-history-snapshot-world"),
            history_dir: history_dir.clone(),
            chunks: BTreeSet::new(),
            raw_keys: [b"k".to_vec()].into_iter().collect(),
            before_raw_records: [(b"k".to_vec(), Some(b"v".to_vec()))].into_iter().collect(),
            before_level_dat: Some(Some(b"level".to_vec())),
        };

        let entry = complete_snapshot(capture, "protect").expect("write snapshot");
        let change = read_history_change(&history_dir, &entry.id).expect("read change");

        assert_eq!(change.raw_records[0].before, Some(b"v".to_vec()));
        assert_eq!(change.raw_records[0].after, Some(b"v".to_vec()));
        assert!(change.level_dat.is_some());
    }

    #[test]
    fn repeated_large_history_values_reuse_object_store_blob() {
        let history_dir = test_world_path("map-history-objects");
        let large_value = vec![7u8; 4096];

        let first_capture = MapHistoryCapture {
            id: "first".to_string(),
            timestamp_secs: now_secs(),
            kind: MapHistoryEntryKind::RestorePoint,
            label: "first".to_string(),
            world_path: test_world_path("map-history-object-world-1"),
            history_dir: history_dir.clone(),
            chunks: BTreeSet::new(),
            raw_keys: [b"k".to_vec()].into_iter().collect(),
            before_raw_records: [(b"k".to_vec(), Some(large_value.clone()))]
                .into_iter()
                .collect(),
            before_level_dat: None,
        };
        let second_capture = MapHistoryCapture {
            id: "second".to_string(),
            timestamp_secs: now_secs(),
            kind: MapHistoryEntryKind::RestorePoint,
            label: "second".to_string(),
            world_path: test_world_path("map-history-object-world-2"),
            history_dir: history_dir.clone(),
            chunks: BTreeSet::new(),
            raw_keys: [b"k".to_vec()].into_iter().collect(),
            before_raw_records: [(b"k".to_vec(), Some(large_value.clone()))]
                .into_iter()
                .collect(),
            before_level_dat: None,
        };

        let first = complete_snapshot(first_capture, "first").expect("write first");
        let second = complete_snapshot(second_capture, "second").expect("write second");
        let first_change = read_history_change(&history_dir, &first.id).expect("read first");
        let second_change = read_history_change(&history_dir, &second.id).expect("read second");

        assert_eq!(first.stored_object_count, 1);
        assert!(second.reused_object_count >= 1);
        assert_eq!(
            first_change.raw_records[0].before,
            Some(large_value.clone())
        );
        assert_eq!(second_change.raw_records[0].after, Some(large_value));
    }

    #[test]
    fn prune_history_keeps_referenced_shared_objects() {
        let history_dir = test_world_path("map-history-prune-objects");
        let large_value = vec![3u8; 4096];
        let capture = MapHistoryCapture {
            id: "kept".to_string(),
            timestamp_secs: now_secs(),
            kind: MapHistoryEntryKind::RestorePoint,
            label: "kept".to_string(),
            world_path: test_world_path("map-history-prune-world"),
            history_dir: history_dir.clone(),
            chunks: BTreeSet::new(),
            raw_keys: [b"k".to_vec()].into_iter().collect(),
            before_raw_records: [(b"k".to_vec(), Some(large_value))].into_iter().collect(),
            before_level_dat: None,
        };
        let entry = complete_snapshot(capture, "kept").expect("write snapshot");

        prune_history(&history_dir, 60, u64::MAX).expect("prune history");
        let change = read_history_change(&history_dir, &entry.id).expect("read kept");

        assert_eq!(change.raw_records.len(), 1);
        assert!(history_dir.join(HISTORY_OBJECT_STORE_DIR).exists());
    }
}
