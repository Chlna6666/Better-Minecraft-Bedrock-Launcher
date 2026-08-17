//! Minecraft Bedrock LevelChunk records grouped by chunk position.

use super::key::{ChunkKey, ChunkRecordTag};
use super::legacy::LegacyTerrain;
use super::palette::BlockState;
use super::position::ChunkPos;
use super::subchunk::{SubChunk, parse_subchunk};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::{NbtTag, parse_consecutive_root_nbt};
use bytes::Bytes;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Raw chunk record paired with its decoded chunk key.
pub struct ChunkRecord {
    /// Decoded storage key for this record.
    pub key: ChunkKey,
    /// Parsed or raw value associated with this record.
    pub value: Bytes,
}

#[derive(Debug, Clone, PartialEq)]
/// Entity NBT payload stored directly in a chunk record.
pub struct EntityData {
    /// Root NBT tag for the entity payload.
    pub tag: NbtTag,
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed Bedrock LevelChunk with records grouped by position.
pub struct Chunk {
    /// Chunk position represented by this parsed chunk.
    pub pos: ChunkPos,
    /// Bedrock format or payload version.
    pub version: Option<u8>,
    /// Records included in this result.
    pub records: Vec<ChunkRecord>,
}

impl Chunk {
    /// Returns a decoded subchunk by vertical index, when the record is present.
    pub fn get_subchunk(&self, y: i8) -> Result<Option<SubChunk>> {
        let Some(record) = self.records.iter().find(|record| {
            record.key.tag == ChunkRecordTag::SubChunkPrefix && record.key.subchunk_y == Some(y)
        }) else {
            return Ok(None);
        };
        parse_subchunk(y, record.value.clone()).map(Some)
    }

    /// Returns the decoded legacy terrain record, when present.
    pub fn legacy_terrain(&self) -> Result<Option<LegacyTerrain>> {
        let Some(record) = self
            .records
            .iter()
            .find(|record| record.key.tag == ChunkRecordTag::LegacyTerrain)
        else {
            return Ok(None);
        };
        LegacyTerrain::parse(record.value.clone()).map(Some)
    }

    /// Returns the semantic block state at local chunk coordinates.
    pub fn get_block(&self, x: u8, y: i16, z: u8) -> Result<BlockState> {
        if x >= 16 || z >= 16 {
            return Err(BedrockWorldError::Validation(format!(
                "local block coordinates must use x/z in 0..15, got x={x}, z={z}"
            )));
        }

        let subchunk_y = i8::try_from(i32::from(y).div_euclid(16)).map_err(|_| {
            BedrockWorldError::Validation(format!(
                "block y={y} cannot be represented as a Bedrock subchunk index"
            ))
        })?;
        let local_y = u8::try_from(i32::from(y).rem_euclid(16)).map_err(|_| {
            BedrockWorldError::Validation(format!("block y={y} has invalid local subchunk offset"))
        })?;
        if let Some(subchunk) = self.get_subchunk(subchunk_y)? {
            if let Some(state) = subchunk.block_state_at(x, local_y, z) {
                return Ok(state.clone());
            }
            if let Some(id) = subchunk.legacy_block_id_at(x, local_y, z) {
                let mut states = BTreeMap::new();
                if let Some(data) = subchunk.legacy_block_data_at(x, local_y, z) {
                    states.insert("data".to_string(), NbtTag::Byte(data as i8));
                }
                return Ok(BlockState {
                    name: format!("legacy:{id}"),
                    states,
                    version: None,
                });
            }
        }
        if (0..=127).contains(&y) {
            let Some(terrain) = self.legacy_terrain()? else {
                return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "chunk {:?} has no legacy terrain record",
                    self.pos
                )));
            };
            let local_y = u8::try_from(y).map_err(|_| {
                BedrockWorldError::Validation(format!("legacy block y={y} is outside 0..127"))
            })?;
            let id = terrain.block_id_at(x, local_y, z).ok_or_else(|| {
                BedrockWorldError::UnsupportedChunkFormat(format!(
                    "chunk {:?} has no legacy block id at local ({x}, {y}, {z})",
                    self.pos
                ))
            })?;
            let data = terrain.block_data_at(x, local_y, z).unwrap_or(0);
            let mut states = BTreeMap::new();
            states.insert("data".to_string(), NbtTag::Byte(data as i8));
            return Ok(BlockState {
                name: format!("legacy:{id}"),
                states,
                version: None,
            });
        }
        Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "chunk {:?} does not expose a block state at local ({x}, {y}, {z})",
            self.pos
        )))
    }

    /// Replaces a block in a structured editable chunk.
    pub fn set_block(&mut self, _x: u8, _y: i16, _z: u8, _block: BlockState) -> Result<()> {
        Err(BedrockWorldError::UnsupportedChunkFormat(
            "structured block editing is not enabled for this chunk format".to_string(),
        ))
    }

    /// Returns legacy inline entity records decoded from this chunk.
    pub fn get_entities(&self) -> Result<Vec<EntityData>> {
        let mut entities = Vec::new();
        for record in self
            .records
            .iter()
            .filter(|record| record.key.tag == ChunkRecordTag::Entity)
        {
            entities.extend(parse_consecutive_nbt(record.value.as_ref())?);
        }
        Ok(entities)
    }

    /// Returns block-entity records decoded from this chunk.
    pub fn get_block_entities(&self) -> Result<Vec<EntityData>> {
        let mut entities = Vec::new();
        for record in self
            .records
            .iter()
            .filter(|record| record.key.tag == ChunkRecordTag::BlockEntity)
        {
            entities.extend(parse_consecutive_nbt(record.value.as_ref())?);
        }
        Ok(entities)
    }
}

fn parse_consecutive_nbt(bytes: &[u8]) -> Result<Vec<EntityData>> {
    parse_consecutive_root_nbt(bytes)
        .map(|tags| tags.into_iter().map(|tag| EntityData { tag }).collect())
}
