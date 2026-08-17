//! Modern paletted SubChunk encoder shared by editing and migration.

use crate::codec::nbt::{NbtTag, NbtWriter};
use crate::error::{BedrockWorldError, Result};
use crate::model::BlockState;
use bytes::Bytes;
use indexmap::IndexMap;
use std::collections::BTreeMap;

const BLOCKS_PER_SUBCHUNK: usize = 4096;

/// Encodes one modern Bedrock paletted subchunk.
///
/// `layers` are supplied in Bedrock storage order and each layer must contain exactly 4096 semantic
/// block states. Versions 8 and 9 are supported. Version 9 writes the explicit subchunk Y byte.
pub fn encode_paletted_subchunk(
    version: u8,
    y: i8,
    layers: &[&[BlockState]],
) -> Result<Bytes> {
    if !matches!(version, 8 | 9) {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "modern paletted subchunk encoder supports versions 8/9, got {version}"
        )));
    }
    if layers.is_empty() || layers.len() > usize::from(u8::MAX) {
        return Err(BedrockWorldError::Validation(format!(
            "subchunk must contain 1..={} storage layers, got {}",
            u8::MAX,
            layers.len()
        )));
    }
    let storage_count = u8::try_from(layers.len()).map_err(|_| {
        BedrockWorldError::Validation("subchunk storage count overflowed u8".to_string())
    })?;
    let mut bytes = match version {
        8 => vec![8, storage_count],
        9 => vec![9, storage_count, y.to_ne_bytes()[0]],
        _ => unreachable!(),
    };
    for layer in layers {
        bytes.extend_from_slice(&encode_palette_layer(layer)?);
    }
    Ok(Bytes::from(bytes))
}

/// Encodes one 4096-block palette storage layer.
pub fn encode_palette_layer(states: &[BlockState]) -> Result<Vec<u8>> {
    if states.len() != BLOCKS_PER_SUBCHUNK {
        return Err(BedrockWorldError::Validation(format!(
            "subchunk layer has {} blocks, expected {BLOCKS_PER_SUBCHUNK}",
            states.len()
        )));
    }
    let mut palette = Vec::<BlockState>::new();
    let mut lookup = BTreeMap::<Vec<u8>, u16>::new();
    let mut indices = Vec::<u16>::with_capacity(BLOCKS_PER_SUBCHUNK);
    for state in states {
        validate_writable_state(state)?;
        let key = storage_identity_bytes(state)?;
        let index = if let Some(index) = lookup.get(&key) {
            *index
        } else {
            let index = u16::try_from(palette.len()).map_err(|_| {
                BedrockWorldError::Validation("subchunk palette exceeds u16".to_string())
            })?;
            palette.push(state.clone());
            lookup.insert(key, index);
            index
        };
        indices.push(index);
    }

    let bits = bits_per_palette_index(palette.len())?;
    let mut bytes = vec![bits << 1];
    if bits != 0 {
        for word in pack_palette_indices(&indices, bits)? {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(
            &i32::try_from(palette.len())
                .map_err(|_| BedrockWorldError::Validation("palette too large".to_string()))?
                .to_le_bytes(),
        );
    }
    for state in &palette {
        bytes.extend_from_slice(&NbtWriter::write_root(&storage_state_nbt(state)?)?);
    }
    Ok(bytes)
}

fn validate_writable_state(state: &BlockState) -> Result<()> {
    if state.name.trim().is_empty() || matches!(state.name.as_str(), "<invalid>" | "<unknown>") {
        return Err(BedrockWorldError::Validation(
            "cannot encode a block state with an invalid identifier".to_string(),
        ));
    }
    if state.version.is_none() {
        return Err(BedrockWorldError::Validation(format!(
            "block state {} has no storage version",
            state.name
        )));
    }
    state.canonical_bytes()?;
    Ok(())
}

fn storage_identity_bytes(state: &BlockState) -> Result<Vec<u8>> {
    let mut bytes = state.canonical_bytes()?;
    let version = state.version.ok_or_else(|| {
        BedrockWorldError::Validation(format!("block state {} has no version", state.name))
    })?;
    bytes.extend_from_slice(&version.to_le_bytes());
    Ok(bytes)
}

fn storage_state_nbt(state: &BlockState) -> Result<NbtTag> {
    let version = state.version.ok_or_else(|| {
        BedrockWorldError::Validation(format!("block state {} has no version", state.name))
    })?;
    Ok(NbtTag::Compound(IndexMap::from([
        ("name".to_string(), NbtTag::String(state.name.clone())),
        (
            "states".to_string(),
            NbtTag::Compound(
                state
                    .states
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
            ),
        ),
        ("version".to_string(), NbtTag::Int(version)),
    ])))
}

fn bits_per_palette_index(palette_len: usize) -> Result<u8> {
    match palette_len {
        0 | 1 => Ok(0),
        2 => Ok(1),
        3..=4 => Ok(2),
        5..=8 => Ok(3),
        9..=16 => Ok(4),
        17..=32 => Ok(5),
        33..=64 => Ok(6),
        65..=256 => Ok(8),
        257..=4096 => Ok(16),
        _ => Err(BedrockWorldError::Validation(format!(
            "subchunk palette length exceeds 4096: {palette_len}"
        ))),
    }
}

fn pack_palette_indices(indices: &[u16], bits: u8) -> Result<Vec<u32>> {
    if bits == 0 {
        return Ok(Vec::new());
    }
    let values_per_word = usize::from(32 / bits);
    let mask = (1_u32 << bits) - 1;
    let mut words = vec![0_u32; indices.len().div_ceil(values_per_word)];
    for (index, value) in indices.iter().copied().enumerate() {
        if u32::from(value) > mask {
            return Err(BedrockWorldError::Validation(format!(
                "palette index {value} does not fit {bits} bits"
            )));
        }
        words[index / values_per_word] |=
            u32::from(value) << ((index % values_per_word) * usize::from(bits));
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::subchunk::{SubChunkDecodeMode, SubChunkFormat, parse_subchunk_with_mode};

    #[test]
    fn encoded_v9_roundtrips() {
        let air = BlockState {
            name: "minecraft:air".to_string(),
            states: BTreeMap::new(),
            version: Some(18_168_865),
        };
        let stone = BlockState {
            name: "minecraft:stone".to_string(),
            states: BTreeMap::new(),
            version: Some(18_168_865),
        };
        let mut layer = vec![air; 4096];
        layer[123] = stone;
        let encoded = encode_paletted_subchunk(9, -4, &[&layer]).unwrap();
        let parsed = parse_subchunk_with_mode(-4, encoded, SubChunkDecodeMode::FullIndices).unwrap();
        let SubChunkFormat::Paletted { version, storages } = parsed.format else {
            panic!("expected paletted subchunk");
        };
        assert_eq!(version, 9);
        assert_eq!(storages.len(), 1);
        assert_eq!(storages[0].states.len(), 2);
    }
}
