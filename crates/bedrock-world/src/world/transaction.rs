//! Buffered Minecraft Bedrock world transactions.

use super::*;

#[derive(Debug, Clone)]
pub(super) struct StoragePrecondition {
    key: Bytes,
    expected: Option<Bytes>,
}

#[derive(Debug, Clone)]
pub(super) struct ChunkRecordsPrecondition {
    pos: ChunkPos,
    expected: BTreeMap<Bytes, Bytes>,
}

/// Buffered LevelDB mutations for one Minecraft Bedrock world.
///
/// A transaction can stage player, map, chunk, actor and raw-record mutations into one
/// [`StorageBatch`]. Commits opened for the same world path are serialized by the shared mutation
/// lock. Player and map writes validate their source values there; staged block-edit plans validate
/// the complete source chunk record sets. An older in-process snapshot cannot silently replace a
/// newer LevelDB value through these typed paths.
///
/// Direct raw storage writes and external Minecraft processes do not participate in this lock.
/// `level.dat` is a separate file and is intentionally outside this atomic LevelDB boundary.
pub struct WorldTransaction<'a, S = Arc<dyn WorldStorage>>
where
    S: StorageBackend,
{
    pub(super) storage: &'a S,
    pub(super) batch: StorageBatch,
    pub(super) read_only: bool,
    pub(super) actor_ownership: Option<ActorOwnershipIndex>,
    pub(super) preconditions: Vec<StoragePrecondition>,
    pub(super) chunk_preconditions: Vec<ChunkRecordsPrecondition>,
    pub(super) mutation_lock: Arc<Mutex<()>>,
}

