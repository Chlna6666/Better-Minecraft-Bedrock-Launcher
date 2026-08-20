//! Buffered Minecraft Bedrock world transaction operations.

use super::*;

/// Buffered atomic mutations for one Minecraft Bedrock world storage backend.
pub struct WorldTransaction<'a, S = Arc<dyn WorldStorage>>
where
    S: WorldStorageHandle,
{
    pub(super) storage: &'a S,
    pub(super) batch: StorageBatch,
    pub(super) read_only: bool,
    pub(super) actor_ownership: Option<ActorOwnershipIndex>,
}

impl<S> WorldTransaction<'_, S>
where
    S: WorldStorageHandle,
{
    /// Stages a raw chunk record write.
    pub fn put_raw_record(&mut self, key: &ChunkKey, value: impl Into<Bytes>) {
        self.batch.put(key.encode(), value.into());
    }

    /// Stages a raw chunk record delete.
    pub fn delete_raw_record(&mut self, key: &ChunkKey) {
        self.batch.delete(key.encode());
    }

    /// Stages a raw key/value write.
    pub fn put_raw_key(&mut self, key: impl Into<Bytes>, value: impl Into<Bytes>) {
        self.batch.put(key.into(), value.into());
    }

    /// Stages a raw key delete.
    pub fn delete_raw_key(&mut self, key: impl Into<Bytes>) {
        self.batch.delete(key.into());
    }

    /// Stages deletion of every raw record and modern actor owned by one chunk.
    ///
    /// # Errors
    ///
    /// Returns storage or actor-digest parse errors.
    pub fn delete_chunk(&mut self, pos: ChunkPos) -> Result<usize> {
        let mut raw_keys = Vec::new();
        self.storage.storage().for_each_prefix(
            &chunk_record_prefix(pos),
            StorageReadOptions::default(),
            &mut |raw_key, _| {
                if ChunkKey::decode(raw_key).is_ok_and(|key| key.pos == pos) {
                    raw_keys.push(Bytes::copy_from_slice(raw_key));
                }
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        let mut deleted = raw_keys.len();
        for raw_key in raw_keys {
            self.batch.delete(raw_key);
        }

        let actor_ids = self
            .actor_ownership()?
            .actors(pos)
            .cloned()
            .unwrap_or_default();
        self.replace_actor_digest(pos, Vec::clear)?;
        for actor_uid in actor_ids {
            if self
                .actor_ownership
                .as_ref()
                .is_some_and(|index| index.owner_count(actor_uid) == 0)
            {
                self.batch.delete(actor_uid.storage_key());
                deleted = deleted.saturating_add(1);
            }
        }
        Ok(deleted)
    }

    /// Stages a validated block-entity payload for one chunk.
    ///
    /// # Errors
    ///
    /// Returns validation or serialization errors.
    pub fn put_block_entities(
        &mut self,
        pos: ChunkPos,
        entities: &[ParsedBlockEntity],
    ) -> Result<()> {
        validate_block_entities_in_chunk(pos, entities)?;
        let roots = entities
            .iter()
            .map(|entity| entity.nbt.clone())
            .collect::<Vec<_>>();
        let value = encode_consecutive_roots(&roots)?;
        let mut report = WorldParseReport::default();
        let parsed = parse_block_entities_from_value(&value, &mut report);
        validate_block_entities_in_chunk(pos, &parsed)?;
        self.put_raw_record(&ChunkKey::new(pos, ChunkRecordTag::BlockEntity), value);
        Ok(())
    }

    /// Stages a validated hardcoded-spawn-area payload for one chunk.
    ///
    /// # Errors
    ///
    /// Returns validation or serialization errors.
    pub fn put_hsa_for_chunk(
        &mut self,
        pos: ChunkPos,
        areas: &[ParsedHardcodedSpawnArea],
    ) -> Result<()> {
        let value = encode_hardcoded_spawn_area_records(areas)?;
        parse_hardcoded_spawn_area_records(&value)?;
        self.put_raw_record(
            &ChunkKey::new(pos, ChunkRecordTag::HardcodedSpawners),
            value,
        );
        Ok(())
    }

    /// Stages a player record write using the player's storage key.
    ///
    /// # Errors
    ///
    /// Returns validation errors when the player id does not map to a `LevelDB`
    /// key.
    pub fn put_player(&mut self, player: &PlayerData) -> Result<()> {
        let Some(key) = player.id.storage_key() else {
            return Err(BedrockWorldError::Validation(
                "player id has no LevelDB key".to_string(),
            ));
        };
        self.batch
            .put(Bytes::copy_from_slice(key.as_ref()), player.raw.clone());
        Ok(())
    }

    /// Stages a typed map record write after roundtrip validation.
    ///
    /// # Errors
    ///
    /// Returns validation or serialization errors for malformed map data.
    pub fn put_map_record(&mut self, record: &ParsedMapData) -> Result<()> {
        let value = encode_map_record(record)?;
        parse_map_record(record.record_id.clone(), value.clone())?;
        self.batch.put(record.record_id.storage_key(), value);
        Ok(())
    }

    /// Stages a typed map record delete.
    pub fn delete_map_record(&mut self, id: &MapRecordId) {
        self.batch.delete(id.storage_key());
    }

    /// Stages a typed global record write after roundtrip validation.
    ///
    /// # Errors
    ///
    /// Returns validation or serialization errors for malformed global data.
    pub fn put_global_record(&mut self, record: &ParsedGlobalData) -> Result<()> {
        let value = encode_global_record(record)?;
        parse_global_record(record.kind.clone(), record.name.clone(), value.clone())?;
        self.batch.put(record.kind.storage_key(), value);
        Ok(())
    }

    /// Stages a typed global record delete.
    pub fn delete_global_record(&mut self, kind: &GlobalRecordKind) {
        self.batch.delete(kind.storage_key());
    }

    /// Stages a modern actor write and updates the chunk `digp` digest.
    ///
    /// # Errors
    ///
    /// Returns validation errors for malformed actor NBT or digest data.
    pub fn put_actor(&mut self, pos: ChunkPos, uid: ActorUid, value: Bytes) -> Result<()> {
        parse_entities_from_value(&value, &mut WorldParseReport::default());
        if self
            .actor_ownership()?
            .chunks(uid)
            .is_some_and(|chunks| chunks.iter().any(|owner| *owner != pos))
        {
            return Err(BedrockWorldError::Validation(format!(
                "actor storage id {uid:?} is already owned by another chunk digest"
            )));
        }
        self.replace_actor_digest(pos, |ids| {
            if !ids.contains(&uid) {
                ids.push(uid);
            }
        })?;
        self.batch.put(uid.storage_key(), value);
        Ok(())
    }

    /// Stages a modern actor delete and removes it from the chunk `digp` digest.
    ///
    /// # Errors
    ///
    /// Returns validation errors for malformed existing digest data.
    pub fn delete_actor(&mut self, pos: ChunkPos, uid: ActorUid) -> Result<()> {
        self.replace_actor_digest(pos, |ids| ids.retain(|id| *id != uid))?;
        if self
            .actor_ownership
            .as_ref()
            .is_some_and(|index| index.owner_count(uid) == 0)
        {
            self.batch.delete(uid.storage_key());
        }
        Ok(())
    }

    /// Validates and commits all staged writes atomically through the storage backend.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors for unsafe key/value combinations, or storage errors.
    pub fn commit(self) -> Result<()> {
        if self.read_only {
            return Err(BedrockWorldError::ReadOnly);
        }
        validate_batch(&self.batch)?;
        self.storage.storage().write_batch(&self.batch)
    }

    fn replace_actor_digest<F>(&mut self, pos: ChunkPos, update: F) -> Result<()>
    where
        F: FnOnce(&mut Vec<ActorUid>),
    {
        let key = ActorDigestKey::new(pos).storage_key();
        let mut ids = self
            .actor_ownership()?
            .actors(pos)
            .map(|actors| actors.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        update(&mut ids);
        let ids = ids.into_iter().collect::<BTreeSet<_>>();
        self.actor_ownership
            .as_mut()
            .expect("actor ownership is initialized")
            .replace_chunk(pos, ids.iter().copied());
        if ids.is_empty() {
            self.batch.delete(key);
        } else {
            self.batch.put(
                key,
                encode_actor_digest_ids(&ids.iter().copied().collect::<Vec<_>>()),
            );
        }
        Ok(())
    }

    fn actor_ownership(&mut self) -> Result<&mut ActorOwnershipIndex> {
        if self.actor_ownership.is_none() {
            self.actor_ownership = Some(ActorOwnershipIndex::scan(self.storage.storage())?);
        }
        Ok(self
            .actor_ownership
            .as_mut()
            .expect("actor ownership is initialized"))
    }
}

fn validate_batch(batch: &StorageBatch) -> Result<()> {
    for op in batch.ops() {
        match op {
            StorageOp::Put { key, value } => {
                if key.is_empty() {
                    return Err(BedrockWorldError::Validation(
                        "batch contains empty key".to_string(),
                    ));
                }
                if value.is_empty() {
                    return Err(BedrockWorldError::Validation(format!(
                        "batch put for key {key:?} contains empty value"
                    )));
                }
            }
            StorageOp::Delete { key } => {
                if key.is_empty() {
                    return Err(BedrockWorldError::Validation(
                        "batch contains empty delete key".to_string(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_block_entities_in_chunk(pos: ChunkPos, entities: &[ParsedBlockEntity]) -> Result<()> {
    for entity in entities {
        let Some([x, y, z]) = entity.position else {
            return Err(BedrockWorldError::Validation(
                "block entity is missing x/y/z position".to_string(),
            ));
        };
        let block_pos = BlockPos { x, y, z };
        if block_pos.to_chunk_pos(pos.dimension) != pos {
            return Err(BedrockWorldError::Validation(format!(
                "block entity at {x},{y},{z} is outside chunk {pos:?}"
            )));
        }
    }
    Ok(())
}
