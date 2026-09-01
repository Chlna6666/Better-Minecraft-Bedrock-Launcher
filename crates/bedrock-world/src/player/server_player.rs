//! Minecraft Bedrock `player_<id>` LevelDB records.
//!
//! The storage key is authoritative. Raw-key APIs preserve arbitrary suffix bytes and never assume
//! that a `player_*` suffix is an Xbox XUID or valid UTF-8. The older string-suffix helpers remain for
//! callers that already have a textual Bedrock player key.

use crate::error::{BedrockWorldError, Result};
use crate::level::LevelDatDocument;
use crate::nbt::{NbtTag, parse_root_nbt, serialize_root_nbt};
use crate::player::{PlayerData, PlayerId};
use crate::storage::{StorageBatch, WorldStorage};
use bytes::Bytes;

const PLAYER_KEY_PREFIX: &[u8] = b"player_";

fn key(id: &str) -> Result<Bytes> {
    if id.is_empty() {
        return Err(BedrockWorldError::Validation(
            "player_<id> suffix cannot be empty".to_string(),
        ));
    }
    Ok(Bytes::from(format!("player_{id}")))
}

fn validate_player_key(key: &[u8]) -> Result<()> {
    if !key.starts_with(PLAYER_KEY_PREFIX) || key.len() == PLAYER_KEY_PREFIX.len() {
        return Err(BedrockWorldError::Validation(
            "player LevelDB key must start with player_ and contain a non-empty suffix".to_string(),
        ));
    }
    Ok(())
}

/// One exact raw-key `player_*` record.
///
/// This is the lossless storage-level representation for player keys whose suffix may not be UTF-8.
/// It deliberately does not infer account identity from the key bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerKeyRecord {
    key: Bytes,
    /// Parsed player NBT compound.
    pub nbt: NbtTag,
    raw: Bytes,
    original_nbt: NbtTag,
}

impl PlayerKeyRecord {
    /// Parses one exact `player_*` key/value pair while retaining both byte sequences.
    pub fn from_raw(key: Bytes, raw: Bytes) -> Result<Self> {
        validate_player_key(&key)?;
        let nbt = parse_root_nbt(&raw)?;
        if !matches!(nbt, NbtTag::Compound(_)) {
            return Err(BedrockWorldError::CorruptWorld(
                "player_* value root is not an NBT compound".to_string(),
            ));
        }
        Ok(Self {
            key,
            original_nbt: nbt.clone(),
            nbt,
            raw,
        })
    }

    /// Returns the exact LevelDB key, including the `player_` prefix.
    #[must_use]
    pub fn key(&self) -> &Bytes {
        &self.key
    }

    /// Returns the exact suffix bytes after `player_`.
    #[must_use]
    pub fn suffix(&self) -> &[u8] {
        &self.key[PLAYER_KEY_PREFIX.len()..]
    }

    /// Returns whether the current NBT differs from the value that was read.
    #[must_use]
    pub fn is_modified(&self) -> bool {
        self.nbt != self.original_nbt
    }

    /// Edits the owned NBT without changing this record's LevelDB key.
    pub fn edit_nbt<R>(&mut self, edit: impl FnOnce(&mut NbtTag) -> R) -> R {
        edit(&mut self.nbt)
    }

    /// Serializes current NBT, returning original value bytes verbatim when unchanged.
    pub fn to_raw(&self) -> Result<Bytes> {
        if !self.is_modified() {
            return Ok(self.raw.clone());
        }
        Ok(Bytes::from(serialize_root_nbt(&self.nbt)?))
    }
}

/// Reads one exact raw `player_*` LevelDB key without UTF-8 conversion.
pub fn read_player_key(storage: &dyn WorldStorage, key: &[u8]) -> Result<Option<PlayerKeyRecord>> {
    validate_player_key(key)?;
    storage
        .get(key)?
        .map(|raw| PlayerKeyRecord::from_raw(Bytes::copy_from_slice(key), raw))
        .transpose()
}

/// Writes a raw-key player record back to the exact key from which it was read.
pub fn write_player_key(storage: &dyn WorldStorage, player: &PlayerKeyRecord) -> Result<()> {
    validate_player_key(player.key())?;
    let mut batch = StorageBatch::new();
    batch.put(player.key().clone(), player.to_raw()?);
    storage.write_batch(&batch)
}