impl<S> WorldTransaction<'_, S>
where
    S: StorageBackend,
{
    /// Stages one exact raw chunk-record write in this transaction.
    pub fn put_raw(&mut self, key: &ChunkKey, value: impl Into<Bytes>) {
        self.batch.put(key.encode(), value.into());
    }

    /// Stages deletion of one exact raw chunk record.
    pub fn delete_raw(&mut self, key: &ChunkKey) {
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

    /// Requires the current bytes of a raw LevelDB key to match a source snapshot.
    ///
    /// Use this with raw mutations whose structured domain API is unavailable,
    /// including history replay. The condition is checked under the world
    /// transaction's commit lock immediately before its single `StorageBatch`.
    /// `None` requires the key to remain absent. This does not coordinate
    /// external Minecraft processes or the separate `level.dat` file.
    ///
    /// # Errors
    /// [`Self::commit`] returns [`BedrockWorldError::ConcurrentWrite`] if the
    /// current record differs from `expected`, or a storage read error.
    pub fn require_raw_key(&mut self, key: impl Into<Bytes>, expected: Option<Bytes>) {
        self.preconditions.push(StoragePrecondition {
            key: key.into(),
            expected,
        });
    }

    pub(crate) fn require_chunk_records(
        &mut self,
        pos: ChunkPos,
        expected: BTreeMap<Bytes, Bytes>,
    ) {
        self.chunk_preconditions
            .push(ChunkRecordsPrecondition { pos, expected });
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
    pub fn put_block_entities(&mut self, pos: ChunkPos, entities: &[BlockEntity]) -> Result<()> {
        validate_block_entities_in_chunk(pos, entities)?;
        let roots = entities
            .iter()
            .map(|entity| entity.nbt.clone())
            .collect::<Vec<_>>();
        let value = encode_consecutive_roots(&roots)?;
        let mut report = ScanReport::default();
        let parsed = parse_block_entities_from_value(&value, &mut report);
        validate_block_entities_in_chunk(pos, &parsed)?;
        self.put_raw(&ChunkKey::new(pos, ChunkRecordTag::BlockEntity), value);
        Ok(())
    }

    /// Stages a chunk's complete BlockEntity payload with a source-record condition.
    ///
    /// `expected` must be the raw bytes returned by [`World::block_entities_snapshot`] before
    /// editing. The condition is checked under the commit lock immediately before the single
    /// LevelDB batch; `None` requires the record to remain absent. Unknown NBT fields in the
    /// supplied entities are serialized unchanged. This cannot guard writes by an external game
    /// process that does not share the lock.
    ///
    /// # Errors
    ///
    /// Returns validation or serialization errors while staging, or
    /// [`BedrockWorldError::ConcurrentWrite`] at commit when the source changed.
    pub fn update_block_entities(
        &mut self,
        pos: ChunkPos,
        expected: Option<Bytes>,
        entities: &[BlockEntity],
    ) -> Result<()> {
        self.put_block_entities(pos, entities)?;
        self.require_raw_key(
            ChunkKey::new(pos, ChunkRecordTag::BlockEntity).encode(),
            expected,
        );
        Ok(())
    }

    /// Stages a validated hardcoded-spawn-area payload for one chunk.
    ///
    /// # Errors
    ///
    /// Returns validation or serialization errors.
    pub fn save_hardcoded_spawn_areas(
        &mut self,
        pos: ChunkPos,
        areas: &[HardcodedSpawnArea],
    ) -> Result<()> {
        let value = encode_hardcoded_spawn_areas(areas)?;
        decode_hardcoded_spawn_areas(&value)?;
        self.put_raw(
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

    /// Stages a Bedrock `map_<id>` write after round-trip validation.
    ///
    /// An empty `raw` value means a new map and requires an absent key. Otherwise `raw` is the
    /// source snapshot and must still match the stored value when the single LevelDB batch commits.
    /// Unknown NBT fields in `roots` are serialized along with the edited fields. This does not
    /// change the map format version or update any player's `filled_map` reference.
    /// The mutation lock coordinates transactions opened through this world; direct raw storage
    /// writes and external Minecraft processes can still write concurrently.
    ///
    /// # Errors
    ///
    /// Returns validation or serialization errors for malformed map data, or
    /// [`BedrockWorldError::ConcurrentWrite`] if the source record changed.
    pub fn save_map_item(&mut self, item: &SavedData) -> Result<()> {
        let value = encode_map_item(item)?;
        decode_map_item(item.id.clone(), value.clone())?;
        let key = item.id.storage_key();
        self.preconditions.push(StoragePrecondition {
            key: key.clone(),
            expected: (!item.raw.is_empty()).then(|| item.raw.clone()),
        });
        self.batch.put(key, value);
        Ok(())
    }

    /// Stages deletion of one Bedrock map item.
    pub fn delete_map_item(&mut self, id: &MapItemId) {
        self.batch.delete(id.storage_key());
    }

    /// Stages a typed global record write after round-trip validation.
    ///
    /// # Errors
    ///
    /// Returns validation or serialization errors for malformed global data.
    pub fn save_global(&mut self, record: &Global) -> Result<()> {
        let value = encode_global(record)?;
        decode_global(record.kind.clone(), record.name.clone(), value.clone())?;
        self.batch.put(record.kind.storage_key(), value);
        Ok(())
    }

    /// Stages deletion of one typed global record.
    pub fn delete_global(&mut self, kind: &GlobalRecordKind) {
        self.batch.delete(kind.storage_key());
    }

    /// Stages a modern actor write and updates the owning chunk's `digp` digest.
    ///
    /// # Errors
    ///
    /// Returns validation errors for malformed actor NBT or digest data.
    pub fn put_actor(&mut self, pos: ChunkPos, uid: ActorUid, value: Bytes) -> Result<()> {
        parse_entities_from_value(&value, &mut ScanReport::default());
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
    /// Transactions opened for the same world path are serialized. Source conditions are checked
    /// while that mutation lock is held and immediately before the backend batch write, which closes
    /// the in-process read/validate/write race for typed player, map and staged chunk writes.
    ///
    /// This does not coordinate an external Minecraft process. Callers must still avoid editing a
    /// world that the game is actively writing.
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
        validate_chunk_preconditions(self.storage.storage(), &self.chunk_preconditions)?;
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
                encode_actor_ids(&ids.iter().copied().collect::<Vec<_>>()),
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

fn validate_chunk_preconditions(
    storage: &dyn WorldStorage,
    preconditions: &[ChunkRecordsPrecondition],
) -> Result<()> {
    for condition in preconditions {
        let mut current = BTreeMap::new();
        storage.for_each_prefix(
            &chunk_record_prefix(condition.pos),
            StorageReadOptions::default(),
            &mut |raw_key, value| {
                let key = ChunkKey::decode(raw_key).map_err(|error| {
                    BedrockWorldError::CorruptWorld(format!(
                        "invalid chunk record key under prefix for {:?}: {error}",
                        condition.pos
                    ))
                })?;
                if key.pos == condition.pos {
                    current.insert(Bytes::copy_from_slice(raw_key), value.clone());
                }
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        if current != condition.expected {
            return Err(BedrockWorldError::ConcurrentWrite(format!(
                "chunk {:?} changed before transaction commit",
                condition.pos
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

fn validate_block_entities_in_chunk(pos: ChunkPos, entities: &[BlockEntity]) -> Result<()> {
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
