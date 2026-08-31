//! Minecraft global records, maps, villages, block entities, and actor operations.

use super::*;

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Parse global data blocking.
    pub fn parse_global_data_blocking(&self) -> Result<Vec<ParsedDbEntry>> {
        parse_global_storage_entries(self.storage(), WorldParseOptions::summary())
    }

    /// Scan entities blocking.
    pub fn scan_entities_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<ParsedEntity>, WorldParseReport)> {
        let mut report = WorldParseReport::default();
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

    /// Scan block entities blocking.
    pub fn scan_block_entities_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<ParsedBlockEntity>, WorldParseReport)> {
        let mut report = WorldParseReport::default();
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

    /// Scan items blocking.
    pub fn scan_items_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<ItemStack>, WorldParseReport)> {
        let mut report = WorldParseReport::default();
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

    /// Scans map records through the full global-data parser.
    ///
    /// Prefer [`Self::scan_map_records_blocking`] when only `map_` records are
    /// needed because it uses an exact prefix scan.
    ///
    /// # Errors
    ///
    /// Returns storage or parse errors from the underlying world scan.
    pub fn scan_maps_blocking(&self) -> Result<Vec<ParsedMapData>> {
        Ok(self
            .parse_global_data_blocking()?
            .into_iter()
            .filter_map(|entry| match entry.value {
                ParsedDbValue::MapData(value) => Some(value),
                _ => None,
            })
            .collect())
    }

    /// Reads a single typed map record by exact `map_<id>` key.
    ///
    /// # Errors
    ///
    /// Returns storage errors or map NBT parse errors.
    pub fn read_map_record_blocking(&self, id: &MapRecordId) -> Result<Option<ParsedMapData>> {
        self.storage()
            .get(&id.storage_key())?
            .map(|value| parse_map_record(id.clone(), value))
            .transpose()
    }

    /// Prefix-scans typed map records without scanning unrelated globals.
    ///
    /// # Errors
    ///
    /// Returns storage errors, cancellation, or map NBT parse errors.
    pub fn scan_map_records_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ParsedMapData>> {
        let mut records = Vec::new();
        self.storage().for_each_prefix_ref(
            b"map_",
            to_storage_read_options(&options),
            &mut |entry| {
                check_cancelled(&options)?;
                let Some(id) = MapRecordId::from_storage_key(entry.key) else {
                    return Ok(StorageVisitorControl::Continue);
                };
                records.push(parse_map_record(id, Bytes::copy_from_slice(entry.value))?);
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        Ok(records)
    }

    /// Writes a map record after serialize -> parse roundtrip validation.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors for malformed records, or storage errors from the commit.
    pub fn write_map_record_blocking(&self, record: &ParsedMapData) -> Result<()> {
        self.ensure_writable()?;
        let value = encode_map_record(record)?;
        parse_map_record(record.record_id.clone(), value.clone())?;
        let mut transaction = self.transaction();
        transaction.put_raw_key(record.record_id.storage_key(), value);
        transaction.commit()
    }

    /// Deletes a map record by exact id.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds or storage
    /// errors from the commit.
    pub fn delete_map_record_blocking(&self, id: &MapRecordId) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.delete_raw_key(id.storage_key());
        transaction.commit()
    }

    /// Scans village records through the full global-data parser.
    ///
    /// # Errors
    ///
    /// Returns storage or parse errors from the underlying world scan.
    pub fn scan_villages_blocking(&self) -> Result<Vec<ParsedVillageData>> {
        Ok(self
            .parse_global_data_blocking()?
            .into_iter()
            .filter_map(|entry| match entry.value {
                ParsedDbValue::VillageData(value) => Some(value),
                _ => None,
            })
            .collect())
    }

    /// Scan villages lightweight blocking.
    pub fn scan_villages_lightweight_blocking(
        &self,
        cancel: &CancelFlag,
    ) -> Result<Vec<ParsedVillageData>> {
        let mut villages = Vec::new();
        let options = StorageReadOptions {
            cancel: Some(cancel.to_storage_cancel()),
            ..StorageReadOptions::default()
        };
        self.storage()
            .for_each_prefix_ref(b"VILLAGE_", options, &mut |entry| {
                if cancel.is_cancelled() {
                    return Err(BedrockWorldError::Cancelled {
                        operation: "village scan",
                    });
                }
                let BedrockDbKey::Village(key) = BedrockDbKey::decode(entry.key) else {
                    return Ok(StorageVisitorControl::Continue);
                };
                let roots = parse_consecutive_root_nbt(entry.value).unwrap_or_default();
                villages.push(ParsedVillageData {
                    key,
                    roots,
                    raw: Bytes::new(),
                });
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok(villages)
    }

    /// Scans global records through the full global-data parser.
    ///
    /// Prefer [`Self::scan_global_records_blocking`] when only typed global
    /// records are needed.
    ///
    /// # Errors
    ///
    /// Returns storage or parse errors from the underlying world scan.
    pub fn scan_globals_blocking(&self) -> Result<Vec<ParsedGlobalData>> {
        Ok(self
            .parse_global_data_blocking()?
            .into_iter()
            .filter_map(|entry| match entry.value {
                ParsedDbValue::GlobalData(value) => Some(value),
                _ => None,
            })
            .collect())
    }

    /// Reads a single typed global record by exact key.
    ///
    /// # Errors
    ///
    /// Returns storage errors or global NBT parse errors.
    pub fn read_global_record_blocking(
        &self,
        kind: GlobalRecordKind,
    ) -> Result<Option<ParsedGlobalData>> {
        let key = kind.storage_key();
        self.storage()
            .get(&key)?
            .map(|value| parse_global_record(kind.clone(), kind.name(), value))
            .transpose()
    }

    /// Scans known global records while preserving each typed key kind.
    ///
    /// # Errors
    ///
    /// Returns storage errors, cancellation, or global NBT parse errors.
    pub fn scan_global_records_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ParsedGlobalData>> {
        let mut records = Vec::new();
        self.storage()
            .for_each_entry(to_storage_read_options(&options), &mut |key, value| {
                check_cancelled(&options)?;
                let BedrockDbKey::Global(kind) = BedrockDbKey::decode(key) else {
                    return Ok(StorageVisitorControl::Continue);
                };
                records.push(parse_global_record(
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
    pub fn write_global_record_blocking(&self, record: &ParsedGlobalData) -> Result<()> {
        self.ensure_writable()?;
        let value = encode_global_record(record)?;
        parse_global_record(record.kind.clone(), record.name.clone(), value.clone())?;
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
    pub fn delete_global_record_blocking(&self, kind: GlobalRecordKind) -> Result<()> {
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
    pub fn scan_hsa_records_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<(ChunkPos, Vec<ParsedHardcodedSpawnArea>)>> {
        let mut records = Vec::new();
        self.storage()
            .for_each_entry(to_storage_read_options(&options), &mut |key, value| {
                check_cancelled(&options)?;
                let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) else {
                    return Ok(StorageVisitorControl::Continue);
                };
                if chunk_key.tag == ChunkRecordTag::HardcodedSpawners {
                    records.push((chunk_key.pos, parse_hardcoded_spawn_area_records(value)?));
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
    pub fn put_hsa_for_chunk_blocking(
        &self,
        pos: ChunkPos,
        areas: &[ParsedHardcodedSpawnArea],
    ) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.put_hsa_for_chunk(pos, areas)?;
        transaction.commit()
    }

    /// Deletes hardcoded spawn areas for one chunk.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds or storage
    /// errors.
    pub fn delete_hsa_for_chunk_blocking(&self, pos: ChunkPos) -> Result<()> {
        self.delete_raw_record_blocking(&ChunkKey::new(pos, ChunkRecordTag::HardcodedSpawners))
    }

    /// Reads all block entities from a chunk's consecutive NBT payload.
    ///
    /// # Errors
    ///
    /// Returns storage errors or block-entity NBT parse errors.
    pub fn block_entities_in_chunk_blocking(
        &self,
        pos: ChunkPos,
    ) -> Result<Vec<BlockEntityRecord>> {
        let key = ChunkKey::new(pos, ChunkRecordTag::BlockEntity).encode();
        let Some(value) = self.storage().get(&key)? else {
            return Ok(Vec::new());
        };
        let mut report = WorldParseReport::default();
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
    pub fn put_block_entities_blocking(
        &self,
        pos: ChunkPos,
        entities: &[ParsedBlockEntity],
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
    pub fn edit_block_entity_at_blocking<F>(
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
            .block_entities_in_chunk_blocking(pos)?
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
        let mut report = WorldParseReport::default();
        entities[index] = parse_block_entities_from_value(
            &Bytes::from(serialize_root_nbt(&entities[index].nbt)?),
            &mut report,
        )
        .into_iter()
        .next()
        .ok_or_else(|| BedrockWorldError::Validation("edited block entity vanished".to_string()))?;
        self.put_block_entities_blocking(pos, &entities)
    }

    /// Deletes one block entity by absolute block position.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds or storage
    /// errors from rewriting/deleting the payload.
    pub fn delete_block_entity_at_blocking(&self, pos: ChunkPos, block: BlockPos) -> Result<()> {
        self.ensure_writable()?;
        let entities = self
            .block_entities_in_chunk_blocking(pos)?
            .into_iter()
            .map(|record| record.entity)
            .filter(|entity| entity.position != Some([block.x, block.y, block.z]))
            .collect::<Vec<_>>();
        if entities.is_empty() {
            return self
                .delete_raw_record_blocking(&ChunkKey::new(pos, ChunkRecordTag::BlockEntity));
        }
        self.put_block_entities_blocking(pos, &entities)
    }

    /// Reads actors from both legacy inline `Entity` and modern digest/prefix storage.
    ///
    /// # Errors
    ///
    /// Returns storage errors or digest validation errors.
    pub fn actors_in_chunk_blocking(&self, pos: ChunkPos) -> Result<Vec<ActorRecord>> {
        let mut records = Vec::new();
        let inline_key = ChunkKey::new(pos, ChunkRecordTag::Entity);
        if let Some(value) = self.storage().get(&inline_key.encode())? {
            let mut report = WorldParseReport::default();
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
        let ids = parse_actor_digest_ids(&digest)?;
        let actor_keys = ids.iter().map(|id| id.storage_key()).collect::<Vec<_>>();
        let values = self.storage().get_many(&actor_keys)?;
        for (id, value) in ids.into_iter().zip(values) {
            let Some(value) = value else {
                continue;
            };
            let mut report = WorldParseReport::default();
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
    pub fn put_actor_blocking(&self, pos: ChunkPos, actor: &ParsedEntity) -> Result<()> {
        self.ensure_writable()?;
        let uid = actor
            .unique_id
            .map(ActorUid::from_unique_id)
            .ok_or_else(|| {
                BedrockWorldError::Validation("actor UniqueID is required".to_string())
            })?;
        let value = Bytes::from(serialize_root_nbt(&actor.nbt)?);
        parse_entities_from_value(&value, &mut WorldParseReport::default());
        let mut transaction = self.transaction();
        transaction.put_actor(pos, uid, value)?;
        transaction.commit()
    }

    /// Replaces one actor NBT document selected by its NBT `UniqueID`.
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
            .ok_or_else(|| {
                BedrockWorldError::Validation(format!("actor UniqueID {unique_id} does not exist"))
            })?;
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
                    BedrockWorldError::Validation(
                        "legacy inline actor record disappeared".to_string(),
                    )
                })?;
                let mut inline_report = WorldParseReport::default();
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
    pub fn delete_actor_by_unique_id_blocking(&self, pos: ChunkPos, unique_id: i64) -> Result<()> {
        self.ensure_writable()?;
        let records = self.actors_in_chunk_blocking(pos)?;
        let source = records
            .iter()
            .find(|record| record.entity.unique_id == Some(unique_id))
            .map(|record| record.source.clone())
            .ok_or_else(|| {
                BedrockWorldError::Validation(format!("actor UniqueID {unique_id} does not exist"))
            })?;
        match source {
            ActorSource::ActorPrefix(storage_uid) => self.delete_actor_blocking(pos, storage_uid),
            ActorSource::InlineChunk(inline_key) => {
                let raw = self.storage().get(&inline_key.encode())?.ok_or_else(|| {
                    BedrockWorldError::Validation(
                        "legacy inline actor record disappeared".to_string(),
                    )
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
    pub fn delete_actor_blocking(&self, pos: ChunkPos, uid: ActorUid) -> Result<()> {
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
    pub fn move_actor_blocking(
        &self,
        from: ChunkPos,
        to: ChunkPos,
        actor: &ParsedEntity,
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