/// Deletes one exact raw `player_*` key without interpreting its suffix.
pub fn delete_player_key(storage: &dyn WorldStorage, key: &[u8]) -> Result<()> {
    validate_player_key(key)?;
    let mut batch = StorageBatch::new();
    batch.delete(Bytes::copy_from_slice(key));
    storage.write_batch(&batch)
}

/// Reads one textual `player_<id>` record.
pub fn read_server_player(storage: &dyn WorldStorage, id: &str) -> Result<Option<PlayerData>> {
    storage
        .get(&key(id)?)?
        .map(|raw| PlayerData::from_raw(PlayerId::Xuid(id.to_string()), raw))
        .transpose()
}

/// Reads one textual `player_<id>` record with actual Bedrock version evidence from `level.dat`.
pub fn read_server_player_with_level(
    storage: &dyn WorldStorage,
    id: &str,
    level: &LevelDatDocument,
) -> Result<Option<PlayerData>> {
    storage
        .get(&key(id)?)?
        .map(|raw| PlayerData::from_raw_with_level(PlayerId::Xuid(id.to_string()), raw, level))
        .transpose()
}

/// Writes one player back to the exact textual `player_<id>` key selected when it was read.
///
/// The record suffix must match the supplied player's persisted source id.
pub fn write_server_player(
    storage: &dyn WorldStorage,
    id: &str,
    player: &PlayerData,
) -> Result<()> {
    if player.id.player_key_id() != Some(id) {
        return Err(BedrockWorldError::Validation(format!(
            "write_server_player target player_{id} does not match PlayerData source {:?}",
            player.id
        )));
    }
    let mut batch = StorageBatch::new();
    batch.put(key(id)?, player.to_raw()?);
    storage.write_batch(&batch)
}

/// Deletes one exact textual `player_<id>` key.
pub fn delete_server_player(storage: &dyn WorldStorage, id: &str) -> Result<()> {
    delete_player_key(storage, &key(id)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemoryStorage;
    use indexmap::IndexMap;

    fn player_nbt(name: &str) -> NbtTag {
        NbtTag::Compound(IndexMap::from([(
            "PlayerName".to_string(),
            NbtTag::String(name.to_string()),
        )]))
    }

    #[test]
    fn raw_player_key_preserves_non_utf8_suffix_and_original_bytes() {
        let storage = MemoryStorage::new();
        let key = b"player_\xff";
        let raw = Bytes::from(serialize_root_nbt(&player_nbt("Alex")).unwrap());
        storage.put(key, &raw).unwrap();

        let record = read_player_key(&storage, key).unwrap().unwrap();
        assert_eq!(record.key().as_ref(), key);
        assert_eq!(record.suffix(), b"\xff");
        assert_eq!(record.to_raw().unwrap(), raw);
        assert!(!record.is_modified());
    }

    #[test]
    fn raw_player_key_edit_writes_same_key_only() {
        let storage = MemoryStorage::new();
        let key = b"player_custom\xff";
        let raw = Bytes::from(serialize_root_nbt(&player_nbt("Alex")).unwrap());
        storage.put(key, &raw).unwrap();

        let mut record = read_player_key(&storage, key).unwrap().unwrap();
        record.edit_nbt(|nbt| {
            let NbtTag::Compound(root) = nbt else {
                unreachable!()
            };
            root.insert("PlayerLevel".to_string(), NbtTag::Int(7));
        });
        assert!(record.is_modified());
        write_player_key(&storage, &record).unwrap();

        let written = storage.get(key).unwrap().unwrap();
        let parsed = parse_root_nbt(&written).unwrap();
        let NbtTag::Compound(root) = parsed else {
            unreachable!()
        };
        assert_eq!(root.get("PlayerLevel"), Some(&NbtTag::Int(7)));
        assert!(storage.get(b"player_custom").unwrap().is_none());
    }

    #[test]
    fn raw_player_key_rejects_non_player_keys_and_empty_suffix() {
        let storage = MemoryStorage::new();
        assert!(read_player_key(&storage, b"actorprefix123").is_err());
        assert!(delete_player_key(&storage, b"player_").is_err());
    }
}
