//! Buffered Minecraft Bedrock world transaction operations.

use super::*;

#[derive(Debug, Clone)]
struct StoragePrecondition {
    key: Bytes,
    expected: Option<Bytes>,
}

/// Buffered LevelDB mutations for one Minecraft Bedrock world.
///
/// A transaction can stage player, map, chunk, actor and raw-record mutations into one
/// [`StorageBatch`]. Commits created from clones of the same [`BedrockWorld`] are serialized by the
/// shared world mutation lock. Player update/create helpers also validate their source condition
/// while that lock is held, preventing an older in-process read snapshot from silently replacing a
/// newer LevelDB value.
///
/// `level.dat` is a separate file and is intentionally outside this atomic LevelDB boundary.
pub struct WorldTransaction<'a, S = Arc<dyn WorldStorage>>
where
    S: WorldStorageHandle,
{
    pub(super) storage: &'a S,
    pub(super) batch: StorageBatch,
    pub(super) read_only: bool,
    pub(super) actor_ownership: Option<ActorOwnershipIndex>,
    pub(super) preconditions: Vec<StoragePrecondition>,
    pub(super) mutation_lock: &'a Mutex<()>,
}

impl<S> WorldTransaction<'_, S>
where
    S: WorldStorageHandle,
{
    /// Stages one exact raw chunk-record write in this transaction.
    pub fn put_raw_record(&mut self, key: &ChunkKey, value: impl Into<Bytes>) {
        self.batch.put(key.encode(), value.into());
    }

    /// Stages deletion of one exact raw chunk record.
    pub fn delete_raw_record(&mut self, key: &ChunkKey) {
        self.batch.delete(key.encode());
    }

    /// Stages one exact raw key/value write.
    ///
    /// This is a low-level escape hatch for real Bedrock records that do not yet have a typed
    /// transaction API. Prefer typed methods when one exists.
    pub fn put_raw_key(&mut self, key: impl Into<Bytes>, value: impl Into<Bytes>) {
        self.batch.put(key.into(), value.into());
    }

    /// Stages deletion of one exact raw storage key.
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

    /// Stages a validated BlockEntity payload for one chunk.
    ///
    /// The complete BlockEntity record is encoded and round-trip validated before it enters the
    /// transaction batch.
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

    /// Stages an update to an existing LevelDB-backed player record.
    ///
    /// The player's original raw bytes are treated as the source snapshot. During [`Self::commit`],
    /// the current `~local_player` or `player_*` value must still equal that snapshot. The staged value
    /// is produced by [`PlayerData::to_raw`], so edits made through `PlayerData` are persisted instead
    /// of accidentally writing the old source bytes.
    ///
    /// This method does not accept historical `level.dat.Player`, because that record is not in
    /// LevelDB and cannot participate in this atomic batch.
    ///
    /// # Errors
    ///
    /// Returns a validation error for non-LevelDB player ids, serialization errors for invalid player
    /// NBT, or [`BedrockWorldError::ConcurrentWrite`] at commit time when the stored player changed
    /// after it was read.
    pub fn update_player(&mut self, player: &PlayerData) -> Result<()> {
        let Some(key) = player.id.storage_key() else {
            return Err(BedrockWorldError::Validation(
                "player id has no LevelDB key".to_string(),
            ));
        };
        let key = Bytes::copy_from_slice(key.as_ref());
        let value = player.to_raw()?;
        self.preconditions.push(StoragePrecondition {
            key: key.clone(),
            expected: Some(player.raw.clone()),
        });
        self.batch.put(key, value);
        Ok(())
    }

    /// Stages creation of a LevelDB-backed player record only when the target key does not exist.
    ///
    /// Use this for a genuinely new `~local_player` or `player_*` record. Updating a record that was
    /// read earlier must use [`Self::update_player`] so stale async reads cannot overwrite newer data.
    ///
    /// # Errors
    ///
    /// Returns validation/serialization errors immediately, or
    /// [`BedrockWorldError::ConcurrentWrite`] at commit time when the target key already exists.
    pub fn create_player(&mut self, player: &PlayerData) -> Result<()> {
        let Some(key) = player.id.storage_key() else {
            return Err(BedrockWorldError::Validation(
                "player id has no LevelDB key".to_string(),
            ));
        };
        let key = Bytes::copy_from_slice(key.as_ref());
        let value = player.to_raw()?;
        self.preconditions.push(StoragePrecondition {
            key: key.clone(),
            expected: None,
        });
        self.batch.put(key, value);
        Ok(())
    }

    /// Stages a typed map record write after round-trip validation.
    ///
    /// Player changes staged in the same transaction are committed in the same LevelDB batch, making
    /// map/editor events and player inventory changes visible together to this storage backend.
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

    /// Stages deletion of one typed map record.
    pub fn delete_map_record(&mut self, id: &MapRecordId) {
        self.batch.delete(id.storage_key());
    }

    /// Stages a typed global record write after round-trip validation.
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

    /// Stages deletion of one typed global record.
    pub fn delete_global_record(&mut self, kind: &GlobalRecordKind) {
        self.batch.delete(kind.storage_key());
    }

    /// Stages a modern actor write and updates the owning chunk's `digp` digest.
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

    /// Stages a modern actor delete and removes it from the owning chunk's `digp` digest.
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

    /// Validates source conditions and commits all staged LevelDB mutations atomically.
    ///
    /// Commits from clones of the same world handle are serialized. Source conditions are checked
    /// while that mutation lock is held and immediately before the backend batch write, which closes
    /// the in-process read/validate/write race for `update_player` and `create_player`.
    ///
    /// This does not coordinate another independently opened world handle or an external Minecraft
    /// process. Callers must still avoid editing a world that the game is actively writing.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds,
    /// [`BedrockWorldError::ConcurrentWrite`] when a source condition is stale, validation errors for
    /// unsafe raw operations, or storage errors.
    pub fn commit(self) -> Result<()> {
        if self.read_only {
            return Err(BedrockWorldError::ReadOnly);
        }
        validate_batch(&self.batch)?;
        let _mutation = self.mutation_lock.lock().map_err(|_| {
            BedrockWorldError::ConcurrentWrite("world mutation lock poisoned".to_string())
        })?;
        validate_preconditions(self.storage.storage(), &self.preconditions)?;
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

fn validate_preconditions(
    storage: &dyn WorldStorage,
    preconditions: &[StoragePrecondition],
) -> Result<()> {
    for condition in preconditions {
        let current = storage.get(condition.key.as_ref())?;
        if current != condition.expected {
            return Err(BedrockWorldError::ConcurrentWrite(format!(
                "storage source changed before transaction commit for key {:?}",
                condition.key
            )));
        }
    }
    Ok(())
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
