from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, got {count}")
    return text.replace(old, new, 1)


def replace_between(text: str, start: str, end: str, replacement: str, label: str) -> str:
    start_pos = text.find(start)
    if start_pos < 0:
        raise RuntimeError(f"{label}: start marker missing")
    end_pos = text.find(end, start_pos)
    if end_pos < 0:
        raise RuntimeError(f"{label}: end marker missing")
    return text[:start_pos] + replacement + text[end_pos:]


# ---------------------------------------------------------------------------
# Player inventory layout: keep all 9-slot rows in the same full-width flex
# context and let slot size shrink on narrow center stages instead of overflowing.
# ---------------------------------------------------------------------------
path = ROOT / "src/ui/window/map_viewer/player_workspace.rs"
text = path.read_text(encoding="utf-8")
old_metrics = '''    fn player_workspace_metrics(&self) -> PlayerWorkspaceMetrics {
        // Do not reuse viewport.width here: opening/closing the right dock can leave it one
        // layout tick behind the actual center workspace. Compute against the current dock
        // geometry directly so backpack rows and hotbar share the same pixel grid immediately.
        let available = (self
            .center_stage_size(size(px(self.window_width), px(self.window_height)))
            .width
            / px(1.0))
        .max(320.0);
        let compact = available < 620.0;
        let outer_padding = if available < 470.0 {
            8.0
        } else if compact {
            12.0
        } else {
            18.0
        };
        let panel_padding = if available < 470.0 {
            9.0
        } else if compact {
            12.0
        } else {
            18.0
        };
        let slot_gap = if compact { 3.0 } else { 4.0 };
        let usable = (available - outer_padding * 2.0 - panel_padding * 2.0)
            .min(584.0)
            .max(288.0);
        let slot_size = ((usable - slot_gap * 8.0) / 9.0).floor().clamp(30.0, 52.0);
        let grid_width = (slot_size * 9.0 + slot_gap * 8.0).round();
        PlayerWorkspaceMetrics {
            slot_size,
            slot_gap,
            grid_width,
            panel_padding,
            outer_padding,
            compact,
        }
    }
'''
new_metrics = '''    fn player_workspace_metrics(&self) -> PlayerWorkspaceMetrics {
        // The center stage is the actual flex container available to the inventory. Do not
        // clamp it upward: doing so made narrow windows calculate a grid wider than their
        // parent and pushed the hotbar out of the card.
        let available = (self
            .center_stage_size(size(px(self.window_width), px(self.window_height)))
            .width
            / px(1.0))
        .max(1.0);
        player_workspace_metrics_for_width(available)
    }
'''
text = replace_once(text, old_metrics, new_metrics, "responsive player metrics method")
insert_after = '''struct PlayerWorkspaceMetrics {
    slot_size: f32,
    slot_gap: f32,
    grid_width: f32,
    panel_padding: f32,
    outer_padding: f32,
    compact: bool,
}
'''
metrics_helper = insert_after + '''
fn player_workspace_metrics_for_width(available: f32) -> PlayerWorkspaceMetrics {
    let available = available.max(1.0);
    let compact = available < 620.0;
    let tight = available < 470.0;
    let outer_padding = if tight { 6.0 } else if compact { 10.0 } else { 18.0 };
    let panel_padding = if tight { 7.0 } else if compact { 11.0 } else { 18.0 };
    let slot_gap = if tight { 2.0 } else if compact { 3.0 } else { 4.0 };
    let available_grid = (available - outer_padding * 2.0 - panel_padding * 2.0 - 2.0).max(1.0);
    let natural_slot = ((available_grid - slot_gap * 8.0) / 9.0).floor();
    // 22 px keeps the controls usable on very narrow layouts while still guaranteeing that
    // a nine-slot row never invents width beyond its parent. Extremely small stages become
    // horizontally dense instead of offsetting the hotbar outside the inventory card.
    let slot_size = natural_slot.clamp(22.0, 52.0).min((available_grid / 9.0).max(1.0));
    let grid_width = (slot_size * 9.0 + slot_gap * 8.0)
        .min(available_grid)
        .max(1.0);
    PlayerWorkspaceMetrics {
        slot_size,
        slot_gap,
        grid_width,
        panel_padding,
        outer_padding,
        compact,
    }
}
'''
text = replace_once(text, insert_after, metrics_helper, "player metrics helper")
text = replace_once(
    text,
    '''                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .children((0..3).map(|row| {''',
    '''                div()
                    .w_full()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.0))
                    .children((0..3).map(|row| {''',
    "main inventory full-width rows",
)
text = replace_once(
    text,
    '''                div()
                    .pt(px(8.0))
                    .border_t_1()''',
    '''                div()
                    .w_full()
                    .min_w(px(0.0))
                    .pt(px(8.0))
                    .border_t_1()
                    .flex()
                    .justify_center()''',
    "hotbar full-width wrapper",
)
text += '''

#[cfg(test)]
mod responsive_inventory_layout_tests {
    use super::player_workspace_metrics_for_width;

    #[::core::prelude::v1::test]
    fn nine_slot_grid_never_exceeds_available_center_width() {
        for available in [240.0_f32, 320.0, 420.0, 560.0, 800.0] {
            let metrics = player_workspace_metrics_for_width(available);
            let occupied = metrics.grid_width
                + metrics.panel_padding * 2.0
                + metrics.outer_padding * 2.0
                + 2.0;
            assert!(occupied <= available + 0.5, "available={available} occupied={occupied}");
        }
    }
}
'''
path.write_text(text, encoding="utf-8")


