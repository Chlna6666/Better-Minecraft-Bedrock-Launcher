use crate::chunk::{ActorUid, BedrockDbKey, ChunkPos};
use crate::database::{StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::error::Result;
use crate::parsed::parse_actor_digest_ids;
use std::collections::{BTreeMap, BTreeSet};

/// Global ownership graph between modern Bedrock actors and chunk `digp` records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActorOwnershipIndex {
    actors_by_chunk: BTreeMap<ChunkPos, BTreeSet<ActorUid>>,
    chunks_by_actor: BTreeMap<ActorUid, BTreeSet<ChunkPos>>,
}

impl ActorOwnershipIndex {
    /// Builds an ownership index from every visible `digp` record.
    ///
    /// # Errors
    ///
    /// Returns storage errors or malformed actor digest errors.
    pub fn scan(storage: &dyn WorldStorage) -> Result<Self> {
        let mut index = Self::default();
        storage.for_each_prefix(b"digp", StorageReadOptions::default(), &mut |key, value| {
            let BedrockDbKey::ActorDigest { pos } = BedrockDbKey::decode(key) else {
                return Ok(StorageVisitorControl::Continue);
            };
            index.replace_chunk(pos, parse_actor_digest_ids(value)?);
            Ok(StorageVisitorControl::Continue)
        })?;
        Ok(index)
    }

    /// Returns every chunk digest that currently references `uid`.
    #[must_use]
    pub fn chunks(&self, uid: ActorUid) -> Option<&BTreeSet<ChunkPos>> {
        self.chunks_by_actor.get(&uid)
    }

    /// Returns the actors referenced by one chunk digest.
    #[must_use]
    pub fn actors(&self, pos: ChunkPos) -> Option<&BTreeSet<ActorUid>> {
        self.actors_by_chunk.get(&pos)
    }

    /// Returns the number of chunk digests that reference `uid`.
    #[must_use]
    pub fn owner_count(&self, uid: ActorUid) -> usize {
        self.chunks_by_actor.get(&uid).map_or(0, BTreeSet::len)
    }

    pub(crate) fn replace_chunk(
        &mut self,
        pos: ChunkPos,
        actors: impl IntoIterator<Item = ActorUid>,
    ) {
        if let Some(previous) = self.actors_by_chunk.remove(&pos) {
            for uid in previous {
                if let Some(chunks) = self.chunks_by_actor.get_mut(&uid) {
                    chunks.remove(&pos);
                    if chunks.is_empty() {
                        self.chunks_by_actor.remove(&uid);
                    }
                }
            }
        }
        let actors = actors.into_iter().collect::<BTreeSet<_>>();
        for uid in &actors {
            self.chunks_by_actor.entry(*uid).or_default().insert(pos);
        }
        if !actors.is_empty() {
            self.actors_by_chunk.insert(pos, actors);
        }
    }
}
