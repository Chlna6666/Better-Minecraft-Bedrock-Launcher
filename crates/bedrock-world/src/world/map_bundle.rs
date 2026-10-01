//! Atomic installation of BMCBL map bundles into LevelDB player inventories.

use super::*;
use crate::chunk::Dimension;
use crate::editor::{
    BlockEdit, BlockEditOptions, BlockEditPlan, BlockStateCondition, WriteGuard, plan_block_edits,
};
use crate::item::set_item_display;
use indexmap::IndexMap;

const PLAYER_SLOTS: std::ops::Range<i8> = 0..36;
const SHULKER_SLOTS: usize = 27;
const VERIFIED_UP_FRAME_VERSION: i32 = 18_168_865;

/// Prepared map records and player inventory edits for one LevelDB batch.
///
/// Preparation only reads the world. The chosen map keys are not reserved until [`Self::commit`],
/// which checks absent map keys and the original player bytes under the world mutation lock.
/// `level.dat.Player` is unsupported because that file cannot join the LevelDB batch.
pub struct MapPlayerImportPlan {
    bundle: MapBundle,
    player: PlayerData,
    map_ids: Vec<i64>,
    slots: Vec<PlayerInventorySlot>,
}

/// Prepared map records and one existing chest or shulker-box BlockEntity edit.
///
/// The source payload is retained for a commit-time comparison. This plan never creates or
/// replaces the physical block, and it does not modify another block entity in the same chunk.
pub struct MapBlockContainerImportPlan {
    bundle: MapBundle,
    chunk: ChunkPos,
    source: Option<Bytes>,
    entities: Vec<BlockEntity>,
    map_ids: Vec<i64>,
    slots: Vec<i8>,
}

/// One prepared filled-map insertion into an existing empty Bedrock item frame.
///
/// The frame's `Item` BlockEntity field and `item_frame_map_bit` block state are staged with the
/// map record in the same LevelDB batch. The existing frame orientation is retained.
pub struct MapFrameImportPlan {
    bundle: MapBundle,
    entity_updates: Vec<MapFrameEntityUpdate>,
    block_edit: BlockEditPlan,
    map_ids: Vec<i64>,
}

struct MapFrameEntityUpdate {
    chunk: ChunkPos,
    source: Option<Bytes>,
    entities: Vec<BlockEntity>,
}

struct FrameGridBlockEdits {
    edits: Vec<BlockEdit>,
    conditions: Vec<BlockStateCondition>,
}

impl MapFrameImportPlan {
    /// Numeric IDs of the maps to be displayed by the prepared frame grid.
    #[must_use]
    pub fn map_ids(&self) -> &[i64] {
        &self.map_ids
    }

    /// Chunks whose terrain or item-frame records will change.
    #[must_use]
    pub fn affected_chunks(&self) -> BTreeSet<ChunkPos> {
        self.block_edit.affected_chunks().clone()
    }

    /// Commits map pixels, frame `Item`, and its block-state map bit atomically.
    ///
    /// Source chunk records and absent map key are checked under the commit lock. The caller
    /// must stop honoring cancellation once this commit begins; later reversal uses history Undo.
    ///
    /// # Errors
    ///
    /// Returns validation, serialization, capacity, concurrent-write or storage errors.
    pub fn commit<S: StorageBackend>(self, world: &World<S>) -> Result<()> {
        world.ensure_writable()?;
        let mut transaction = world.transaction();
        self.block_edit.stage(&mut transaction);
        for update in self.entity_updates {
            transaction.update_block_entities(update.chunk, update.source, &update.entities)?;
        }
        for record in self.bundle.records() {
            transaction.save_map_item(record)?;
        }
        transaction.commit()
    }
}

impl MapBlockContainerImportPlan {
    /// Numeric IDs whose `map_<id>` records will be created.
    #[must_use]
    pub fn map_ids(&self) -> &[i64] {
        &self.map_ids
    }

    /// Slots used in the selected chest or shulker box.
    #[must_use]
    pub fn slots(&self) -> &[i8] {
        &self.slots
    }

