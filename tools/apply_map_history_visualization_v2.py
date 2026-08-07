from pathlib import Path

def read(path: str) -> str:
    return Path(path).read_text(encoding="utf-8")

def write(path: str, text: str) -> None:
    Path(path).write_text(text, encoding="utf-8")

def replace_once(path: str, old: str, new: str) -> None:
    text = read(path)
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"expected one match in {path}, found {count}: {old[:160]!r}")
    write(path, text.replace(old, new, 1))

def replace_count(path: str, old: str, new: str, expected: int) -> None:
    text = read(path)
    count = text.count(old)
    if count != expected:
        raise SystemExit(f"expected {expected} matches in {path}, found {count}: {old[:160]!r}")
    write(path, text.replace(old, new))

def replace_between(path: str, start_marker: str, end_marker: str, new_block: str) -> None:
    text = read(path)
    start_count = text.count(start_marker)
    if start_count != 1:
        raise SystemExit(f"expected one start marker in {path}, found {start_count}: {start_marker!r}")
    start = text.index(start_marker)
    end = text.index(end_marker, start)
    write(path, text[:start] + new_block + text[end:])

# History data model.
history = "src/ui/window/map_viewer/map_history.rs"

history_types = r'''const HISTORY_VISUAL_COLUMN_SIDE: usize = 16;
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
        self.added_blocks as u32
            + self.removed_blocks as u32
            + self.modified_blocks as u32
    }

    pub(super) const fn filtered_counts(
        self,
        filter: MapHistoryVisualFilter,
    ) -> (u32, u32, u32) {
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
            MapHistoryVisualFilterKind::Added => {
                self.added_blocks > 0 || self.added_records > 0
            }
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

'''
replace_between(
    history,
    "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub(super) enum MapHistoryChunkVisualKind",
    "impl Default for MapHistoryState {",
    history_types,
)

replace_once(
    history,
    '''            visualization: Arc::new(Vec::new()),
            visualization_loading: false,
            visualization_enabled: true,
            visualization_error: None,
''',
    '''            visualization: Arc::new(MapHistoryVisualization::default()),
            visualization_loading: false,
            visualization_enabled: true,
            visualization_filter: MapHistoryVisualFilter::default(),
            visualization_error: None,
''',
)

history_loader = r'''#[derive(Clone, Copy, Debug)]
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

    fn add_block(
        &mut self,
        local_x: u8,
        local_z: u8,
        kind: MapHistoryVisualFilterKind,
    ) {
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
                visualization.changed_subchunks =
                    visualization.changed_subchunks.saturating_add(1);
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
            visualization.record_only_chunks =
                visualization.record_only_chunks.saturating_add(1);
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
        Bytes::copy_from_slice(bytes),
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
        && history_subchunk_blocks_equal(before, after, local_x, local_y, local_z)
            == Some(true)
    {
        return None;
    }
    Some(MapHistoryVisualFilterKind::Modified)
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
                if !bedrock_world::surface::is_air_block_name(&state.name) {
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
            for (before_storage, after_storage) in
                before_storages.iter().zip(after_storages.iter())
            {
                let before_state =
                    before_storage.block_state_at(local_x, local_y, local_z)?;
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
            let after_air =
                history_subchunk_block_is_air(Some(after), local_x, local_y, local_z)?;
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
    match bedrock_world::LegacyTerrain::parse(Bytes::copy_from_slice(bytes)) {
        Ok(terrain) => (Some(terrain), true),
        Err(_) => (None, false),
    }
}

'''
replace_between(
    history,
    "#[derive(Clone, Copy, Debug, Default)]\nstruct HistoryChunkDeltaStats",
    "pub(super) fn apply_undo_with_progress",
    history_loader,
)

# Prelude exports.
prelude = "src/ui/window/map_viewer/prelude.rs"
replace_once(
    prelude,
    '''    MapHistoryApplyOutcome, MapHistoryApplyProgress, MapHistoryCaptureSpec, MapHistoryChunkVisualKind,
    MapHistoryEntry, MapHistoryEntryKind, MapHistoryEntryStatus, MapHistoryState,
''',
    '''    MapHistoryApplyOutcome, MapHistoryApplyProgress, MapHistoryCaptureSpec,
    MapHistoryChunkVisualKind, MapHistoryEntry, MapHistoryEntryKind, MapHistoryEntryStatus,
    MapHistoryState, MapHistoryVisualFilter, MapHistoryVisualFilterKind, MapHistoryVisualization,
''',
)

