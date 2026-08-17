//! Player record identifiers and raw player payload helpers.

use crate::error::Result;
use crate::nbt::{NbtTag, parse_root_nbt, serialize_root_nbt};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
/// Player identifier as stored by Bedrock.
pub enum PlayerId {
    /// Local single-player record stored under `~local_player`.
    Local,
    /// Xbox user id record stored under `player_<xuid>`.
    Xuid(String),
    /// Legacy player data embedded in `level.dat`.
    LegacyLevelDat,
    /// Player-like identifier that is not backed by a known `LevelDB` key.
    Unknown(String),
}

impl PlayerId {
    #[must_use]
    /// Encodes this value as its Bedrock storage key.
    pub fn storage_key(&self) -> Option<Cow<'_, [u8]>> {
        match self {
            Self::Local => Some(Cow::Borrowed(b"~local_player")),
            Self::Xuid(xuid) => Some(Cow::Owned(format!("player_{xuid}").into_bytes())),
            Self::LegacyLevelDat | Self::Unknown(_) => None,
        }
    }

    #[must_use]
    /// Decodes a Bedrock player storage key.
    pub fn from_storage_key(key: &[u8]) -> Option<Self> {
        if key == b"~local_player" {
            return Some(Self::Local);
        }
        let text = std::str::from_utf8(key).ok()?;
        text.strip_prefix("player_")
            .map(|xuid| Self::Xuid(xuid.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Decoded player record with both structured NBT and original raw bytes.
pub struct PlayerData {
    /// Player record id.
    pub id: PlayerId,
    /// Parsed root NBT.
    pub nbt: NbtTag,
    /// Raw bytes as stored in `LevelDB`.
    pub raw: Bytes,
}

impl PlayerData {
    /// Parses a raw player payload into structured NBT while retaining bytes.
    pub fn from_raw(id: PlayerId, raw: Bytes) -> Result<Self> {
        let nbt = parse_root_nbt(&raw)?;
        Ok(Self { id, nbt, raw })
    }

    /// Serializes structured NBT into a raw player payload.
    pub fn from_nbt(id: PlayerId, nbt: NbtTag) -> Result<Self> {
        let raw = Bytes::from(serialize_root_nbt(&nbt)?);
        Ok(Self { id, nbt, raw })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_keys_roundtrip() {
        assert_eq!(
            PlayerId::from_storage_key(b"~local_player"),
            Some(PlayerId::Local)
        );
        assert_eq!(
            PlayerId::from_storage_key(b"player_123"),
            Some(PlayerId::Xuid("123".to_string()))
        );
        assert_eq!(
            PlayerId::Xuid("123".to_string()).storage_key().as_deref(),
            Some(&b"player_123"[..])
        );
    }
}