    /// Chunk whose block-entity record will be changed by this plan.
    #[must_use]
    pub fn affected_chunks(&self) -> BTreeSet<ChunkPos> {
        BTreeSet::from([self.chunk])
    }

    /// Commits the map records and edited BlockEntity payload as one LevelDB batch.
    ///
    /// The source container record and absence of every map key are rechecked under the world
    /// mutation lock. Cancellation must be handled before calling this method; once the atomic
    /// batch starts, callers must report its actual result and use history Undo to reverse it.
    ///
    /// # Errors
    ///
    /// Returns validation, serialization, capacity, concurrent-write or storage errors.
    pub fn commit<S: StorageBackend>(self, world: &World<S>) -> Result<()> {
        world.ensure_writable()?;
        let mut transaction = world.transaction();
        for record in self.bundle.records() {
            transaction.save_map_item(record)?;
        }
        transaction.update_block_entities(self.chunk, self.source, &self.entities)?;
        transaction.commit()
    }
}

impl MapPlayerImportPlan {
    /// Numeric IDs whose `map_<id>` records will be created by this plan.
    #[must_use]
    pub fn map_ids(&self) -> &[i64] {
        &self.map_ids
    }

    /// Player inventory slots occupied by the maps or their shulker boxes.
    #[must_use]
    pub fn slots(&self) -> &[PlayerInventorySlot] {
        &self.slots
    }

    /// Commits all map records and the edited LevelDB player in one atomic storage batch.
    ///
    /// The source player and absence of every target map key are checked at commit. A failure
    /// before the batch write leaves all of them unchanged. This does not prevent an external
    /// Minecraft process from writing the world concurrently.
    ///
    /// # Errors
    ///
    /// Returns validation, serialization, capacity, concurrent-write or storage errors.
    pub fn commit<S: StorageBackend>(self, world: &World<S>) -> Result<()> {
        world.ensure_writable()?;
        let mut transaction = world.transaction();
        for record in self.bundle.records() {
            transaction.save_map_item(record)?;
        }
        transaction.update_player(&self.player)?;
        transaction.commit()
    }
}

impl<S: StorageBackend> World<S> {
    /// Prepares a north-up grid of upward-facing map frames over support blocks.
    ///
    /// `position` is the center of the requested grid. Grid columns extend east and rows extend
    /// south, in the row-major order carried by the bundle. A single-map bundle without grid
    /// metadata is treated as 1×1; a multi-map bundle must include verified dimensions. Every
    /// frame position must be air. Support positions may be air or already contain the selected
    /// support block. Newly created frames use the verified frame schema `18168865`; existing
    /// air and support entries may carry another storage version and are not migrated.
    /// Preparation does not modify the world. The resulting plan writes all missing support,
    /// frames, BlockEntities and `map_<id>` records in one LevelDB batch. Stone is the UI default.
    ///
    /// # Errors
    ///
    /// Returns validation, missing-chunk, unsupported-version, collision or storage errors.
    pub fn prepare_map_bundle_new_up_frame(
        &self,
        bundle: &MapBundle,
        chunk: ChunkPos,
        position: BlockPos,
        support: BlockState,
    ) -> Result<MapFrameImportPlan> {
        let items = bundle.flat_items()?;
        if position.to_chunk_pos(chunk.dimension) != chunk {
            return Err(BedrockWorldError::Validation(
                "frame anchor must be in the selected chunk".to_string(),
            ));
        }
        let (columns, rows) = frame_grid_dimensions(bundle, items.len())?;
        validate_upward_frame_support(&support)?;
        let frame_positions = frame_grid_positions(position, columns, rows)?;
        let (bundle, ids) = self.remap_bundle(bundle)?;
        let items = bundle.flat_items()?;
        let block_edits =
            self.prepare_upward_frame_blocks(chunk.dimension, &frame_positions, &support)?;
        let entity_updates =
            self.prepare_upward_frame_entities(chunk.dimension, &frame_positions, items)?;
        let guard = WriteGuard::confirmed(
            self.path().to_path_buf(),
            "create upward map frame grid with support",
        );
        let block_edit = plan_block_edits(
            self,
            &block_edits.edits,
            &block_edits.conditions,
            &guard,
            BlockEditOptions {
                commit_batch_chunks: frame_positions
                    .iter()
                    .map(|position| position.to_chunk_pos(chunk.dimension))
                    .collect::<BTreeSet<_>>()
                    .len()
                    .max(1),
                ..BlockEditOptions::default()
            },
        )?
        .ok_or_else(|| {
            BedrockWorldError::ConcurrentWrite("frame or support block changed".to_string())
        })?;
        Ok(MapFrameImportPlan {
            bundle,
            entity_updates,
            block_edit,
            map_ids: ids,
        })
    }