# ---------------------------------------------------------------------------
# Actor storage identity. digp stores an opaque 8-byte StorageKey, not NBT
# UniqueID directly. Match BedrockMap/Bedrock's UniqueID -> StorageKey mapping.
# ---------------------------------------------------------------------------
path = ROOT / "crates/bedrock-world/src/chunk.rs"
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
/// Actor unique id used by modern `actorprefix` records.
pub struct ActorUid(pub i64);

impl ActorUid {
    #[must_use]
    /// Encodes this actor id as `actorprefix<little-endian i64>`.
    pub fn storage_key(self) -> Bytes {
        let mut bytes = Vec::with_capacity(19);
        bytes.extend_from_slice(b"actorprefix");
        bytes.extend_from_slice(&self.0.to_le_bytes());
        Bytes::from(bytes)
    }
''',
    '''#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
/// Opaque 8-byte actor storage token stored in `digp` and appended to `actorprefix`.
///
/// This is deliberately not the NBT `UniqueID`. Bedrock derives this token from
/// `UniqueID` by complementing the world-start-count half and encoding the result
/// big-endian before storing those raw bytes in the database key.
pub struct ActorUid(pub i64);

impl ActorUid {
    #[must_use]
    /// Derives the modern actor storage token from the NBT `UniqueID` using the
    /// same transformation as Bedrock/BedrockMap.
    pub fn from_unique_id(unique_id: i64) -> Self {
        let unique = unique_id as u64;
        let world_start_count = unique >> 32;
        let index = unique & 0xffff_ffff;
        let storage = ((0xffff_ffff_u64.wrapping_sub(world_start_count)) << 32) | index;
        Self(i64::from_le_bytes(storage.to_be_bytes()))
    }

    #[must_use]
    /// Returns the exact eight storage bytes referenced by `digp`.
    pub const fn raw_storage_bytes(self) -> [u8; 8] {
        self.0.to_le_bytes()
    }