# Canvas snapshots and adaptive rendering.
canvas = "src/ui/window/map_viewer/canvas.rs"
replace_once(
    canvas,
    "use super::map_history::{MapHistoryChunkVisual, MapHistoryChunkVisualKind};",
    "use super::map_history::{\n    MapHistoryChunkVisual, MapHistoryChunkVisualKind, MapHistoryVisualFilter,\n    MapHistoryVisualization,\n};",
)
replace_once(
    canvas,
    "const MAP_TILE_IDLE_NEW_IMAGE_BUDGET_PER_FRAME: usize = 8;",
    "const MAP_TILE_IDLE_NEW_IMAGE_BUDGET_PER_FRAME: usize = 8;\nconst HISTORY_VISUAL_COLUMN_SIDE: usize = 16;",
)
replace_once(
    canvas,
    '''    pub(super) history_visualization: Arc<Vec<MapHistoryChunkVisual>>,
    pub(super) history_visualization_enabled: bool,
''',
    '''    pub(super) history_visualization: Arc<MapHistoryVisualization>,
    pub(super) history_visualization_enabled: bool,
    pub(super) history_visualization_filter: MapHistoryVisualFilter,
''',
)
replace_once(
    canvas,
    '''    history_visualization: Arc<Vec<MapHistoryChunkVisual>>,
    history_visualization_enabled: bool,
    history_visualization_ptr: usize,
''',
    '''    history_visualization: Arc<MapHistoryVisualization>,
    history_visualization_enabled: bool,
    history_visualization_filter: MapHistoryVisualFilter,
    history_visualization_ptr: usize,
''',
)
replace_once(
    canvas,
    '''            history_visualization: snapshot.history_visualization.clone(),
            history_visualization_enabled: snapshot.history_visualization_enabled,
            history_visualization_ptr: Arc::as_ptr(&snapshot.history_visualization) as usize,
''',
    '''            history_visualization: snapshot.history_visualization.clone(),
            history_visualization_enabled: snapshot.history_visualization_enabled,
            history_visualization_filter: snapshot.history_visualization_filter,
            history_visualization_ptr: Arc::as_ptr(&snapshot.history_visualization) as usize,
''',
)
replace_once(
    canvas,
    '''            && self.history_visualization_enabled == other.history_visualization_enabled
            && self.history_visualization_ptr == other.history_visualization_ptr
''',
    '''            && self.history_visualization_enabled == other.history_visualization_enabled
            && self.history_visualization_filter == other.history_visualization_filter
            && self.history_visualization_ptr == other.history_visualization_ptr
''',
)
replace_once(
    canvas,
    '''    let history_visualization = snapshot.history_visualization.clone();
    let history_visualization_enabled = snapshot.history_visualization_enabled;
    let colors = snapshot.colors;
''',
    '''    let history_visualization = snapshot.history_visualization.clone();
    let history_visualization_enabled = snapshot.history_visualization_enabled;
    let history_visualization_filter = snapshot.history_visualization_filter;
    let colors = snapshot.colors;
''',
)
replace_once(
    canvas,
    '''                        &history_visualization,
                        window,
''',
    '''                        &history_visualization,
                        history_visualization_filter,
                        window,
''',
)