    fn prepare_upward_frame_blocks(
        &self,
        dimension: Dimension,
        frame_positions: &[BlockPos],
        support: &BlockState,
    ) -> Result<FrameGridBlockEdits> {
        let frame = upward_frame_state();
        let mut edits = Vec::with_capacity(frame_positions.len() * 2);
        let mut conditions = Vec::with_capacity(frame_positions.len() * 2);
        for frame_position in frame_positions {
            let (support_edit, pair_conditions) =
                self.prepare_upward_frame_pair(dimension, *frame_position, support)?;
            if let Some(support_edit) = support_edit {
                edits.push(support_edit);
            }
            edits.push(BlockEdit::new(dimension, *frame_position, frame.clone()));
            conditions.extend(pair_conditions);
        }
        Ok(FrameGridBlockEdits { edits, conditions })
    }

    fn prepare_upward_frame_pair(
        &self,
        dimension: Dimension,
        frame_position: BlockPos,
        support: &BlockState,
    ) -> Result<(Option<BlockEdit>, [BlockStateCondition; 2])> {
        let support_y = frame_position.y.checked_sub(1).ok_or_else(|| {
            BedrockWorldError::Validation(
                "frame support Y is outside world coordinates".to_string(),
            )
        })?;
        let below = BlockPos {
            x: frame_position.x,
            y: support_y,
            z: frame_position.z,
        };
        let frame_before = self.frame_target_state(dimension, frame_position)?;
        let support_before = self.frame_target_state(dimension, below)?;
        if frame_before.name != "minecraft:air"
            || (support_before.name != "minecraft:air" && !support_before.semantic_eq(support))
        {
            return Err(BedrockWorldError::Validation(format!(
                "frame grid collides at {}, {}, {} or its support differs",
                frame_position.x, frame_position.y, frame_position.z
            )));
        }
        let support_edit = (support_before.name == "minecraft:air")
            .then(|| BlockEdit::new(dimension, below, support.clone()));
        let conditions = [
            BlockStateCondition::new(dimension, below, support_before),
            BlockStateCondition::new(dimension, frame_position, frame_before),
        ];
        Ok((support_edit, conditions))
    }

    fn frame_target_state(&self, dimension: Dimension, position: BlockPos) -> Result<BlockState> {
        if let Some(state) = self.block_state(dimension, position)? {
            return Ok(state);
        }
        let chunk = self.chunk(position.to_chunk_pos(dimension))?;
        if chunk.records.is_empty()
            || self
                .subchunk_layer(
                    chunk.pos,
                    position.y,
                    crate::SubChunkDecodeMode::FullIndices,
                )?
                .is_some()
        {
            return Err(BedrockWorldError::Validation(format!(
                "frame target at {}, {}, {} has missing or unsupported terrain",
                position.x, position.y, position.z
            )));
        }
        // Bedrock omits all-air SubChunks. Keep this distinct from a persisted versioned entry.
        Ok(BlockState {
            name: "minecraft:air".to_string(),
            states: BTreeMap::new(),
            version: None,
        })
    }