    #[must_use]
    /// Encodes this storage token as `actorprefix<raw 8 bytes>`.
    pub fn storage_key(self) -> Bytes {
        let mut bytes = Vec::with_capacity(19);
        bytes.extend_from_slice(b"actorprefix");
        bytes.extend_from_slice(&self.raw_storage_bytes());
        Bytes::from(bytes)
    }
''',
    "actor storage identity",
)
path.write_text(text, encoding="utf-8")


# ---------------------------------------------------------------------------
# bedrock-world write primitives: correct actor storage-key generation, support
# exact actor NBT editing/deletion, and support deleting player LevelDB records.
# ---------------------------------------------------------------------------
path = ROOT / "crates/bedrock-world/src/world.rs"
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''    /// Put player blocking.
    pub fn put_player_blocking(&self, player: &PlayerData) -> Result<()> {
        self.ensure_writable()?;
        let Some(key) = player.id.storage_key() else {
            return Err(BedrockWorldError::Validation(
                "player id has no LevelDB key".to_string(),
            ));
        };
        self.storage().put(key.as_ref(), &player.raw)
    }
''',
    '''    /// Put player blocking.
    pub fn put_player_blocking(&self, player: &PlayerData) -> Result<()> {
        self.ensure_writable()?;
        let Some(key) = player.id.storage_key() else {
            return Err(BedrockWorldError::Validation(
                "player id has no LevelDB key".to_string(),
            ));
        };
        self.storage().put(key.as_ref(), &player.raw)
    }

    /// Deletes an exact LevelDB-backed player record.
    ///
    /// Legacy level.dat pseudo players are intentionally rejected because they are not
    /// independent LevelDB records.
    pub fn delete_player_blocking(&self, id: &PlayerId) -> Result<()> {
        self.ensure_writable()?;
        let Some(key) = id.storage_key() else {
            return Err(BedrockWorldError::Validation(
                "player id has no deletable LevelDB key".to_string(),
            ));
        };
        let mut transaction = self.transaction();
        transaction.delete_raw_key(Bytes::copy_from_slice(key.as_ref()));
        transaction.commit()
    }
''',
    "player delete primitive",
)
text = text.replace("actor.unique_id.map(ActorUid).ok_or_else(|| {", "actor.unique_id.map(ActorUid::from_unique_id).ok_or_else(|| {")
# Two occurrences are expected: put_actor and move_actor.
if text.count("ActorUid::from_unique_id") < 2:
    raise RuntimeError("actor UniqueID conversion replacements were incomplete")
insert_before = '''    /// Deletes a modern actor record and removes it from the chunk digest.
'''
new_actor_methods = '''    /// Replaces one actor NBT document selected by its NBT `UniqueID`.
    ///
    /// Modern actors preserve the exact storage token read from `digp`; changing `UniqueID`
    /// in the edited document is rejected because that would require a new entity identity.
    pub fn edit_actor_nbt_by_unique_id_blocking(
        &self,
        pos: ChunkPos,
        unique_id: i64,
        nbt: NbtTag,
    ) -> Result<BTreeSet<ChunkPos>> {
        self.ensure_writable()?;
        let records = self.actors_in_chunk_blocking(pos)?;
        let source = records
            .iter()
            .find(|record| record.entity.unique_id == Some(unique_id))
            .map(|record| record.source.clone())
            .ok_or_else(|| BedrockWorldError::Validation(format!("actor UniqueID {unique_id} does not exist")))?;
        let value = Bytes::from(serialize_root_nbt(&nbt)?);
        let mut report = WorldParseReport::default();
        let mut parsed = parse_entities_from_value(&value, &mut report);
        if parsed.len() != 1 {
            return Err(BedrockWorldError::Validation(
                "edited actor NBT must contain exactly one entity root".to_string(),
            ));
        }
        let edited = parsed.remove(0);
        if edited.unique_id != Some(unique_id) {
            return Err(BedrockWorldError::Validation(
                "editing actor UniqueID is not supported; duplicate/delete and recreate instead"
                    .to_string(),
            ));
        }
        let target = edited.position.map_or(pos, |position| {
            BlockPos {
                x: position[0].floor() as i32,
                y: position[1].floor() as i32,
                z: position[2].floor() as i32,
            }
            .to_chunk_pos(pos.dimension)
        });
        let mut affected = BTreeSet::from([pos]);
        match source {
            ActorSource::ActorPrefix(storage_uid) => {
                let mut transaction = self.transaction();
                if target != pos {
                    transaction.delete_actor(pos, storage_uid)?;
                    affected.insert(target);
                }
                transaction.put_actor(target, storage_uid, value)?;
                transaction.commit()?;
            }
            ActorSource::InlineChunk(inline_key) => {
                if target != pos {
                    return Err(BedrockWorldError::Validation(
                        "moving a legacy inline actor to another chunk is not supported"
                            .to_string(),
                    ));
                }
                let raw = self.storage().get(&inline_key.encode())?.ok_or_else(|| {
                    BedrockWorldError::Validation("legacy inline actor record disappeared".to_string())
                })?;
                let mut inline_report = WorldParseReport::default();
                let mut actors = parse_entities_from_value(&raw, &mut inline_report);
                let actor = actors
                    .iter_mut()
                    .find(|actor| actor.unique_id == Some(unique_id))
                    .ok_or_else(|| BedrockWorldError::Validation("legacy inline actor disappeared".to_string()))?;
                *actor = edited;
                let mut encoded = Vec::new();
                for actor in actors {
                    encoded.extend(serialize_root_nbt(&actor.nbt)?);
                }
                let mut transaction = self.transaction();
                transaction.put_raw_key(inline_key.encode(), Bytes::from(encoded));
                transaction.commit()?;
            }
        }
        Ok(affected)
    }

    /// Deletes exactly one actor selected by NBT `UniqueID` from modern or legacy storage.
    pub fn delete_actor_by_unique_id_blocking(&self, pos: ChunkPos, unique_id: i64) -> Result<()> {
        self.ensure_writable()?;
        let records = self.actors_in_chunk_blocking(pos)?;
        let source = records
            .iter()
            .find(|record| record.entity.unique_id == Some(unique_id))
            .map(|record| record.source.clone())
            .ok_or_else(|| BedrockWorldError::Validation(format!("actor UniqueID {unique_id} does not exist")))?;
        match source {
            ActorSource::ActorPrefix(storage_uid) => self.delete_actor_blocking(pos, storage_uid),
            ActorSource::InlineChunk(inline_key) => {
                let raw = self.storage().get(&inline_key.encode())?.ok_or_else(|| {
                    BedrockWorldError::Validation("legacy inline actor record disappeared".to_string())
                })?;
                let mut report = WorldParseReport::default();
                let mut removed = false;
                let actors = parse_entities_from_value(&raw, &mut report)
                    .into_iter()
                    .filter(|actor| {
                        let keep = actor.unique_id != Some(unique_id);
                        removed |= !keep;
                        keep
                    })
                    .collect::<Vec<_>>();
                if !removed {
                    return Err(BedrockWorldError::Validation("legacy inline actor disappeared".to_string()));
                }
                let mut transaction = self.transaction();
                if actors.is_empty() {
                    transaction.delete_raw_key(inline_key.encode());
                } else {
                    let mut encoded = Vec::new();
                    for actor in actors {
                        encoded.extend(serialize_root_nbt(&actor.nbt)?);
                    }
                    transaction.put_raw_key(inline_key.encode(), Bytes::from(encoded));
                }
                transaction.commit()
            }
        }
    }

'''
text = replace_once(text, insert_before, new_actor_methods + insert_before, "specific actor write methods")
path.write_text(text, encoding="utf-8")


# ---------------------------------------------------------------------------
# Overlay query identity: retain UniqueID in the non-tiled query path too.
# ---------------------------------------------------------------------------
path = ROOT / "crates/bedrock-world/src/query.rs"
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''pub struct EntityOverlay {
    /// Entity identifier decoded from NBT, when present.
    pub identifier: Option<String>,
    /// World position `[x, y, z]` decoded from the entity record.
    pub position: [f64; 3],''',
    '''pub struct EntityOverlay {
    /// Entity identifier decoded from NBT, when present.
    pub identifier: Option<String>,
    /// NBT `UniqueID`, retained for exact editor targeting.
    pub unique_id: Option<i64>,
    /// World position `[x, y, z]` decoded from the entity record.
    pub position: [f64; 3],''',
    "entity overlay unique id",
)
text = replace_once(
    text,
    '''        target.push(EntityOverlay {
            identifier: entity.identifier,
            chunk: BlockPos {''',
    '''        target.push(EntityOverlay {
            identifier: entity.identifier,
            unique_id: entity.unique_id,
            chunk: BlockPos {''',
    "entity overlay construction unique id",
)
path.write_text(text, encoding="utf-8")


# ---------------------------------------------------------------------------
# Persistent map-info tiles: retain enough actor identity to hit-test/edit an
# exact entity, and surface parser skips instead of silently hiding them.
# ---------------------------------------------------------------------------
path = ROOT / "src/core/minecraft/map_info_cache.rs"
text = path.read_text(encoding="utf-8")
text = replace_once(text, "const CACHE_VERSION: u16 = 3;", "const CACHE_VERSION: u16 = 4;", "map info cache v4")
text = replace_once(
    text,
    '''pub struct MapInfoEntity {
    /// Absolute X coordinate in blocks.
    pub block_x: f32,
    /// Absolute Z coordinate in blocks.
    pub block_z: f32,
    /// Bedrock entity identifier, when available.
    pub identifier: Option<String>,
}''',
    '''pub struct MapInfoEntity {
    /// Absolute X coordinate in blocks.
    pub block_x: f32,
    /// Absolute Y coordinate in blocks.
    pub block_y: f32,
    /// Absolute Z coordinate in blocks.
    pub block_z: f32,
    /// Chunk whose Entity/digp record referenced this actor.
    pub source_chunk_x: i32,
    /// Chunk whose Entity/digp record referenced this actor.
    pub source_chunk_z: i32,
    /// Dimension of the source chunk.
    pub dimension_id: i32,
    /// NBT UniqueID used to resolve the exact actor for editing.
    pub unique_id: Option<i64>,
    /// Bedrock entity identifier, when available.
    pub identifier: Option<String>,
}''',
    "map info entity identity",
)
text = replace_once(
    text,
    '''pub struct MapInfoTilePayload {
    /// Entity markers within the tile's chunk range.
    pub entities: Vec<MapInfoEntity>,''',
    '''pub struct MapInfoTilePayload {
    /// Entity markers within the tile's chunk range.
    pub entities: Vec<MapInfoEntity>,
    /// Parsed entity roots omitted only because they had no usable Pos value.
    pub skipped_entity_count: u32,''',
    "tile skipped entity count",
)
text = replace_once(
    text,
    '''pub struct MapInfoOverlaySnapshot {
    /// Entity markers from all requested tiles.
    pub entities: Vec<MapInfoEntity>,''',
    '''pub struct MapInfoOverlaySnapshot {
    /// Entity markers from all requested tiles.
    pub entities: Vec<MapInfoEntity>,
    /// Parsed entity roots omitted only because they had no usable Pos value.
    pub skipped_entity_count: usize,''',
    "snapshot skipped entity count",
)
text = replace_once(
    text,
    '''                            let Some(position) = entity.position else {
                                continue;
                            };
                            payload.entities.push(MapInfoEntity {
                                block_x: position[0] as f32,
                                block_z: position[2] as f32,
                                identifier: entity.identifier.clone(),
                            });''',
    '''                            let Some(position) = entity.position else {
                                payload.skipped_entity_count =
                                    payload.skipped_entity_count.saturating_add(1);
                                continue;
                            };
                            payload.entities.push(MapInfoEntity {
                                block_x: position[0] as f32,
                                block_y: position[1] as f32,
                                block_z: position[2] as f32,
                                source_chunk_x: result.pos.x,
                                source_chunk_z: result.pos.z,
                                dimension_id: result.pos.dimension.id(),
                                unique_id: entity.unique_id,
                                identifier: entity.identifier.clone(),
                            });''',
    "map info entity construction",
)
text = replace_once(
    text,
    '''        for payload in payloads {
            snapshot.entities.extend(payload.entities);''',
    '''        for payload in payloads {
            snapshot.skipped_entity_count = snapshot
                .skipped_entity_count
                .saturating_add(payload.skipped_entity_count as usize);
            snapshot.entities.extend(payload.entities);''',
    "aggregate skipped entity count",
)
path.write_text(text, encoding="utf-8")


# ---------------------------------------------------------------------------
# Map viewer model: retain exact target identity in paint points/context state and
# add an individual Actor edit target.
# ---------------------------------------------------------------------------
path = ROOT / "src/ui/window/map_viewer/model.rs"
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''    Actors(ChunkPos),
    HeightMap(ChunkPos),''',
    '''    Actors(ChunkPos),
    Actor { chunk: ChunkPos, unique_id: i64 },
    HeightMap(ChunkPos),''',
    "individual actor edit target",
)
text = replace_once(
    text,
    '''            Self::Actors(pos) => format!("edit actors chunk {},{}", pos.x, pos.z),
            Self::HeightMap(pos) =>''',
    '''            Self::Actors(pos) => format!("edit actors chunk {},{}", pos.x, pos.z),
            Self::Actor { chunk, unique_id } => {
                format!("edit actor {unique_id} in chunk {},{}", chunk.x, chunk.z)
            }
            Self::HeightMap(pos) =>''',
    "individual actor operation label",
)
text = replace_once(
    text,
    '''    pub(super) overlay_paint: Option<Arc<ProfessionalOverlayPaintCache>>,
    /// True only after the current overlay scope was validated against LevelDB.''',
    '''    pub(super) overlay_paint: Option<Arc<ProfessionalOverlayPaintCache>>,
    /// Entity currently hit by a right-click on the map overlay.
    pub(super) entity_context_target: Option<EntityContextTarget>,
    /// True only after the current overlay scope was validated against LevelDB.''',
    "entity context state",
)
text = replace_once(
    text,
    '''            cache.entity_points.push(EntityOverlayPoint {
                block_x: entity.block_x,
                block_z: entity.block_z,
                identifier: entity.identifier.clone(),
            });''',
    '''            cache.entity_points.push(EntityOverlayPoint {
                block_x: entity.block_x,
                block_y: entity.block_y,
                block_z: entity.block_z,
                source_chunk: ChunkPos {
                    x: entity.source_chunk_x,
                    z: entity.source_chunk_z,
                    dimension: Dimension::from_id(entity.dimension_id),
                },
                unique_id: entity.unique_id,
                identifier: entity.identifier.clone(),
            });''',
    "map info paint entity identity",
)
text = replace_once(
    text,
    '''            cache.entity_points.push(EntityOverlayPoint {
                block_x: entity.position[0] as f32,
                block_z: entity.position[2] as f32,
                identifier: entity.identifier.clone(),
            });''',
    '''            cache.entity_points.push(EntityOverlayPoint {
                block_x: entity.position[0] as f32,
                block_y: entity.position[1] as f32,
                block_z: entity.position[2] as f32,
                source_chunk: entity.chunk,
                unique_id: entity.unique_id,
                identifier: entity.identifier.clone(),
            });''',
    "query paint entity identity",
)
text = replace_once(
    text,
    '''#[derive(Clone, Debug, PartialEq)]
pub(super) struct EntityOverlayPoint {
    pub(super) block_x: f32,
    pub(super) block_z: f32,
    pub(super) identifier: Option<String>,
}''',
    '''#[derive(Clone, Debug, PartialEq)]
pub(super) struct EntityContextTarget {
    pub(super) source_chunk: ChunkPos,
    pub(super) unique_id: Option<i64>,
    pub(super) identifier: Option<String>,
    pub(super) position: [f32; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct EntityOverlayPoint {
    pub(super) block_x: f32,
    pub(super) block_y: f32,
    pub(super) block_z: f32,
    pub(super) source_chunk: ChunkPos,
    pub(super) unique_id: Option<i64>,
    pub(super) identifier: Option<String>,
}''',
    "entity overlay point identity",
)
path.write_text(text, encoding="utf-8")


# ---------------------------------------------------------------------------
# Right-click hit testing and entity-specific context actions.
# ---------------------------------------------------------------------------
path = ROOT / "src/ui/window/map_viewer/interactions.rs"
text = path.read_text(encoding="utf-8")
open_marker = '''    pub(super) fn open_context_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
'''
entity_hit = '''    fn entity_context_target_at(&self, position: Point<Pixels>) -> Option<EntityContextTarget> {
        if !self.overlay_options.entities {
            return None;
        }
        let paint = self.professional.overlay_paint.as_ref()?;
        let local = self.stage_local_position(position);
        let pointer_x = local.x / px(1.0);
        let pointer_y = local.y / px(1.0);
        let hit_radius = 18.0_f32;
        let hit_radius_sq = hit_radius * hit_radius;
        paint
            .entity_points
            .iter()
            .filter_map(|entity| {
                let (screen_x, screen_y) = viewport_screen_for_block(
                    self.viewport,
                    self.active_layout,
                    entity.block_x.floor() as i32,
                    entity.block_z.floor() as i32,
                )?;
                let dx = screen_x - pointer_x;
                let dy = screen_y - pointer_y;
                let distance_sq = dx * dx + dy * dy;
                (distance_sq <= hit_radius_sq).then_some((distance_sq, entity))
            })
            .min_by(|left, right| left.0.total_cmp(&right.0))
            .map(|(_, entity)| EntityContextTarget {
                source_chunk: entity.source_chunk,
                unique_id: entity.unique_id,
                identifier: entity.identifier.clone(),
                position: [entity.block_x, entity.block_y, entity.block_z],
            })
    }

'''
text = replace_once(text, open_marker, entity_hit + open_marker, "entity right click hit test")
text = replace_once(
    text,
    '''        self.ui_state.top_more_open = false;
        self.ui_state.context_more_open = false;''',
    '''        self.ui_state.top_more_open = false;
        self.players.context_target = None;
        self.professional.entity_context_target = self.entity_context_target_at(position);
        self.ui_state.context_more_open = false;''',
    "set entity context target",
)
text = replace_once(
    text,
    '''        let changed = self.context_menu.take().is_some()
            || self.ui_state.context_more_open''',
    '''        let changed = self.context_menu.take().is_some()
            || self.professional.entity_context_target.take().is_some()
            || self.ui_state.context_more_open''',
    "clear entity context target",
)
# The generic close-all path should also clear an entity target.
text = replace_once(
    text,
    '''        let changed = self.context_menu.take().is_some()
            || self.player_workspace.item_context_menu.take().is_some()''',
    '''        let changed = self.context_menu.take().is_some()
            || self.professional.entity_context_target.take().is_some()
            || self.player_workspace.item_context_menu.take().is_some()''',
    "close all entity context",
)
# Player marker context has precedence over entity hit context.
text = replace_once(
    text,
    '''        self.players.context_target = Some(player_id);
        self.context_menu = Some(ContextMenuState {''',
    '''        self.players.context_target = Some(player_id);
        self.professional.entity_context_target = None;
        self.context_menu = Some(ContextMenuState {''',
    "player context precedence",
)
path.write_text(text, encoding="utf-8")


path = ROOT / "src/ui/window/map_viewer/menus.rs"
text = path.read_text(encoding="utf-8")
insert_before_player = '''        if let Some(player_id) = self.players.context_target.clone() {
'''
entity_menu = '''        if let Some(actor) = self.professional.entity_context_target.clone() {
            let identifier = actor
                .identifier
                .clone()
                .unwrap_or_else(|| "minecraft:unknown".to_string());
            let edit_target = actor.unique_id.map(|unique_id| EditTarget::Actor {
                chunk: actor.source_chunk,
                unique_id,
            });
            let mut entries = Vec::new();
            if let Some(target) = edit_target.clone() {
                let entity = cx.entity();
                entries.push(ContextMenuEntry::item(
                    ContextMenuItem::new("编辑此实体 NBT")
                        .description(format!("{identifier} · UniqueID {}", actor.unique_id.unwrap_or_default()))
                        .on_click(move |cx| {
                            let target = target.clone();
                            entity.update(cx, move |this, cx| {
                                this.context_menu = None;
                                this.professional.entity_context_target = None;
                                this.load_edit_detail(target, cx);
                            })
                        }),
                ));
                let target = edit_target.expect("entity edit target");
                let entity = cx.entity();
                entries.push(ContextMenuEntry::item(
                    ContextMenuItem::new("删除此实体")
                        .danger(true)
                        .description("第一次点击进入确认；再次执行后可从历史记录撤销")
                        .on_click(move |cx| {
                            let target = target.clone();
                            entity.update(cx, move |this, cx| {
                                this.context_menu = None;
                                this.professional.entity_context_target = None;
                                this.confirm_or_run_edit(target, EditAction::Delete, cx);
                            })
                        }),
                ));
            } else {
                let entity = cx.entity();
                entries.push(ContextMenuEntry::item(
                    ContextMenuItem::new("查看所在 chunk 实体")
                        .description("该实体缺少 UniqueID，不能安全执行单实体写入")
                        .on_click(move |cx| {
                            let chunk = actor.source_chunk;
                            entity.update(cx, move |this, cx| {
                                this.context_menu = None;
                                this.professional.entity_context_target = None;
                                this.load_edit_detail(EditTarget::Actors(chunk), cx);
                            })
                        }),
                ));
            }
            groups.insert(0, ContextMenuGroup::titled(identifier, entries));
        }
'''
text = replace_once(text, insert_before_player, entity_menu + insert_before_player, "entity context menu")
path.write_text(text, encoding="utf-8")


# ---------------------------------------------------------------------------
# Editor: individual actor visual summary + editable raw NBT, exact save/delete,
# and player delete support.
# ---------------------------------------------------------------------------
path = ROOT / "src/ui/window/map_viewer/editor.rs"
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''        EditTarget::Actors(pos) => actors_editor_detail(editor, pos),
        EditTarget::HeightMap(pos) =>''',
    '''        EditTarget::Actors(pos) => actors_editor_detail(editor, pos),
        EditTarget::Actor { chunk, unique_id } => actor_editor_detail(editor, chunk, unique_id),
        EditTarget::HeightMap(pos) =>''',
    "load individual actor detail",
)
text = replace_once(
    text,
    '''        (EditTarget::Player(id), EditAction::Save) => {
            let text = document_text.ok_or_else(|| {''',
    '''        (EditTarget::Player(id), EditAction::Delete) => {
            editor.world().delete_player_blocking(&id)?;
            Ok(MapEditInvalidation::metadata())
        }
        (EditTarget::Player(id), EditAction::Save) => {
            let text = document_text.ok_or_else(|| {''',
    "player delete edit action",
)
actors_delete = '''        (EditTarget::Actors(pos), EditAction::Delete) => {
            let Some(uid) = editor
                .actors_in_chunk(pos)?
                .into_iter()
                .find_map(|actor| actor.uid)
            else {
                return Err(bedrock_render::BedrockRenderError::Validation(
                    "chunk has no modern actor UID to delete".to_string(),
                ));
            };
            editor.delete_actor(pos, uid)
        }
'''
replacement_actor_actions = actors_delete + '''        (EditTarget::Actor { chunk, unique_id }, EditAction::Save) => {
            let text = document_text.ok_or_else(|| {
                bedrock_render::BedrockRenderError::Validation(
                    "missing actor JSON document".to_string(),
                )
            })?;
            let nbt = serde_json::from_str::<NbtTag>(&text).map_err(|error| {
                bedrock_render::BedrockRenderError::Validation(format!(
                    "actor JSON is not valid NBT JSON: {error}"
                ))
            })?;
            let affected = editor
                .world()
                .edit_actor_nbt_by_unique_id_blocking(chunk, unique_id, nbt)?;
            Ok(MapEditInvalidation::chunks(affected).with_metadata())
        }
        (EditTarget::Actor { chunk, unique_id }, EditAction::Delete) => {
            editor
                .world()
                .delete_actor_by_unique_id_blocking(chunk, unique_id)?;
            Ok(MapEditInvalidation::chunk(chunk).with_metadata())
        }
'''
text = replace_once(text, actors_delete, replacement_actor_actions, "individual actor edit actions")
text = replace_once(
    text,
    '''        EditTarget::HsaChunk(chunk)
        | EditTarget::BlockEntities(chunk)
        | EditTarget::Actors(chunk)
        | EditTarget::HeightMap(chunk)''',
    '''        EditTarget::HsaChunk(chunk)
        | EditTarget::BlockEntities(chunk)
        | EditTarget::Actors(chunk)
        | EditTarget::Actor { chunk, .. }
        | EditTarget::HeightMap(chunk)''',
    "actor history chunk",
)
actor_detail_marker = '''pub(super) fn heightmap_editor_detail(
'''
actor_detail = '''pub(super) fn actor_editor_detail(
    editor: &MapWorldEditor,
    chunk: ChunkPos,
    unique_id: i64,
) -> bedrock_render::Result<ProfessionalDetail> {
    let actor = editor
        .actors_in_chunk(chunk)?
        .into_iter()
        .find(|actor| actor.entity.unique_id == Some(unique_id))
        .ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation(format!(
                "actor UniqueID {unique_id} does not exist in chunk {},{}",
                chunk.x, chunk.z
            ))
        })?;
    let identifier = actor
        .entity
        .identifier
        .clone()
        .unwrap_or_else(|| "minecraft:unknown".to_string());
    let position = actor.entity.position;
    Ok(ProfessionalDetail::Editor {
        target: EditTarget::Actor { chunk, unique_id },
        title: SharedString::from(format!("实体 {identifier}")),
        sections: vec![EditSection {
            title: SharedString::from("实体信息"),
            rows: vec![
                readonly_row("identifier", identifier),
                readonly_row("UniqueID", unique_id.to_string()),
                readonly_row(
                    "position",
                    position.map_or_else(
                        || "缺失".to_string(),
                        |value| format!("{:.3}, {:.3}, {:.3}", value[0], value[1], value[2]),
                    ),
                ),
                readonly_row("source chunk", format!("{}, {}", chunk.x, chunk.z)),
                readonly_row("storage", format!("{:?}", actor.source)),
            ],
        }],
        // Individual actors intentionally expose the NBT root itself. Unlike the chunk-level
        // Actors inspector this document can be parsed back and written safely.
        json: pretty_json(serde_json::json!(actor.entity.nbt)),
    })
}

'''
text = replace_once(text, actor_detail_marker, actor_detail + actor_detail_marker, "individual actor editor detail")
# Make player deletion refresh the player list after the generic write completes.
text = replace_once(
    text,
    '''        let operation = edit_action_status(&action, &target);
        let history_spec = match edit_history_spec(&self.world_path, &target, &action) {''',
    '''        let operation = edit_action_status(&action, &target);
        let deletes_player = matches!((&target, &action), (EditTarget::Player(_), EditAction::Delete));
        let history_spec = match edit_history_spec(&self.world_path, &target, &action) {''',
    "player delete completion flag",
)
text = replace_once(
    text,
    '''                                this.apply_map_edit_invalidation(&invalidation, cx);
                                this.status = SharedString::from(message.clone());''',
    '''                                this.apply_map_edit_invalidation(&invalidation, cx);
                                if deletes_player {
                                    this.players.selected = None;
                                    this.players.detail = None;
                                    this.players.context_target = None;
                                    this.refresh_players(cx);
                                }
                                this.status = SharedString::from(message.clone());''',
    "refresh players after delete",
)
path.write_text(text, encoding="utf-8")


# ---------------------------------------------------------------------------
# Player workspace exposes deletion directly beside the NBT editor. It reuses the
# shared two-step edit confirmation/history path.
# ---------------------------------------------------------------------------
path = ROOT / "src/ui/window/map_viewer/player_workspace.rs"
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''        let title = SharedString::from(stable_middle_ellipsis(title.as_ref(), 38));
        div()''',
    '''        let title = SharedString::from(stable_middle_ellipsis(title.as_ref(), 38));
        let delete_player_id = detail.id.clone();
        div()''',
    "player header delete id",
)
text = replace_once(
    text,
    '''            .child(toolbar_button(colors, "玩家 NBT").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| this.open_selected_player_in_editor(cx)),
            ))
''',
    '''            .child(toolbar_button(colors, "玩家 NBT").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| this.open_selected_player_in_editor(cx)),
            ))
            .child(danger_button(colors, "删除玩家").on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.confirm_or_run_edit(
                        EditTarget::Player(delete_player_id.clone()),
                        EditAction::Delete,
                        cx,
                    )
                }),
            ))
''',
    "player delete header button",
)
path.write_text(text, encoding="utf-8")


# ---------------------------------------------------------------------------
# Overlay completion status reports entity count and no-Pos skips. This makes a
# missing-query/parser condition visible instead of silently looking like an icon issue.
# ---------------------------------------------------------------------------
path = ROOT / "src/ui/window/map_viewer/overlays.rs"
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''                        this.status = SharedString::from(format!(
                            "实体/叠加区域已完整 · 缓存 {}/{} · 重建 {}",
                            map_info.cached_tile_count,
                            map_info.requested_tile_count,
                            map_info.rebuilt_tile_count
                        ));''',
    '''                        this.status = SharedString::from(format!(
                            "实体/叠加区域已完整 · 实体 {} · 无坐标跳过 {} · 缓存 {}/{} · 重建 {}",
                            map_info.entities.len(),
                            map_info.skipped_entity_count,
                            map_info.cached_tile_count,
                            map_info.requested_tile_count,
                            map_info.rebuilt_tile_count
                        ));''',
    "entity overlay diagnostics status",
)
path.write_text(text, encoding="utf-8")


# ---------------------------------------------------------------------------
# Tests: namespace normalization already exists; add exact StorageKey derivation
# and individual actor target operation label coverage in existing test modules.
# ---------------------------------------------------------------------------
path = ROOT / "src/ui/window/map_viewer/tests.rs"
text = path.read_text(encoding="utf-8")
text += '''

#[::core::prelude::v1::test]
fn individual_actor_edit_target_has_stable_identity() {
    let target = EditTarget::Actor {
        chunk: ChunkPos {
            x: -5,
            z: 4,
            dimension: Dimension::Overworld,
        },
        unique_id: 123456789,
    };
    assert!(target.operation_label().contains("123456789"));
}
'''
path.write_text(text, encoding="utf-8")

path = ROOT / "crates/bedrock-world/src/chunk.rs"
text = path.read_text(encoding="utf-8")
text += '''

#[cfg(test)]
mod actor_storage_key_tests {
    use super::ActorUid;

    #[test]
    fn unique_id_is_not_used_as_raw_actorprefix_suffix() {
        let unique_id = 0x0000_0002_1234_5678_i64;
        let storage = ActorUid::from_unique_id(unique_id);
        let expected_numeric = ((0xffff_ffff_u64 - 2) << 32) | 0x1234_5678;
        assert_eq!(storage.raw_storage_bytes(), expected_numeric.to_be_bytes());
        assert_ne!(storage.raw_storage_bytes(), unique_id.to_le_bytes());
    }
}
'''
path.write_text(text, encoding="utf-8")
