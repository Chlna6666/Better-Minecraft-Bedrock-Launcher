//! Player record identifiers, raw payloads and automatic persisted-format detection.

use crate::error::Result;
use crate::level::LevelDatDocument;
use crate::nbt::{NbtTag, parse_root_nbt, serialize_root_nbt};
use crate::player::format::PlayerDataFormat;
use crate::version::LevelVersion;
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
    /// Historical player data embedded in `level.dat`.
    LegacyLevelDat,
    /// Player-like identifier that is not backed by a known Bedrock storage location.
    Unknown(String),
}

impl PlayerId {
    #[must_use]
    /// Encodes this value as its LevelDB storage key when it belongs in the world database.
    pub fn storage_key(&self) -> Option<Cow<'_, [u8]>> {
        match self {
            Self::Local => Some(Cow::Borrowed(b"~local_player")),
            Self::Xuid(xuid) => Some(Cow::Owned(format!("player_{xuid}").into_bytes())),
            Self::LegacyLevelDat | Self::Unknown(_) => None,
        }
    }

    #[must_use]
    /// Decodes a Bedrock player LevelDB storage key.
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
/// Decoded player record retaining source bytes and automatically detected persisted format.
pub struct PlayerData {
    /// Player record id and storage source.
    pub id: PlayerId,
    /// Parsed root NBT.
    pub nbt: NbtTag,
    /// Raw bytes read from the source, retained for exact unmodified round-trips.
    pub raw: Bytes,
    /// Persisted format evidence detected without converting the payload.
    pub format: PlayerDataFormat,
}

impl PlayerData {
    /// Parses a raw player payload and automatically detects its persisted format.
    pub fn from_raw(id: PlayerId, raw: Bytes) -> Result<Self> {
        Self::from_raw_with_level_version(id, raw, None)
    }

    /// Parses a raw player payload using real version evidence from `level.dat`.
    pub fn from_raw_with_level(
        id: PlayerId,
        raw: Bytes,
        level: &LevelDatDocument,
    ) -> Result<Self> {
        let level_version = Some(LevelVersion::detect(level)?);
        Self::from_raw_with_level_version(id, raw, level_version)
    }

    /// Creates a player payload from structured NBT and automatically detects its persisted format.
    pub fn from_nbt(id: PlayerId, nbt: NbtTag) -> Result<Self> {
        Self::from_nbt_with_level_version(id, nbt, None)
    }

    /// Creates a player payload from structured NBT using real version evidence from `level.dat`.
    pub fn from_nbt_with_level(
        id: PlayerId,
        nbt: NbtTag,
        level: &LevelDatDocument,
    ) -> Result<Self> {
        let level_version = Some(LevelVersion::detect(level)?);
        Self::from_nbt_with_level_version(id, nbt, level_version)
    }

    /// Creates player data with already detected level-version evidence.
    pub fn from_nbt_with_level_version(
        id: PlayerId,
        nbt: NbtTag,
        level_version: Option<LevelVersion>,
    ) -> Result<Self> {
        let raw = Bytes::from(serialize_root_nbt(&nbt)?);
        let format = PlayerDataFormat::detect(&id, &nbt, level_version);
        Ok(Self { id, nbt, raw, format })
    }

    /// Serializes the current structured NBT without changing its detected/selected data format.
    pub fn to_raw(&self) -> Result<Bytes> {
        Ok(Bytes::from(serialize_root_nbt(&self.nbt)?))
    }

    /// Re-runs format detection after the caller edits the structured NBT.
    pub fn refresh_format(&mut self, level_version: Option<LevelVersion>) {
        self.format = PlayerDataFormat::detect(&self.id, &self.nbt, level_version);
    }

    fn from_raw_with_level_version(
        id: PlayerId,
        raw: Bytes,
        level_version: Option<LevelVersion>,
    ) -> Result<Self> {
        let nbt = parse_root_nbt(&raw)?;
        let format = PlayerDataFormat::detect(&id, &nbt, level_version);
        Ok(Self { id, nbt, raw, format })
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