    fn prepare_upward_frame_entities(
        &self,
        dimension: Dimension,
        frame_positions: &[BlockPos],
        items: &[NbtTag],
    ) -> Result<Vec<MapFrameEntityUpdate>> {
        let chunks = frame_positions
            .iter()
            .map(|position| position.to_chunk_pos(dimension))
            .collect::<BTreeSet<_>>();
        let support_positions = frame_positions
            .iter()
            .map(|position| {
                position
                    .y
                    .checked_sub(1)
                    .map(|y| [position.x, y, position.z])
                    .ok_or_else(|| {
                        BedrockWorldError::Validation(
                            "frame support Y is outside world coordinates".to_string(),
                        )
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        let occupied_positions = frame_positions
            .iter()
            .map(|position| [position.x, position.y, position.z])
            .chain(support_positions)
            .collect::<BTreeSet<_>>();
        let mut updates = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            let (records, source) = self.block_entities_snapshot(chunk)?;
            let mut entities = records
                .into_iter()
                .map(|record| record.entity)
                .collect::<Vec<_>>();
            if entities.iter().any(|entity| {
                entity
                    .position
                    .is_some_and(|position| occupied_positions.contains(&position))
            }) {
                return Err(BedrockWorldError::Validation(
                    "frame grid or support position has a BlockEntity".to_string(),
                ));
            }
            for (item, position) in items.iter().zip(frame_positions) {
                if position.to_chunk_pos(dimension) == chunk {
                    entities.push(upward_frame_entity(item, *position)?);
                }
            }
            updates.push(MapFrameEntityUpdate {
                chunk,
                source,
                entities,
            });
        }
        Ok(updates)
    }

    /// Prepares one map for an existing empty item frame at an exact block position.
    ///
    /// The frame block and BlockEntity must agree. The `facing_direction` and every unrelated
    /// block state and NBT field are retained. Preparation reads the world but writes nothing;
    /// the returned plan stages the map bit, frame item and new `map_<id>` together.
    ///
    /// # Errors
    ///
    /// Returns an error for multi-map bundles, occupied/missing frames, unexpected block state,
    /// stale source chunks, invalid map records, or storage/parse failures.
    pub fn prepare_map_bundle_frame(
        &self,
        bundle: &MapBundle,
        chunk: ChunkPos,
        position: BlockPos,
    ) -> Result<MapFrameImportPlan> {
        if bundle.flat_items()?.len() != 1 || position.to_chunk_pos(chunk.dimension) != chunk {
            return Err(BedrockWorldError::Validation(
                "one map and a matching frame chunk are required".to_string(),
            ));
        }
        let block = self
            .block_state(chunk.dimension, position)?
            .ok_or_else(|| {
                BedrockWorldError::Validation("item frame block is missing".to_string())
            })?;
        if block.name != "minecraft:frame"
            || !matches!(
                block.states.get("facing_direction"),
                Some(NbtTag::Int(0..=5))
            )
            || !matches!(
                block.states.get("item_frame_map_bit"),
                Some(NbtTag::Byte(0))
            )
        {
            return Err(BedrockWorldError::Validation(
                "target is not an empty vanilla item frame block".to_string(),
            ));
        }
        let (records, source) = self.block_entities_snapshot(chunk)?;
        let mut entities = records
            .into_iter()
            .map(|record| record.entity)
            .collect::<Vec<_>>();
        let target = entities
            .iter_mut()
            .find(|entity| entity.position == Some([position.x, position.y, position.z]))
            .ok_or_else(|| {
                BedrockWorldError::Validation("item frame BlockEntity is missing".to_string())
            })?;
        if target.id.as_deref() != Some("ItemFrame") {
            return Err(BedrockWorldError::Validation(
                "target BlockEntity is not ItemFrame".to_string(),
            ));
        }
        let NbtTag::Compound(fields) = &mut target.nbt else {
            return Err(BedrockWorldError::CorruptWorld(
                "item frame BlockEntity is not a compound".to_string(),
            ));
        };
        if fields.contains_key("Item") {
            return Err(BedrockWorldError::Validation(
                "item frame already contains an item".to_string(),
            ));
        }
        let (bundle, ids) = self.remap_bundle(bundle)?;
        let NbtTag::Compound(mut item) = bundle.flat_items()?[0].clone() else {
            return Err(BedrockWorldError::Validation(
                "map bundle contains a non-compound item".to_string(),
            ));
        };
        item.swap_remove("Slot");
        fields.insert("Item".to_string(), NbtTag::Compound(item));
        let mut shown_block = block.clone();
        shown_block
            .states
            .insert("item_frame_map_bit".to_string(), NbtTag::Byte(1));
        let guard = WriteGuard::confirmed(
            self.path().to_path_buf(),
            "insert filled map into item frame",
        );
        let block_edit = plan_block_edits(
            self,
            &[BlockEdit::new(chunk.dimension, position, shown_block)],
            &[BlockStateCondition::new(chunk.dimension, position, block)],
            &guard,
            BlockEditOptions::default(),
        )?
        .ok_or_else(|| {
            BedrockWorldError::ConcurrentWrite("item frame block changed".to_string())
        })?;
        Ok(MapFrameImportPlan {
            bundle,
            entity_updates: vec![MapFrameEntityUpdate {
                chunk,
                source,
                entities,
            }],
            block_edit,
            map_ids: ids,
        })
    }

    /// Prepares insertion of a flat BMCBL map bundle into one existing chest or shulker box.
    ///
    /// The block and its BlockEntity must agree with the requested position and type. One map
    /// occupies one slot; multiple maps in a chest are grouped in undyed shulker items of up to
    /// 27 maps each. An existing shulker item in the chest or local player's inventory supplies
    /// the target world's `Block` NBT template. A physical shulker box receives maps directly.
    /// Existing `Items` and unknown entity fields are preserved; only free slots `0..27` are
    /// used. The exact source BlockEntity bytes and proposed map IDs are checked at commit.
    /// This method reads but does not write LevelDB.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing/mismatched block or entity, occupied/duplicate/malformed
    /// slots, insufficient capacity, invalid map bundle, or storage/parse failure.
    pub fn prepare_map_bundle_container(
        &self,
        bundle: &MapBundle,
        chunk: ChunkPos,
        position: BlockPos,
    ) -> Result<MapBlockContainerImportPlan> {
        if position.to_chunk_pos(chunk.dimension) != chunk {
            return Err(BedrockWorldError::Validation(
                "container position is outside the selected chunk".to_string(),
            ));
        }
        let block = self
            .block_state(chunk.dimension, position)?
            .ok_or_else(|| {
                BedrockWorldError::Validation("container block is missing".to_string())
            })?;
        let (records, source) = self.block_entities_snapshot(chunk)?;
        let mut entities = records
            .into_iter()
            .map(|record| record.entity)
            .collect::<Vec<_>>();
        let target = entities
            .iter_mut()
            .find(|entity| entity.position == Some([position.x, position.y, position.z]))
            .ok_or_else(|| {
                BedrockWorldError::Validation("container BlockEntity is missing".to_string())
            })?;
        let valid = matches!(
            (target.id.as_deref(), block.name.as_str()),
            (Some("Chest"), "minecraft:chest")
                | (Some("ShulkerBox"), "minecraft:undyed_shulker_box")
        ) || target.id.as_deref() == Some("ShulkerBox")
            && block.name.ends_with("_shulker_box");
        if !valid {
            return Err(BedrockWorldError::Validation(
                "target block is not a matching chest or shulker box".to_string(),
            ));
        }
        let items = bundle.flat_items()?;
        let NbtTag::Compound(fields) = &mut target.nbt else {
            return Err(BedrockWorldError::CorruptWorld(
                "container BlockEntity is not a compound".to_string(),
            ));
        };
        let Some(NbtTag::List(existing)) = fields.get_mut("Items") else {
            return Err(BedrockWorldError::CorruptWorld(
                "container has no Items list".to_string(),
            ));
        };
        let mut occupied = BTreeSet::new();
        for item in existing.iter() {
            let NbtTag::Compound(item) = item else {
                return Err(BedrockWorldError::CorruptWorld(
                    "container Items contains a non-compound".to_string(),
                ));
            };
            let Some(NbtTag::Byte(slot)) = item.get("Slot") else {
                return Err(BedrockWorldError::CorruptWorld(
                    "container item has no Slot byte".to_string(),
                ));
            };
            if !(0..SHULKER_SLOTS as i8).contains(slot) || !occupied.insert(*slot) {
                return Err(BedrockWorldError::CorruptWorld(
                    "container contains an invalid or duplicate slot".to_string(),
                ));
            }
        }
        let group_in_chest = target.id.as_deref() == Some("Chest") && items.len() > 1;
        let needed = if group_in_chest {
            items.len().div_ceil(SHULKER_SLOTS)
        } else {
            items.len()
        };
        let shulker_block = if group_in_chest {
            let in_chest = existing.iter().find_map(|item| match item {
                NbtTag::Compound(fields) => shulker_block_template(fields),
                _ => None,
            });
            if in_chest.is_some() {
                in_chest
            } else if let Some(player) = self.player(&PlayerId::Local)? {
                player
                    .inventory()?
                    .iter()
                    .find_map(|entry| shulker_block_template(entry.nbt))
            } else {
                None
            }
        } else {
            None
        };
        if group_in_chest && shulker_block.is_none() {
            return Err(BedrockWorldError::Validation(
                "multi-map chest import needs an existing undyed shulker item template".to_string(),
            ));
        }
        let slots = (0..SHULKER_SLOTS as i8)
            .filter(|slot| !occupied.contains(slot))
            .take(needed)
            .collect::<Vec<_>>();
        if slots.len() != needed {
            return Err(BedrockWorldError::Validation(
                "container has too few empty slots for this map bundle".to_string(),
            ));
        }
        let (bundle, map_ids) = self.remap_bundle(bundle)?;
        if let Some(block) = shulker_block {
            for (index, (group, slot)) in bundle
                .flat_items()?
                .chunks(SHULKER_SLOTS)
                .zip(&slots)
                .enumerate()
            {
                existing.push(shulker_item(group, &block, *slot, index, needed)?);
            }
        } else {
            for (item, slot) in bundle.flat_items()?.iter().zip(&slots) {
                let NbtTag::Compound(mut item) = item.clone() else {
                    return Err(BedrockWorldError::Validation(
                        "map bundle contains a non-compound item".to_string(),
                    ));
                };
                item.insert("Slot".to_string(), NbtTag::Byte(*slot));
                existing.push(NbtTag::Compound(item));
            }
        }
        Ok(MapBlockContainerImportPlan {
            bundle,
            chunk,
            source,
            entities,
            map_ids,
            slots,
        })
    }

    /// Prepares a BMCBL map bundle for the selected LevelDB player's free inventory slots.
    ///
    /// One map is placed directly. Multiple maps are split into groups of at most 27 inside
    /// undyed shulker-box items, using an existing undyed box in this player's inventory as the
    /// target world's `Block` NBT template. The template's contents are never copied or changed.
    /// Empty slots are recognised by the observed Bedrock `Name=""`, `Count=0` item records in
    /// slots `0..36`; occupied slots are never overwritten. Map IDs are remapped for this world.
    /// This reads but does not write LevelDB and does not alter `level.dat`.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing/non-LevelDB player, invalid bundle, insufficient empty
    /// slots, missing shulker template for multiple maps, or storage/parse failures.
    pub fn prepare_map_bundle_player(
        &self,
        bundle: &MapBundle,
        player_id: &PlayerId,
    ) -> Result<MapPlayerImportPlan> {
        if player_id.storage_key().is_none() {
            return Err(BedrockWorldError::Validation(
                "map bundle requires a LevelDB-backed player".to_string(),
            ));
        }
        let source_items = bundle.flat_items()?;
        let mut player = self.player(player_id)?.ok_or_else(|| {
            BedrockWorldError::Validation("selected player record is missing".to_string())
        })?;
        let inventory = player.inventory()?;
        let mut occupied = BTreeSet::new();
        let mut shulker_block = None;
        for entry in &inventory {
            let Some(slot) = entry.slot else {
                return Err(BedrockWorldError::CorruptWorld(
                    "player Inventory item has no Slot".to_string(),
                ));
            };
            if !PLAYER_SLOTS.contains(&slot.raw()) {
                continue;
            }
            if !empty_inventory_item(entry.nbt) {
                occupied.insert(slot.raw());
            }
            shulker_block = shulker_block_template(entry.nbt).or(shulker_block);
        }
        let required = if source_items.len() == 1 {
            1
        } else {
            source_items.len().div_ceil(SHULKER_SLOTS)
        };
        let slots = PLAYER_SLOTS
            .filter(|slot| !occupied.contains(slot))
            .take(required)
            .map(PlayerInventorySlot::from_raw)
            .collect::<Vec<_>>();
        if slots.len() != required {
            return Err(BedrockWorldError::Validation(format!(
                "player inventory needs {required} empty slots"
            )));
        }
        if source_items.len() > 1 && shulker_block.is_none() {
            return Err(BedrockWorldError::Validation(
                "multi-map import needs an undyed shulker box item as a Block NBT template"
                    .to_string(),
            ));
        }
        let (bundle, map_ids) = self.remap_bundle(bundle)?;
        let items = bundle.flat_items()?;
        if items.len() == 1 {
            player.set_inventory_item(slots[0], items[0].clone())?;
        } else {
            let block = shulker_block.expect("multi-map import requires a checked template");
            for (index, (group, slot)) in items
                .chunks(SHULKER_SLOTS)
                .zip(slots.iter().copied())
                .enumerate()
            {
                let box_item = shulker_item(group, &block, slot.raw(), index, required)?;
                player.set_inventory_item(slot, box_item)?;
            }
        }
        Ok(MapPlayerImportPlan {
            bundle,
            player,
            map_ids,
            slots,
        })
    }

    fn remap_bundle(&self, bundle: &MapBundle) -> Result<(MapBundle, Vec<i64>)> {
        let map_ids = self.available_map_ids(bundle.records().len())?;
        let mapping = bundle
            .records()
            .iter()
            .zip(&map_ids)
            .map(|(record, id)| {
                record
                    .id
                    .as_str()
                    .parse::<i64>()
                    .map(|old| (old, *id))
                    .map_err(|_| BedrockWorldError::Validation("non-numeric map id".to_string()))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        Ok((bundle.remap(&mapping)?, map_ids))
    }
}

fn frame_grid_dimensions(bundle: &MapBundle, item_count: usize) -> Result<(u32, u32)> {
    let grid = match bundle.grid() {
        Some(grid) => grid,
        None if item_count == 1 => (1, 1),
        None => {
            return Err(BedrockWorldError::Validation(
                "multi-map frame placement requires verified grid dimensions".to_string(),
            ));
        }
    };
    if !(1..=64).contains(&grid.0)
        || !(1..=64).contains(&grid.1)
        || u64::from(grid.0) * u64::from(grid.1) != item_count as u64
    {
        return Err(BedrockWorldError::Validation(
            "frame grid dimensions do not match the map count".to_string(),
        ));
    }
    Ok(grid)
}

fn validate_upward_frame_support(support: &BlockState) -> Result<()> {
    if !support.name.starts_with("minecraft:")
        || matches!(support.name.as_str(), "minecraft:air" | "minecraft:frame")
    {
        return Err(BedrockWorldError::Validation(
            "support must be a non-air vanilla block".to_string(),
        ));
    }
    if support.version != Some(VERIFIED_UP_FRAME_VERSION) {
        return Err(BedrockWorldError::Validation(
            "upward frame creation is verified only for block version 18168865".to_string(),
        ));
    }
    Ok(())
}

fn frame_grid_positions(center: BlockPos, columns: u32, rows: u32) -> Result<Vec<BlockPos>> {
    let origin_x = center.x.checked_sub((columns / 2) as i32).ok_or_else(|| {
        BedrockWorldError::Validation("frame grid X is outside world coordinates".to_string())
    })?;
    let origin_z = center.z.checked_sub((rows / 2) as i32).ok_or_else(|| {
        BedrockWorldError::Validation("frame grid Z is outside world coordinates".to_string())
    })?;
    let mut positions = Vec::with_capacity((columns * rows) as usize);
    for row in 0..rows {
        for column in 0..columns {
            positions.push(BlockPos {
                x: origin_x.checked_add(column as i32).ok_or_else(|| {
                    BedrockWorldError::Validation(
                        "frame grid X is outside world coordinates".to_string(),
                    )
                })?,
                y: center.y,
                z: origin_z.checked_add(row as i32).ok_or_else(|| {
                    BedrockWorldError::Validation(
                        "frame grid Z is outside world coordinates".to_string(),
                    )
                })?,
            });
        }
    }
    Ok(positions)
}

fn upward_frame_state() -> BlockState {
    BlockState {
        name: "minecraft:frame".to_string(),
        states: BTreeMap::from([
            ("facing_direction".to_string(), NbtTag::Int(1)),
            ("item_frame_map_bit".to_string(), NbtTag::Byte(1)),
            ("item_frame_photo_bit".to_string(), NbtTag::Byte(0)),
        ]),
        version: Some(VERIFIED_UP_FRAME_VERSION),
    }
}

fn upward_frame_entity(item: &NbtTag, position: BlockPos) -> Result<BlockEntity> {
    let NbtTag::Compound(mut item) = item.clone() else {
        return Err(BedrockWorldError::Validation(
            "map bundle contains a non-compound item".to_string(),
        ));
    };
    item.swap_remove("Slot");
    Ok(BlockEntity {
        id: Some("ItemFrame".to_string()),
        position: Some([position.x, position.y, position.z]),
        is_movable: None,
        custom_name: None,
        items: Vec::new(),
        nbt: NbtTag::Compound(IndexMap::from([
            ("BlockEntityVersion".to_string(), NbtTag::Int(0)),
            ("Item".to_string(), NbtTag::Compound(item)),
            ("ItemDropChance".to_string(), NbtTag::Float(1.0)),
            ("ItemRotation".to_string(), NbtTag::Float(0.0)),
            ("id".to_string(), NbtTag::String("ItemFrame".to_string())),
            ("x".to_string(), NbtTag::Int(position.x)),
            ("y".to_string(), NbtTag::Int(position.y)),
            ("z".to_string(), NbtTag::Int(position.z)),
        ])),
    })
}

fn empty_inventory_item(fields: &IndexMap<String, NbtTag>) -> bool {
    matches!(fields.get("Name"), Some(NbtTag::String(name)) if name.is_empty())
        && matches!(fields.get("Count"), Some(NbtTag::Byte(0)))
}

fn shulker_block_template(fields: &IndexMap<String, NbtTag>) -> Option<NbtTag> {
    if !matches!(fields.get("Name"), Some(NbtTag::String(name)) if name == "minecraft:undyed_shulker_box")
    {
        return None;
    }
    let Some(NbtTag::Compound(block)) = fields.get("Block") else {
        return None;
    };
    if !matches!(block.get("name"), Some(NbtTag::String(name)) if name == "minecraft:undyed_shulker_box")
        || !matches!(block.get("states"), Some(NbtTag::Compound(_)))
        || !matches!(block.get("version"), Some(NbtTag::Int(_)))
    {
        return None;
    }
    fields.get("Block").cloned()
}

fn shulker_item(
    group: &[NbtTag],
    block: &NbtTag,
    slot: i8,
    index: usize,
    total: usize,
) -> Result<NbtTag> {
    let mut contents = Vec::with_capacity(group.len());
    for (item_slot, item) in group.iter().enumerate() {
        let NbtTag::Compound(mut fields) = item.clone() else {
            return Err(BedrockWorldError::Validation(
                "map bundle contains a non-compound item".to_string(),
            ));
        };
        fields.insert("Slot".to_string(), NbtTag::Byte(item_slot as i8));
        contents.push(NbtTag::Compound(fields));
    }
    let mut box_item = NbtTag::Compound(IndexMap::from([
        ("Block".to_string(), block.clone()),
        ("Count".to_string(), NbtTag::Byte(1)),
        ("Damage".to_string(), NbtTag::Short(0)),
        (
            "Name".to_string(),
            NbtTag::String("minecraft:undyed_shulker_box".to_string()),
        ),
        ("Slot".to_string(), NbtTag::Byte(slot)),
        ("WasPickedUp".to_string(), NbtTag::Byte(0)),
        (
            "tag".to_string(),
            NbtTag::Compound(IndexMap::from([(
                "Items".to_string(),
                NbtTag::List(contents),
            )])),
        ),
    ]));
    set_item_display(
        &mut box_item,
        &format!("BMCBL 地图包 {}/{}", index + 1, total),
        &[format!("北向上 · {} 张地图", group.len())],
    )?;
    Ok(box_item)
}