canvas_render = r'''fn draw_history_visualization_overlay(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    dimension: Dimension,
    visualization: &MapHistoryVisualization,
    filter: MapHistoryVisualFilter,
    window: &mut Window,
) {
    if !filter.any_enabled() {
        return;
    }

    draw_history_operation_envelope(
        bounds,
        viewport,
        layout,
        dimension,
        &visualization.chunks,
        filter,
        window,
    );

    let canvas_left = bounds.left() / px(1.0);
    let canvas_top = bounds.top() / px(1.0);
    let canvas_right = bounds.right() / px(1.0);
    let canvas_bottom = bounds.bottom() / px(1.0);
    for item in visualization
        .chunks
        .iter()
        .filter(|item| item.pos.dimension == dimension)
        .filter(|item| item.filtered_total(filter) > 0)
        .take(4096)
    {
        let left = screen_x_for_block(
            bounds,
            viewport,
            layout,
            item.pos.x.saturating_mul(16),
        );
        let top = screen_y_for_block(
            bounds,
            viewport,
            layout,
            item.pos.z.saturating_mul(16),
        );
        let right = screen_x_for_block(
            bounds,
            viewport,
            layout,
            item.pos.x.saturating_add(1).saturating_mul(16),
        );
        let bottom = screen_y_for_block(
            bounds,
            viewport,
            layout,
            item.pos.z.saturating_add(1).saturating_mul(16),
        );
        if right <= left
            || bottom <= top
            || right < canvas_left
            || bottom < canvas_top
            || left > canvas_right
            || top > canvas_bottom
        {
            continue;
        }

        let rect = Bounds::new(
            point(px(left), px(top)),
            size(px(right - left), px(bottom - top)),
        );
        let chunk_pixels = (right - left).abs();
        let Some(kind) = item.filtered_kind(filter) else {
            continue;
        };
        let outline_color = history_visual_kind_color(kind);
        let intensity = history_visual_intensity(item.filtered_total(filter));

        if item.precise_block_diff() && item.filtered_block_counts(filter) != (0, 0, 0) {
            let grid_side = if chunk_pixels >= 64.0 {
                16
            } else if chunk_pixels >= 12.0 {
                4
            } else {
                0
            };
            if grid_side > 0 {
                paint_history_block_grid(rect, item, grid_side, filter, window);
            } else {
                window.paint_quad(fill(rect, outline_color.alpha(0.06 + intensity * 0.08)));
            }
        } else {
            paint_history_record_summary(rect, item, filter, intensity, window);
        }

        let border_alpha = 0.42 + intensity * 0.34;
        let border_width = if chunk_pixels >= 28.0 {
            px(1.5)
        } else {
            px(1.0)
        };
        paint_history_outline(
            rect,
            outline_color.alpha(border_alpha.min(0.82)),
            border_width,
            window,
        );

        if item.unresolved_terrain_records > 0 && chunk_pixels >= 14.0 {
            paint_history_uncertainty_corner(rect, window);
        }
    }
}

fn draw_history_operation_envelope(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    dimension: Dimension,
    visualization: &[MapHistoryChunkVisual],
    filter: MapHistoryVisualFilter,
    window: &mut Window,
) {
    let mut min_x = i32::MAX;
    let mut min_z = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_z = i32::MIN;
    let mut count = 0usize;
    for item in visualization
        .iter()
        .filter(|item| item.pos.dimension == dimension)
        .filter(|item| item.filtered_total(filter) > 0)
    {
        min_x = min_x.min(item.pos.x);
        min_z = min_z.min(item.pos.z);
        max_x = max_x.max(item.pos.x);
        max_z = max_z.max(item.pos.z);
        count = count.saturating_add(1);
    }
    if count < 2 {
        return;
    }
    let width = i64::from(max_x)
        .saturating_sub(i64::from(min_x))
        .saturating_add(1);
    let height = i64::from(max_z)
        .saturating_sub(i64::from(min_z))
        .saturating_add(1);
    let area = width.saturating_mul(height);
    if area <= 0 || area > 65_536 {
        return;
    }
    let density = count as f32 / area as f32;
    if density < 0.32 {
        return;
    }

    let left = screen_x_for_block(bounds, viewport, layout, min_x.saturating_mul(16));
    let top = screen_y_for_block(bounds, viewport, layout, min_z.saturating_mul(16));
    let right = screen_x_for_block(
        bounds,
        viewport,
        layout,
        max_x.saturating_add(1).saturating_mul(16),
    );
    let bottom = screen_y_for_block(
        bounds,
        viewport,
        layout,
        max_z.saturating_add(1).saturating_mul(16),
    );
    if right <= left || bottom <= top {
        return;
    }
    let rect = Bounds::new(
        point(px(left), px(top)),
        size(px(right - left), px(bottom - top)),
    );
    paint_history_outline(rect, rgb(0xf59e0b).alpha(0.72), px(2.0), window);
}

fn paint_history_block_grid(
    rect: Bounds<Pixels>,
    item: &MapHistoryChunkVisual,
    grid_side: usize,
    filter: MapHistoryVisualFilter,
    window: &mut Window,
) {
    let cell_width = rect.size.width / grid_side as f32;
    let cell_height = rect.size.height / grid_side as f32;
    let mut max_total = 0u32;
    for cell_z in 0..grid_side {
        for cell_x in 0..grid_side {
            let counts = history_grid_cell_counts(item, grid_side, cell_x, cell_z, filter);
            max_total = max_total.max(
                counts
                    .0
                    .saturating_add(counts.1)
                    .saturating_add(counts.2),
            );
        }
    }
    if max_total == 0 {
        return;
    }

    for cell_z in 0..grid_side {
        for cell_x in 0..grid_side {
            let counts = history_grid_cell_counts(item, grid_side, cell_x, cell_z, filter);
            if counts == (0, 0, 0) {
                continue;
            }
            let cell = Bounds::new(
                point(
                    rect.origin.x + cell_width * cell_x as f32,
                    rect.origin.y + cell_height * cell_z as f32,
                ),
                size(cell_width, cell_height),
            );
            paint_history_change_segments(cell, counts, max_total, window);
        }
    }
}

fn history_grid_cell_counts(
    item: &MapHistoryChunkVisual,
    grid_side: usize,
    cell_x: usize,
    cell_z: usize,
    filter: MapHistoryVisualFilter,
) -> (u32, u32, u32) {
    let span = HISTORY_VISUAL_COLUMN_SIDE / grid_side;
    let mut added = 0u32;
    let mut removed = 0u32;
    let mut modified = 0u32;
    for local_z in cell_z * span..(cell_z + 1) * span {
        for local_x in cell_x * span..(cell_x + 1) * span {
            let index = local_z * HISTORY_VISUAL_COLUMN_SIDE + local_x;
            let Some(column) = item.columns.get(index) else {
                continue;
            };
            let counts = column.filtered_counts(filter);
            added = added.saturating_add(counts.0);
            removed = removed.saturating_add(counts.1);
            modified = modified.saturating_add(counts.2);
        }
    }
    (added, removed, modified)
}

fn paint_history_record_summary(
    rect: Bounds<Pixels>,
    item: &MapHistoryChunkVisual,
    filter: MapHistoryVisualFilter,
    intensity: f32,
    window: &mut Window,
) {
    let Some(kind) = item.filtered_kind(filter) else {
        return;
    };
    window.paint_quad(fill(
        rect,
        history_visual_kind_color(kind).alpha(0.06 + intensity * 0.10),
    ));
    let records = item.filtered_record_counts(filter);
    let record_counts = (
        u32::try_from(records.0).unwrap_or(u32::MAX),
        u32::try_from(records.1).unwrap_or(u32::MAX),
        u32::try_from(records.2).unwrap_or(u32::MAX),
    );
    let total = record_counts
        .0
        .saturating_add(record_counts.1)
        .saturating_add(record_counts.2);
    if total == 0 {
        return;
    }
    let bar_height = if rect.size.height / px(1.0) >= 10.0 {
        px(2.0)
    } else {
        px(1.0)
    };
    paint_history_change_segments(
        Bounds::new(rect.origin, size(rect.size.width, bar_height)),
        record_counts,
        total,
        window,
    );
}

fn paint_history_change_segments(
    rect: Bounds<Pixels>,
    counts: (u32, u32, u32),
    max_total: u32,
    window: &mut Window,
) {
    let total = counts.0.saturating_add(counts.1).saturating_add(counts.2);
    if total == 0 {
        return;
    }
    let density = (total as f32 / max_total.max(1) as f32).sqrt();
    let alpha = (0.12 + density * 0.28).min(0.40);
    let segments = [
        (counts.0, rgb(0x3b82f6)),
        (counts.1, rgb(0xef4444)),
        (counts.2, rgb(0x8b5cf6)),
    ];
    let mut cursor = rect.origin.x;
    let mut remaining = rect.size.width;
    let mut non_empty = segments.iter().filter(|(count, _)| *count > 0).count();
    for (count, color) in segments {
        if count == 0 {
            continue;
        }
        non_empty = non_empty.saturating_sub(1);
        let width = if non_empty == 0 {
            remaining
        } else {
            rect.size.width * (count as f32 / total as f32)
        };
        if width > px(0.0) {
            window.paint_quad(fill(
                Bounds::new(point(cursor, rect.origin.y), size(width, rect.size.height)),
                color.alpha(alpha),
            ));
            cursor += width;
            remaining -= width;
        }
    }
}

fn paint_history_uncertainty_corner(rect: Bounds<Pixels>, window: &mut Window) {
    let side = px((rect.size.width / px(1.0)).min(5.0).max(2.0));
    window.paint_quad(fill(
        Bounds::new(point(rect.right() - side, rect.origin.y), size(side, side)),
        rgb(0xf59e0b).alpha(0.82),
    ));
}

fn history_visual_intensity(total: u64) -> f32 {
    ((total as f32 + 1.0).ln() / 8.0).clamp(0.18, 1.0)
}

fn history_visual_kind_color(kind: MapHistoryChunkVisualKind) -> Rgba {
    match kind {
        MapHistoryChunkVisualKind::Added => rgb(0x3b82f6),
        MapHistoryChunkVisualKind::Removed => rgb(0xef4444),
        MapHistoryChunkVisualKind::Modified => rgb(0x8b5cf6),
        MapHistoryChunkVisualKind::Mixed => rgb(0xf59e0b),
    }
}

fn paint_history_outline(
    rect: Bounds<Pixels>,
    color: Rgba,
    thickness: Pixels,
    window: &mut Window,
) {
    window.paint_quad(fill(
        Bounds::new(rect.origin, size(rect.size.width, thickness)),
        color,
    ));
    window.paint_quad(fill(
        Bounds::new(
            point(rect.origin.x, rect.bottom() - thickness),
            size(rect.size.width, thickness),
        ),
        color,
    ));
    window.paint_quad(fill(
        Bounds::new(rect.origin, size(thickness, rect.size.height)),
        color,
    ));
    window.paint_quad(fill(
        Bounds::new(
            point(rect.right() - thickness, rect.origin.y),
            size(thickness, rect.size.height),
        ),
        color,
    ));
}

'''
replace_between(
    canvas,
    "fn draw_history_visualization_overlay(",
    "fn render_markers(",
    canvas_render,
)

