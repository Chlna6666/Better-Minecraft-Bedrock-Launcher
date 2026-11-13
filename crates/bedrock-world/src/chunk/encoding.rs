//! Paletted SubChunk encoding shared by editing and historical migration.
//!
//! The indexed encoder accepts an existing [`BlockPalette`] so migration can rewrite palette entries
//! once instead of materialising 4096 owned `BlockState` values per storage layer.

use crate::block::{BlockPalette, BlockState};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::{NbtTag, NbtWriter};
use bytes::Bytes;
use indexmap::IndexMap;

const BLOCKS_PER_SUBCHUNK: usize = 4096;

pub(crate) fn encode_paletted_subchunk(
    version: u8,
    y: i8,
    palettes: &[&BlockPalette],
) -> Result<Bytes> {
    validate_storage_count(version, palettes.len())?;
    let mut bytes = subchunk_header(version, y, palettes.len())?;
    for palette in palettes {
        let indices = palette.surface_indices().ok_or_else(|| {
            BedrockWorldError::Validation(
                "palette encoding requires retained block indices".to_string(),
            )
        })?;
        bytes.extend_from_slice(&encode_palette_storage(&palette.states, indices.as_ref())?);
    }
    Ok(Bytes::from(bytes))
}

fn validate_storage_count(version: u8, count: usize) -> Result<()> {
    match version {
        1 if count == 1 => Ok(()),
        1 => Err(BedrockWorldError::Validation(format!(
            "paletted SubChunk v1 requires exactly one storage layer, got {count}"
        ))),
        8 | 9 if (1..=usize::from(u8::MAX)).contains(&count) => Ok(()),
        8 | 9 => Err(BedrockWorldError::Validation(format!(
            "SubChunk v{version} requires 1..={} storage layers, got {count}",
            u8::MAX
        ))),
        _ => Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "paletted SubChunk encoder supports versions 1, 8 and 9, got {version}"
        ))),
    }
}

fn subchunk_header(version: u8, y: i8, count: usize) -> Result<Vec<u8>> {
    Ok(match version {
        1 => vec![1],
        8 => vec![
            8,
            u8::try_from(count).map_err(|_| {
                BedrockWorldError::Validation("subchunk storage count overflowed u8".to_string())
            })?,
        ],
        9 => vec![
            9,
            u8::try_from(count).map_err(|_| {
                BedrockWorldError::Validation("subchunk storage count overflowed u8".to_string())
            })?,
            y.to_ne_bytes()[0],
        ],
        _ => unreachable!("validated above"),
    })
}

fn encode_palette_storage(states: &[BlockState], indices: &[u16]) -> Result<Vec<u8>> {
    if states.is_empty() || states.len() > BLOCKS_PER_SUBCHUNK {
        return Err(BedrockWorldError::Validation(format!(
            "palette must contain 1..={BLOCKS_PER_SUBCHUNK} states, got {}",
            states.len()
        )));
    }
    if indices.len() != BLOCKS_PER_SUBCHUNK {
        return Err(BedrockWorldError::Validation(format!(
            "palette index array has {} entries, expected {BLOCKS_PER_SUBCHUNK}",
            indices.len()
        )));
    }
    for state in states {
        validate_writable_state(state)?;
    }
    if let Some(invalid) = indices
        .iter()
        .copied()
        .find(|index| usize::from(*index) >= states.len())
    {
        return Err(BedrockWorldError::Validation(format!(
            "palette index {invalid} exceeds palette length {}",
            states.len()
        )));
    }

    let bits = bits_per_palette_index(states.len())?;
    let mut bytes = vec![bits << 1];
    if bits != 0 {
        for word in pack_palette_indices(indices, bits)? {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(
            &i32::try_from(states.len())
                .map_err(|_| BedrockWorldError::Validation("palette too large".to_string()))?
                .to_le_bytes(),
        );
    }
    for state in states {
        bytes.extend_from_slice(&NbtWriter::write_root(&storage_state_nbt(state)?)?);
    }
    Ok(bytes)
}

fn validate_writable_state(state: &BlockState) -> Result<()> {
    if state.name.trim().is_empty()
        || matches!(
            state.name.as_str(),
            "<invalid>" | "<unknown>" | "<unresolved-legacy>"
        )
    {
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
        1 => Ok(0),
        2 => Ok(1),
        3..=4 => Ok(2),
        5..=8 => Ok(3),
        9..=16 => Ok(4),
        17..=32 => Ok(5),
        33..=64 => Ok(6),
        65..=256 => Ok(8),
        257..=4096 => Ok(16),
        _ => Err(BedrockWorldError::Validation(format!(
            "invalid subchunk palette length: {palette_len}"
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
    use crate::chunk::{SubChunk, SubChunkDecodeMode, SubChunkFormat};
    use std::collections::BTreeMap;

    fn state(name: &str) -> BlockState {
        BlockState {
            name: name.to_string(),
            states: BTreeMap::new(),
            version: Some(18_168_865),
        }
    }

    #[test]
    fn indexed_v9_roundtrips_without_expanding_owned_states() {
        let palette = BlockPalette::with_unpacked_indices(
            vec![state("minecraft:air"), state("minecraft:stone")],
            (0..4096)
                .map(|index| if index == 123 { 1_u16 } else { 0_u16 })
                .collect(),
            None,
        );
        let encoded = encode_paletted_subchunk(9, -4, &[&palette]).unwrap();
        let parsed = SubChunk::read(-4, encoded, SubChunkDecodeMode::FullIndices).unwrap();
        let SubChunkFormat::Paletted { version, storages } = parsed.format else {
            panic!("expected paletted subchunk");
        };
        assert_eq!(version, 9);
        assert_eq!(storages[0].states.len(), 2);
    }

    #[test]
    fn paletted_v1_uses_single_storage_layout() {
        let palette =
            BlockPalette::with_unpacked_indices(vec![state("minecraft:air")], vec![0; 4096], None);
        let encoded = encode_paletted_subchunk(1, 0, &[&palette]).unwrap();
        assert_eq!(encoded[0], 1);
        assert_ne!(encoded.get(1).copied(), Some(1));
    }
}
