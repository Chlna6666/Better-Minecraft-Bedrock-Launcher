//! Minecraft global, map, village, block-entity, and actor scans and mutations.

use super::*;

impl<S> World<S>
where
    S: StorageBackend,
{
    /// Scan entities.
    pub fn scan_entities(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<Actor>, ScanReport)> {
        let mut report = ScanReport::default();
        let mut entities = Vec::new();
        let mut entries_seen = 0usize;
        self.storage()
            .for_each_entry(to_storage_read_options(&options), &mut |key, value| {
                check_cancelled(&options)?;
                entries_seen = entries_seen.saturating_add(1);
                match BedrockDbKey::decode(key) {
                    BedrockDbKey::ActorPrefix { .. } => {
                        entities.extend(parse_entities_from_value(value, &mut report));
                    }
                    BedrockDbKey::Chunk(chunk_key) if chunk_key.tag == ChunkRecordTag::Entity => {
                        entities.extend(parse_entities_from_value(value, &mut report));
                    }
                    _ => {}
                }
                if entries_seen.is_multiple_of(8192) {
                    emit_progress(&options, entries_seen);
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok((entities, report))
    }

    /// Scan block entities.
    pub fn scan_block_entities(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<BlockEntity>, ScanReport)> {
        let mut report = ScanReport::default();
        let mut block_entities = Vec::new();
        let mut entries_seen = 0usize;
        self.storage()
            .for_each_entry(to_storage_read_options(&options), &mut |key, value| {
                check_cancelled(&options)?;
                entries_seen = entries_seen.saturating_add(1);
                if let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) {
                    if chunk_key.tag == ChunkRecordTag::BlockEntity {
                        block_entities.extend(parse_block_entities_from_value(value, &mut report));
                    }
                }
                if entries_seen.is_multiple_of(8192) {
                    emit_progress(&options, entries_seen);
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok((block_entities, report))
    }

    /// Scan items.
    pub fn scan_items(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<ItemStack>, ScanReport)> {
        let mut report = ScanReport::default();
        let mut items = Vec::new();
        let mut entries_seen = 0usize;
        self.storage()
            .for_each_entry(to_storage_read_options(&options), &mut |key, value| {
                check_cancelled(&options)?;
                entries_seen = entries_seen.saturating_add(1);
                match BedrockDbKey::decode(key) {
                    BedrockDbKey::LocalPlayer | BedrockDbKey::RemotePlayer(_) => {
                        match parse_root_nbt(value) {
                            Ok(nbt) => {
                                let mut player_items = collect_item_stacks(&nbt);
                                report.item_count =
                                    report.item_count.saturating_add(player_items.len());
                                items.append(&mut player_items);
                            }
                            Err(error) => report
                                .parse_errors
                                .push(format!("player item scan failed: {error}")),
                        }
                    }
                    BedrockDbKey::ActorPrefix { .. } => {
                        for entity in parse_entities_from_value(value, &mut report) {
                            items.extend(entity.items);
                        }
                    }
                    BedrockDbKey::Chunk(chunk_key) if chunk_key.tag == ChunkRecordTag::Entity => {
                        for entity in parse_entities_from_value(value, &mut report) {
                            items.extend(entity.items);
                        }
                    }
                    BedrockDbKey::Chunk(chunk_key)
                        if chunk_key.tag == ChunkRecordTag::BlockEntity =>
                    {
                        for block_entity in parse_block_entities_from_value(value, &mut report) {
                            items.extend(block_entity.items);
                        }
                    }
                    _ => {}
                }
                if entries_seen.is_multiple_of(8192) {
                    emit_progress(&options, entries_seen);
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok((items, report))
    }

    /// Reads one Bedrock map item by its exact `map_<id>` key.
    ///
    /// # Errors
    ///
    /// Returns storage errors or map NBT parse errors.
    pub fn map_item(&self, id: &MapItemId) -> Result<Option<SavedData>> {
        self.storage()
            .get(&id.storage_key())?
            .map(|value| decode_map_item(id.clone(), value))
            .transpose()
    }

    /// Reads Bedrock map items without scanning unrelated globals.
    ///
    /// # Errors
    ///
    /// Returns storage errors, cancellation, or map NBT parse errors.
    pub fn map_items(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<SavedData>> {
        let mut items = Vec::new();
        self.storage().for_each_prefix_ref(
            b"map_",
            to_storage_read_options(&options),
            &mut |entry| {
                check_cancelled(&options)?;
                let Some(id) = MapItemId::from_storage_key(entry.key) else {
                    return Ok(StorageVisitorControl::Continue);
                };
                items.push(decode_map_item(id, Bytes::copy_from_slice(entry.value))?);
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        Ok(items)
    }

    /// Saves a Bedrock map item after an encode/decode round-trip validation.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors for malformed map items, or storage errors from the commit.
    pub fn save_map_item(&self, item: &SavedData) -> Result<()> {
        self.ensure_writable()?;
        let value = encode_map_item(item)?;
        decode_map_item(item.id.clone(), value.clone())?;
        let mut transaction = self.transaction();
        transaction.put_raw_key(item.id.storage_key(), value);
        transaction.commit()
    }

    /// Deletes a Bedrock map item by its exact id.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds or storage
    /// errors from the commit.
    pub fn delete_map_item(&self, id: &MapItemId) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.delete_raw_key(id.storage_key());
        transaction.commit()
    }

    /// Reads Bedrock village data stored under `VILLAGE_*` keys.
    ///
    /// # Errors
    ///
    /// Returns storage, cancellation, or NBT decoding errors.
    pub fn villages(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<Entry>> {
        let mut villages = Vec::new();
        self.storage()
            .for_each_prefix_ref(b"VILLAGE_", to_storage_read_options(&options), &mut |entry| {
                check_cancelled(&options)?;
                let BedrockDbKey::Village(key) = BedrockDbKey::decode(entry.key) else {
                    return Ok(StorageVisitorControl::Continue);
                };
                let roots = parse_consecutive_root_nbt(entry.value).unwrap_or_default();
                villages.push(Entry {
                    key,
                    roots,
                    raw: Bytes::new(),
                });
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok(villages)
    }

    /// Reads a single typed global record by exact key.
    ///
    /// # Errors
    ///
    /// Returns storage errors or global NBT parse errors.
    pub fn global(
        &self,
        kind: GlobalRecordKind,
    ) -> Result<Option<Global>> {
        let key = kind.storage_key();
        self.storage()
            .get(&key)?
            .map(|value| decode_global(kind.clone(), kind.name(), value))
            .transpose()
    }

    /// Scans known global records while preserving each typed key kind.
    ///
    /// # Errors
    ///
    /// Returns storage errors, cancellation, or global NBT parse errors.
    pub fn globals(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<Global>> {
        let mut records = Vec::new();
        self.storage()
            .for_each_entry(to_storage_read_options(&options), &mut |key, value| {
                check_cancelled(&options)?;
                let BedrockDbKey::Global(kind) = BedrockDbKey::decode(key) else {
                    return Ok(StorageVisitorControl::Continue);
                };
                records.push(decode_global(
                    kind.clone(),
                    kind.name(),
                    value.clone(),
                )?);
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok(records)
    }

    /// Writes a global record after serialize -> parse roundtrip validation.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors for malformed records, or storage errors from the commit.
    pub fn save_global(&self, record: &Global) -> Result<()> {
        self.ensure_writable()?;
        let value = encode_global(record)?;
        decode_global(record.kind.clone(), record.name.clone(), value.clone())?;
        let mut transaction = self.transaction();
        transaction.put_raw_key(record.kind.storage_key(), value);
        transaction.commit()
    }

    /// Deletes a typed global record.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds or storage
    /// errors from the commit.
    pub fn delete_global(&self, kind: GlobalRecordKind) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.delete_raw_key(kind.storage_key());
        transaction.commit()
    }

    /// Scans hardcoded spawn area records across the world.
    ///
    /// # Errors
    ///
    /// Returns storage errors, cancellation, or HSA payload validation errors.
    pub fn hardcoded_spawn_areas(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<(ChunkPos, Vec<HardcodedSpawnArea>)>> {
        let mut records = Vec::new();
        self.storage()
            .for_each_entry(to_storage_read_options(&options), &mut |key, value| {
                check_cancelled(&options)?;
                let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) else {
                    return Ok(StorageVisitorControl::Continue);
                };
                if chunk_key.tag == ChunkRecordTag::HardcodedSpawners {
                    records.push((chunk_key.pos, decode_hardcoded_spawn_areas(value)?));
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok(records)
    }

    /// Writes hardcoded spawn areas for one chunk.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors for invalid bounds/lengths, or storage errors.
    pub fn save_hardcoded_spawn_areas(
        &self,
        pos: ChunkPos,
        areas: &[HardcodedSpawnArea],
    ) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.save_hardcoded_spawn_areas(pos, areas)?;
        transaction.commit()
    }

    /// Deletes hardcoded spawn areas for one chunk.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds or storage
    /// errors.
    pub fn delete_hardcoded_spawn_areas(&self, pos: ChunkPos) -> Result<()> {
        self.delete_raw(&ChunkKey::new(pos, ChunkRecordTag::HardcodedSpawners))
    }

    /// Reads all block entities from a chunk's consecutive NBT payload.
    ///
    /// # Errors
    ///
    /// Returns storage errors or block-entity NBT parse errors.
    pub fn block_entities(
        &self,
        pos: ChunkPos,
    ) -> Result<Vec<BlockEntityRecord>> {
        let key = ChunkKey::new(pos, ChunkRecordTag::BlockEntity).encode();
        let Some(value) = self.storage().get(&key)? else {
            return Ok(Vec::new());
        };
        let mut report = ScanReport::default();
        Ok(parse_block_entities_from_value(&value, &mut report)
            .into_iter()
            .enumerate()
            .map(|(index, entity)| BlockEntityRecord {
                chunk: pos,
                index,
                entity,
            })
            .collect())
    }

    /// Replaces a chunk's block entity payload after coordinate validation.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors when entity coordinates do not belong to `pos`, or storage errors.
    pub fn put_block_entities(
        &self,
        pos: ChunkPos,
        entities: &[BlockEntity],
    ) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.put_block_entities(pos, entities)?;
        transaction.commit()
    }

    /// Edits one block entity in place and rewrites the chunk payload.
    ///
    /// # Errors
    ///
    /// Returns validation errors when no block entity exists at `block`, when
    /// the edited NBT no longer parses as a block entity, or storage/read-only
    /// errors from the write.
    pub fn edit_block_entity_at<F>(
        &self,
        pos: ChunkPos,
        block: BlockPos,
        edit: F,
    ) -> Result<()>
    where
        F: FnOnce(&mut NbtTag) -> Result<()>,
    {
        self.ensure_writable()?;
        let mut entities = self
            .block_entities(pos)?
            .into_iter()
            .map(|record| record.entity)
            .collect::<Vec<_>>();
        let Some(index) = entities
            .iter()
            .position(|entity| entity.position == Some([block.x, block.y, block.z]))
        else {
            return Err(BedrockWorldError::Validation(format!(
                "no block entity exists at {},{},{}",
                block.x, block.y, block.z
            )));
        };
        edit(&mut entities[index].nbt)?;
        let mut report = ScanReport::default();
        entities[index] = parse_block_entities_from_value(
            &Bytes::from(serialize_root_nbt(&entities[index].nbt)?),
            &mut report,
        )
        .into_iter()
        .next()
        .ok_or_else(|| BedrockWorldError::Validation("edited block entity vanished".to_string()))?;
        self.put_block_entities(pos, &entities)
    }

    /// Deletes one block entity by absolute block position.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds or storage
    /// errors from rewriting/deleting the payload.
    pub fn delete_block_entity_at(&self, pos: ChunkPos, block: BlockPos) -> Result<()> {
        self.ensure_writable()?;
        let entities = self
            .block_entities(pos)?
            .into_iter()
            .map(|record| record.entity)
            .filter(|entity| entity.position != Some([block.x, block.y, block.z]))
            .collect::<Vec<_>>();
        if entities.is_empty() {
            return self
                .delete_raw(&ChunkKey::new(pos, ChunkRecordTag::BlockEntity));
        }
        self.put_block_entities(pos, &entities)
    }

    /// Reads actors from both legacy inline `Entity` and modern digest/prefix storage.
    ///
    /// # Errors
    ///
    /// Returns storage errors or digest validation errors.
    pub fn actors(&self, pos: ChunkPos) -> Result<Vec<ActorRecord>> {
        let mut records = Vec::new();
        let inline_key = ChunkKey::new(pos, ChunkRecordTag::Entity);
        if let Some(value) = self.storage().get(&inline_key.encode())? {
            let mut report = ScanReport::default();
            records.extend(
                parse_entities_from_value(&value, &mut report)
                    .into_iter()
                    .map(|entity| ActorRecord {
                        uid: entity.unique_id.map(ActorUid),
                        source: ActorSource::InlineChunk(inline_key.clone()),
                        entity,
                        raw: value.clone(),
                    }),
            );
        }
        let digest_key = ActorDigestKey::new(pos).storage_key();
        let Some(digest) = self.storage().get(&digest_key)? else {
            return Ok(records);
        };
        let ids = decode_actor_ids(&digest)?;
        let actor_keys = ids.iter().map(|id| id.storage_key()).collect::<Vec<_>>();
        let values = self.storage().get_many(&actor_keys)?;
        for (id, value) in ids.into_iter().zip(values) {
            let Some(value) = value else {
                continue;
            };
            let mut report = ScanReport::default();
            records.extend(
                parse_entities_from_value(&value, &mut report)
                    .into_iter()
                    .map(|entity| ActorRecord {
                        uid: Some(id),
                        source: ActorSource::ActorPrefix(id),
                        entity,
                        raw: value.clone(),
                    }),
            );
        }
        Ok(records)
    }

    /// Writes a modern actor record and updates the chunk actor digest.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors when `actor` has no `UniqueID`, or storage errors from the commit.
    pub fn put_actor(&self, pos: ChunkPos, actor: &Actor) -> Result<()> {
        self.ensure_writable()?;
        let uid = actor
            .unique_id
            .map(ActorUid::from_unique_id)
            .ok_or_else(|| {
                BedrockWorldError::Validation("actor UniqueID is required".to_string())
            })?;
        let value = Bytes::from(serialize_root_nbt(&actor.nbt)?);
        parse_entities_from_value(&value, &mut ScanReport::default());
        let mut transaction = self.transaction();
        transaction.put_actor(pos, uid, value)?;
        transaction.commit()
    }

    /// Replaces one actor NBT document selected by its NBT `UniqueID`.
    ///
    /// Modern actors preserve the exact storage token read from `digp`; changing `UniqueID`
    /// in the edited document is rejected because that would require a new entity identity.
    pub fn edit_actor_nbt_by_unique_id(
        &self,
        pos: ChunkPos,
        unique_id: i64,
        nbt: NbtTag,
    ) -> Result<BTreeSet<ChunkPos>> {
        self.ensure_writable()?;
        let records = self.actors(pos)?;
        let source = records
            .iter()
            .find(|record| record.entity.unique_id == Some(unique_id))
            .map(|record| record.source.clone())
            .ok_or_else(|| {
                BedrockWorldError::Validation(format!("actor UniqueID {unique_id} does not exist"))
            })?;
        let value = Bytes::from(serialize_root_nbt(&nbt)?);
        let mut report = ScanReport::default();
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
                    BedrockWorldError::Validation(
                        "legacy inline actor record disappeared".to_string(),
                    )
                })?;
                let mut inline_report = ScanReport::default();
                let mut actors = parse_entities_from_value(&raw, &mut inline_report);
                let actor = actors
                    .iter_mut()
                    .find(|actor| actor.unique_id == Some(unique_id))
                    .ok_or_else(|| {
                        BedrockWorldError::Validation("legacy inline actor disappeared".to_string())
                    })?;
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
    pub fn delete_actor_by_unique_id(&self, pos: ChunkPos, unique_id: i64) -> Result<()> {
        self.ensure_writable()?;
        let records = self.actors(pos)?;
        let source = records
            .iter()
            .find(|record| record.entity.unique_id == Some(unique_id))
            .map(|record| record.source.clone())
            .ok_or_else(|| {
                BedrockWorldError::Validation(format!("actor UniqueID {unique_id} does not exist"))
            })?;
        match source {
            ActorSource::ActorPrefix(storage_uid) => self.delete_actor(pos, storage_uid),
            ActorSource::InlineChunk(inline_key) => {
                let raw = self.storage().get(&inline_key.encode())?.ok_or_else(|| {
                    BedrockWorldError::Validation(
                        "legacy inline actor record disappeared".to_string(),
                    )
                })?;
                let mut report = ScanReport::default();
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
                    return Err(BedrockWorldError::Validation(
                        "legacy inline actor disappeared".to_string(),
                    ));
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

    /// Deletes a modern actor record and removes it from the chunk digest.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds or storage
    /// errors from the commit.
    pub fn delete_actor(&self, pos: ChunkPos, uid: ActorUid) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.delete_actor(pos, uid)?;
        transaction.commit()
    }

    /// Moves a modern actor between chunk digests and rewrites its actorprefix payload.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors when `actor` has no `UniqueID`, or storage errors from the commit.
    pub fn move_actor(
        &self,
        from: ChunkPos,
        to: ChunkPos,
        actor: &Actor,
    ) -> Result<()> {
        self.ensure_writable()?;
        let uid = actor
            .unique_id
            .map(ActorUid::from_unique_id)
            .ok_or_else(|| {
                BedrockWorldError::Validation("actor UniqueID is required".to_string())
            })?;
        let value = Bytes::from(serialize_root_nbt(&actor.nbt)?);
        let mut transaction = self.transaction();
        transaction.delete_actor(from, uid)?;
        transaction.put_actor(to, uid, value)?;
        transaction.commit()
    }

    // Async wrappers are implemented in the feature-gated sync_world module.
}