# Lifecycle snapshot publication.
lifecycle = "src/ui/window/map_viewer/lifecycle.rs"
replace_once(
    lifecycle,
    '''            history_visualization: self.history.visualization.clone(),
            history_visualization_enabled: self.history.visualization_enabled,
''',
    '''            history_visualization: self.history.visualization.clone(),
            history_visualization_enabled: self.history.visualization_enabled,
            history_visualization_filter: self.history.visualization_filter,
''',
)

# History panel controls and details.
panel = "src/ui/window/map_viewer/history_panel.rs"
replace_count(
    panel,
    "self.history.visualization = Arc::new(Vec::new());",
    "self.history.visualization = Arc::new(MapHistoryVisualization::default());",
    1,
)
replace_count(
    panel,
    "this.history.visualization = Arc::new(Vec::new());",
    "this.history.visualization = Arc::new(MapHistoryVisualization::default());",
    2,
)
replace_once(
    panel,
    '''    fn toggle_history_visualization(&mut self, cx: &mut Context<Self>) {
        self.history.visualization_enabled = !self.history.visualization_enabled;
        self.professional.overlay_generation =
            self.professional.overlay_generation.saturating_add(1);
        self.last_synced_canvas_snapshot_key = None;
        if self.history.visualization_enabled {
            self.load_selected_history_visualization(cx);
        }
        cx.notify();
    }
''',
    '''    fn toggle_history_visualization(&mut self, cx: &mut Context<Self>) {
        self.history.visualization_enabled = !self.history.visualization_enabled;
        self.professional.overlay_generation =
            self.professional.overlay_generation.saturating_add(1);
        self.last_synced_canvas_snapshot_key = None;
        if self.history.visualization_enabled {
            self.load_selected_history_visualization(cx);
        }
        cx.notify();
    }

    fn toggle_history_visualization_filter(
        &mut self,
        kind: MapHistoryVisualFilterKind,
        cx: &mut Context<Self>,
    ) {
        self.history.visualization_filter.toggle(kind);
        self.professional.overlay_generation =
            self.professional.overlay_generation.saturating_add(1);
        self.last_synced_canvas_snapshot_key = None;
        cx.notify();
    }
''',
)
replace_once(
    panel,
    ".child(history_visualization_legend(colors, &self.history))\n                    .child(history_detail_text(selected, self.history.error.as_ref())),",
    ".child(history_visualization_legend(colors, &self.history, cx))\n                    .child(history_detail_text(\n                        selected,\n                        self.history.error.as_ref(),\n                        &self.history,\n                    )),",
)

