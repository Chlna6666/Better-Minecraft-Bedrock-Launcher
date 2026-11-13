//! Lossless operations for raw `player_*` LevelDB keys.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::{NbtTag, parse_root, serialize_root};
use crate::storage::{StorageBatch, WorldStorage};
use bytes::Bytes;

const PREFIX: &[u8] = b"player_";

fn validate(key: &[u8]) -> Result<()> {
    if !key.starts_with(PREFIX) || key.len() == PREFIX.len() {
        return Err(BedrockWorldError::Validation(
            "player LevelDB key must start with player_ and contain a non-empty suffix".to_string(),
        ));
    }
    Ok(())
}

/// One exact raw-key `player_*` record.
///
/// The suffix remains bytes because it may not be UTF-8. This type deliberately does not infer
/// account identity from the key.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    key: Bytes,
    /// Parsed player NBT compound.
    pub nbt: NbtTag,
    raw: Bytes,
    original_nbt: NbtTag,
}

impl Record {
    /// Parses one exact key/value pair while retaining both byte sequences.
    pub fn from_raw(key: Bytes, raw: Bytes) -> Result<Self> {
        validate(&key)?;
        let nbt = parse_root(&raw)?;
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
        &self.key[PREFIX.len()..]
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
        Ok(Bytes::from(serialize_root(&self.nbt)?))
    }
}

/// Reads one exact raw `player_*` LevelDB key without converting it to text.
pub fn read(storage: &dyn WorldStorage, key: &[u8]) -> Result<Option<Record>> {
    validate(key)?;
    storage
        .get(key)?
        .map(|raw| Record::from_raw(Bytes::copy_from_slice(key), raw))
        .transpose()
}

/// Writes a raw-key player record back to the exact key from which it was read.
pub fn write(storage: &dyn WorldStorage, record: &Record) -> Result<()> {
    validate(record.key())?;
    let mut batch = StorageBatch::new();
    batch.put(record.key().clone(), record.to_raw()?);
    storage.write_batch(&batch)
}

/// Deletes one exact raw `player_*` key without interpreting its suffix.
pub fn delete(storage: &dyn WorldStorage, key: &[u8]) -> Result<()> {
    validate(key)?;
    let mut batch = StorageBatch::new();
    batch.delete(Bytes::copy_from_slice(key));
    storage.write_batch(&batch)
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
    fn read_preserves_raw_suffix_and_original_bytes() {
        let storage = MemoryStorage::new();
        let key = b"player_\xff";
        let raw = Bytes::from(serialize_root(&player_nbt("Alex")).unwrap());
        storage.put(key, &raw).unwrap();

        let record = read(&storage, key).unwrap().unwrap();
        assert_eq!(record.key().as_ref(), key);
        assert_eq!(record.suffix(), b"\xff");
        assert_eq!(record.to_raw().unwrap(), raw);
        assert!(!record.is_modified());
    }

    #[test]
    fn write_keeps_raw_key_after_edit() {
        let storage = MemoryStorage::new();
        let key = b"player_custom\xff";
        let raw = Bytes::from(serialize_root(&player_nbt("Alex")).unwrap());
        storage.put(key, &raw).unwrap();

        let mut record = read(&storage, key).unwrap().unwrap();
        record.edit_nbt(|nbt| {
            let NbtTag::Compound(root) = nbt else {
                unreachable!()
            };
            root.insert("PlayerLevel".to_string(), NbtTag::Int(7));
        });
        assert!(record.is_modified());
        write(&storage, &record).unwrap();

        let written = storage.get(key).unwrap().unwrap();
        let parsed = parse_root(&written).unwrap();
        let NbtTag::Compound(root) = parsed else {
            unreachable!()
        };
        assert_eq!(root.get("PlayerLevel"), Some(&NbtTag::Int(7)));
        assert!(storage.get(b"player_custom").unwrap().is_none());
    }

    #[test]
    fn key_operations_reject_non_player_keys_and_empty_suffix() {
        let storage = MemoryStorage::new();
        assert!(read(&storage, b"actorprefix123").is_err());
        assert!(delete(&storage, b"player_").is_err());
    }
}