panel_helpers = r'''fn history_visualization_legend(
    colors: &ThemeColors,
    history: &MapHistoryState,
    cx: &mut Context<MapViewerWindowView>,
) -> Div {
    let visualization = history.visualization.as_ref();
    div()
        .mb(px(10.0))
        .p(px(9.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.32,
            ..colors.surface
        })
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .child(history_filter_chip(
            "新增",
            MapHistoryVisualFilterKind::Added,
            visualization,
            history.visualization_filter,
            rgb(0x3b82f6),
            colors,
            cx,
        ))
        .child(history_filter_chip(
            "删除",
            MapHistoryVisualFilterKind::Removed,
            visualization,
            history.visualization_filter,
            rgb(0xef4444),
            colors,
            cx,
        ))
        .child(history_filter_chip(
            "修改",
            MapHistoryVisualFilterKind::Modified,
            visualization,
            history.visualization_filter,
            rgb(0x8b5cf6),
            colors,
            cx,
        ))
        .child(history_summary_badge(
            format!("混合 {} chunk", visualization.mixed_chunks),
            rgb(0xf59e0b),
            colors,
        ))
        .child(history_summary_badge(
            format!(
                "精确 {} · 部分 {} · 记录级 {}",
                visualization.precise_chunks,
                visualization.partial_chunks,
                visualization.record_only_chunks
            ),
            rgb(0x10b981),
            colors,
        ))
        .when(history.visualization_loading, |this| {
            this.child(
                div()
                    .text_color(colors.text_muted)
                    .child("正在解析块级差异..."),
            )
        })
        .when(!history.visualization_enabled, |this| {
            this.child(div().text_color(colors.text_muted).child("地图差异已隐藏"))
        })
        .when(!history.visualization_filter.any_enabled(), |this| {
            this.child(div().text_color(colors.text_muted).child("所有差异类型均已过滤"))
        })
        .when_some(history.visualization_error.clone(), |this, error| {
            this.child(div().text_color(colors.danger).child(error))
        })
}

fn history_filter_chip(
    label: &'static str,
    kind: MapHistoryVisualFilterKind,
    visualization: &MapHistoryVisualization,
    filter: MapHistoryVisualFilter,
    color: Rgba,
    colors: &ThemeColors,
    cx: &mut Context<MapViewerWindowView>,
) -> Div {
    let active = filter.includes(kind);
    let blocks = visualization.kind_blocks(kind);
    let records = visualization.kind_records(kind);
    let chunks = visualization
        .chunks
        .iter()
        .filter(|chunk| chunk.has_kind(kind))
        .count();
    let metric = if blocks > 0 {
        format!("{} block", format_history_count(blocks))
    } else {
        format!("{records} record")
    };
    div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .px(px(7.0))
        .py(px(4.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(if active {
            color.alpha(0.48)
        } else {
            Hsla {
                a: 0.14,
                ..colors.border
            }
            .into()
        })
        .bg(if active {
            color.alpha(0.12)
        } else {
            Hsla {
                a: 0.16,
                ..colors.surface_hover
            }
            .into()
        })
        .cursor(CursorStyle::PointingHand)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _event, _window, cx| {
                this.toggle_history_visualization_filter(kind, cx);
            }),
        )
        .child(
            div()
                .w(px(9.0))
                .h(px(9.0))
                .rounded(px(2.0))
                .bg(color.alpha(if active { 0.68 } else { 0.20 })),
        )
        .child(
            div()
                .text_color(if active {
                    colors.text_secondary
                } else {
                    colors.text_muted
                })
                .child(format!("{label} {metric} · {chunks} chunk")),
        )
}

fn history_summary_badge(label: String, color: Rgba, colors: &ThemeColors) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .px(px(7.0))
        .py(px(4.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.16,
            ..colors.surface_hover
        })
        .child(
            div()
                .w(px(8.0))
                .h(px(8.0))
                .rounded_full()
                .bg(color.alpha(0.58)),
        )
        .child(div().text_color(colors.text_muted).child(label))
}

'''
replace_between(
    panel,
    "fn history_visualization_legend(",
    "fn history_detail_text(",
    panel_helpers,
)

detail_helpers = r'''fn history_detail_text(
    entry: Option<&MapHistoryEntry>,
    error: Option<&SharedString>,
    history: &MapHistoryState,
) -> SharedString {
    if let Some(error) = error {
        return SharedString::from(format!("历史加载错误\n\n{error}"));
    }
    let Some(entry) = entry else {
        return SharedString::from("选择左侧历史项查看详情。");
    };
    let visualization = history.visualization.as_ref();
    let mut lines = Vec::new();
    lines.push(format!("变更集 ID: {}", entry.id));
    lines.push(format!("类型: {}", entry.kind_label()));
    lines.push(format!("状态: {}", entry.short_status()));
    lines.push(format!(
        "时间: {}",
        format_history_time(entry.timestamp_secs)
    ));
    lines.push(format!("标题: {}", entry.label));
    lines.push(format!("说明: {}", entry.message));
    lines.push(String::new());

    lines.push("空间范围".to_string());
    lines.extend(history_dimension_bounds_text(&visualization.chunks));
    lines.push(format!(
        "影响 chunk: {}（混合 {}）",
        visualization.chunks.len(),
        visualization.mixed_chunks
    ));
    lines.push(String::new());

    lines.push("块级差异".to_string());
    lines.push(format!(
        "新增 {} · 删除 {} · 修改 {} · 总计 {} block",
        format_history_count(visualization.added_blocks),
        format_history_count(visualization.removed_blocks),
        format_history_count(visualization.modified_blocks),
        format_history_count(visualization.total_blocks()),
    ));
    lines.push(format!(
        "精确 chunk {} · 部分解析 {} · 仅记录级 {}",
        visualization.precise_chunks,
        visualization.partial_chunks,
        visualization.record_only_chunks
    ));
    lines.push(format!("变化子区块: {}", visualization.changed_subchunks));
    lines.push(String::new());

    lines.push("数据库记录".to_string());
    lines.push(format!(
        "新增 {} · 删除 {} · 修改 {} · 总计 {} record",
        visualization.added_records,
        visualization.removed_records,
        visualization.modified_records,
        visualization.total_records(),
    ));
    lines.push(format!(
        "地形 {} · 方块实体 {} · 实体 {} · 元数据 {} · 未映射 {}",
        visualization.terrain_records,
        visualization.block_entity_records,
        visualization.entity_records,
        visualization.metadata_records,
        visualization.unmapped_records,
    ));
    lines.push(format!(
        "level.dat: {}",
        if visualization.level_dat_changed {
            "有变化"
        } else {
            "无变化"
        }
    ));
    lines.push(String::new());

    lines.push("存储".to_string());
    lines.push(format!("世界: {}", entry.world_path));
    lines.push(format!(
        "原始变化字节: {}",
        format_history_count(entry.raw_delta_bytes)
    ));
    lines.push(format!("存储格式: {}", history_storage_label(entry)));
    lines.push(format!(
        "实际新增存储: {} bytes{}",
        format_history_count(entry.stored_bytes),
        history_compression_ratio(entry)
    ));
    if entry.stored_object_count > 0 || entry.reused_object_count > 0 {
        lines.push(format!(
            "对象库: 新增 {} · 复用 {}",
            entry.stored_object_count, entry.reused_object_count
        ));
    }
    lines.push(format!(
        "当前地图筛选: {}",
        history_filter_text(history.visualization_filter)
    ));
    if let Some(error) = &entry.error {
        lines.push(format!("错误: {error}"));
    }
    SharedString::from(lines.join("\n"))
}

fn history_dimension_bounds_text(chunks: &[MapHistoryChunkVisual]) -> Vec<String> {
    let mut bounds = BTreeMap::<Dimension, (i32, i32, i32, i32, usize)>::new();
    for chunk in chunks {
        let entry = bounds
            .entry(chunk.pos.dimension)
            .or_insert((chunk.pos.x, chunk.pos.z, chunk.pos.x, chunk.pos.z, 0));
        entry.0 = entry.0.min(chunk.pos.x);
        entry.1 = entry.1.min(chunk.pos.z);
        entry.2 = entry.2.max(chunk.pos.x);
        entry.3 = entry.3.max(chunk.pos.z);
        entry.4 = entry.4.saturating_add(1);
    }
    if bounds.is_empty() {
        return vec!["无可映射的 chunk 范围".to_string()];
    }
    bounds
        .into_iter()
        .map(|(dimension, (min_x, min_z, max_x, max_z, count))| {
            format!(
                "{}: chunk ({min_x},{min_z}) → ({max_x},{max_z}) · block X {}..{} · Z {}..{} · {count} chunk",
                history_dimension_label(dimension),
                min_x.saturating_mul(16),
                max_x.saturating_add(1).saturating_mul(16).saturating_sub(1),
                min_z.saturating_mul(16),
                max_z.saturating_add(1).saturating_mul(16).saturating_sub(1),
            )
        })
        .collect()
}

fn history_dimension_label(dimension: Dimension) -> String {
    match dimension {
        Dimension::Overworld => "主世界".to_string(),
        Dimension::Nether => "下界".to_string(),
        Dimension::End => "末地".to_string(),
        Dimension::Unknown(id) => format!("维度 {id}"),
    }
}

fn history_filter_text(filter: MapHistoryVisualFilter) -> &'static str {
    match (
        filter.show_added,
        filter.show_removed,
        filter.show_modified,
    ) {
        (true, true, true) => "新增、删除、修改",
        (true, true, false) => "新增、删除",
        (true, false, true) => "新增、修改",
        (false, true, true) => "删除、修改",
        (true, false, false) => "仅新增",
        (false, true, false) => "仅删除",
        (false, false, true) => "仅修改",
        (false, false, false) => "全部隐藏",
    }
}

fn history_compression_ratio(entry: &MapHistoryEntry) -> String {
    if entry.raw_delta_bytes == 0 {
        return String::new();
    }
    let ratio = entry.stored_bytes as f64 / entry.raw_delta_bytes as f64 * 100.0;
    format!("（{ratio:.1}%）")
}

fn format_history_count(value: u64) -> String {
    let raw = value.to_string();
    let mut output = String::with_capacity(raw.len() + raw.len() / 3);
    for (index, ch) in raw.chars().enumerate() {
        if index > 0 && (raw.len() - index) % 3 == 0 {
            output.push(',');
        }
        output.push(ch);
    }
    output
}

'''
replace_between(
    panel,
    "fn history_detail_text(",
    "fn history_storage_label(",
    detail_helpers,
)

guards = {
    history: [
        "pub(super) struct MapHistoryVisualization",
        "columns: [MapHistoryColumnVisual; HISTORY_VISUAL_COLUMN_COUNT]",
        "accumulate_subchunk_delta",
        "MapHistoryChunkVisualKind::Mixed",
    ],
    canvas: [
        "paint_history_block_grid",
        "draw_history_operation_envelope",
        "history_visualization_filter",
    ],
    panel: [
        "toggle_history_visualization_filter",
        "精确 chunk",
        "history_filter_chip",
    ],
    lifecycle: ["history_visualization_filter: self.history.visualization_filter"],
}
for path, needles in guards.items():
    text = read(path)
    for needle in needles:
        if needle not in text:
            raise SystemExit(f"missing guard {needle!r} in {path}")
