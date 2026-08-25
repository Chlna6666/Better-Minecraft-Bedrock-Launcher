//! Allocation-light Data3D encoder for quart-resolution biome producers.
//!
//! Java-style generators naturally produce 4x4x4 biome samples (64 values per 16-block section).
//! The generic parsed-model encoder accepts 4096 unpacked indices, which forced callers to expand
//! every quart sample 64 times into a short-lived Vec before the same values were packed again. This
//! encoder performs that expansion logically while writing packed Bedrock words and never
//! materializes the 4096-entry index array.

use crate::error::{BedrockWorldError, Result};

const QUART_SIDE: usize = 4;
const QUART_VOLUME: usize = QUART_SIDE * QUART_SIDE * QUART_SIDE;
const BLOCK_VOLUME: usize = 16 * 16 * 16;

/// Encodes a complete Data3D payload directly from one 64-entry quart-biome array per subchunk.
///
/// `height_map` uses Bedrock `z * 16 + x` order. Each quart storage uses
/// `(quart_y * 4 + quart_z) * 4 + quart_x`, matching Java's 4x4x4 biome-cell layout. The emitted
/// packed indices use Bedrock's `x * 256 + z * 16 + y` block-storage order.
pub fn encode_data3d_quart(
    height_map: &[i16; 256],
    storages: &[[u32; QUART_VOLUME]],
) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(512 + storages.len().saturating_mul(320));
    for height in height_map {
        bytes.extend_from_slice(&height.to_le_bytes());
    }
    for storage in storages {
        encode_quart_storage(&mut bytes, storage)?;
    }
    Ok(bytes)
}

fn encode_quart_storage(bytes: &mut Vec<u8>, quart: &[u32; QUART_VOLUME]) -> Result<()> {
    // A quart storage can contain at most 64 distinct biomes. Fixed arrays avoid both hashing and
    // allocator traffic; the O(64^2) worst-case palette build is only 4096 integer comparisons.
    let mut palette = [0_u32; QUART_VOLUME];
    let mut quart_palette = [0_u8; QUART_VOLUME];
    let mut palette_len = 0usize;

    for (slot, id) in quart.iter().copied().enumerate() {
        let palette_index = match palette[..palette_len]
            .iter()
            .position(|current| *current == id)
        {
            Some(index) => index,
            None => {
                let index = palette_len;
                palette[index] = id;
                palette_len += 1;
                index
            }
        };
        quart_palette[slot] = palette_index as u8;
    }

    debug_assert!(palette_len > 0 && palette_len <= QUART_VOLUME);
    if palette_len == 1 {
        bytes.push(0);
        write_biome_id(bytes, palette[0])?;
        return Ok(());
    }

    let bits = bits_per_palette_index(palette_len)?;
    bytes.push(bits << 1);
    pack_quart_indices(bytes, &quart_palette, bits);

    let palette_len_i32 = i32::try_from(palette_len).map_err(|_| {
        BedrockWorldError::Validation("biome palette length does not fit i32".to_string())
    })?;
    bytes.extend_from_slice(&palette_len_i32.to_le_bytes());
    for id in palette[..palette_len].iter().copied() {
        write_biome_id(bytes, id)?;
    }
    Ok(())
}

#[inline]
fn write_biome_id(bytes: &mut Vec<u8>, id: u32) -> Result<()> {
    let id = i32::try_from(id)
        .map_err(|_| BedrockWorldError::Validation("biome id does not fit i32".to_string()))?;
    bytes.extend_from_slice(&id.to_le_bytes());
    Ok(())
}

fn bits_per_palette_index(palette_len: usize) -> Result<u8> {
    let max_index = palette_len.saturating_sub(1);
    for bits in [1_u8, 2, 3, 4, 5, 6, 8, 16] {
        if max_index < (1_usize << bits) {
            return Ok(bits);
        }
    }
    Err(BedrockWorldError::Validation(format!(
        "biome palette length {palette_len} exceeds encodable range"
    )))
}

fn pack_quart_indices(bytes: &mut Vec<u8>, quart_palette: &[u8; QUART_VOLUME], bits: u8) {
    debug_assert!(matches!(bits, 1 | 2 | 3 | 4 | 5 | 6 | 8 | 16));
    let values_per_word = usize::from(32 / bits);
    let word_count = BLOCK_VOLUME.div_ceil(values_per_word);
    let mask = (1_u32 << bits) - 1;
    bytes.reserve(word_count.saturating_mul(4));

    for word_index in 0..word_count {
        let first_block = word_index * values_per_word;
        let mut word = 0_u32;
        for offset in 0..values_per_word {
            let block_index = first_block + offset;
            if block_index == BLOCK_VOLUME {
                break;
            }
            // Bedrock storage index is x * 256 + z * 16 + y.
            let x = block_index >> 8;
            let z = (block_index >> 4) & 15;
            let y = block_index & 15;
            let quart_index = ((y >> 2) * QUART_SIDE + (z >> 2)) * QUART_SIDE + (x >> 2);
            let value = u32::from(quart_palette[quart_index]);
            debug_assert!(value <= mask);
            word |= value << (offset * usize::from(bits));
        }
        bytes.extend_from_slice(&word.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsed::{Biome3d, ParsedBiomeStorage};

    fn expanded_storage(y: i32, quart: &[u32; 64]) -> ParsedBiomeStorage {
        let mut palette = Vec::<u32>::new();
        let mut quart_palette = [0_u16; 64];
        for (slot, id) in quart.iter().copied().enumerate() {
            let index = palette
                .iter()
                .position(|current| *current == id)
                .unwrap_or_else(|| {
                    palette.push(id);
                    palette.len() - 1
                });
            quart_palette[slot] = index as u16;
        }
        let mut indices = vec![0_u16; 4096];
        for x in 0..16usize {
            for z in 0..16usize {
                for local_y in 0..16usize {
                    let block_index = x * 256 + z * 16 + local_y;
                    let quart_index = ((local_y >> 2) * 4 + (z >> 2)) * 4 + (x >> 2);
                    indices[block_index] = quart_palette[quart_index];
                }
            }
        }
        let mut counts = vec![0_u16; palette.len()];
        for index in &indices {
            counts[usize::from(*index)] += 1;
        }
        ParsedBiomeStorage {
            y: Some(y),
            palette,
            indices: Some(indices),
            counts,
        }
    }

    #[test]
    fn quart_encoder_matches_generic_data3d_encoding() {
        let heights = [73_i16; 256];
        let mut first = [1_u32; 64];
        let mut second = [4_u32; 64];
        for index in (0..64).step_by(3) {
            first[index] = 16;
        }
        for index in (0..64).step_by(5) {
            second[index] = 7;
        }
        let quart = [first, second];
        let direct = encode_data3d_quart(&heights, &quart).expect("quart encode");
        let generic = Biome3d::new(
            heights.to_vec(),
            vec![
                expanded_storage(-64, &first),
                expanded_storage(-48, &second),
            ],
        )
        .expect("generic model")
        .encode()
        .expect("generic encode");
        assert_eq!(direct, generic);
    }

    #[test]
    fn uniform_storage_is_five_bytes() {
        let mut bytes = Vec::new();
        encode_quart_storage(&mut bytes, &[42_u32; 64]).expect("uniform");
        assert_eq!(bytes.len(), 5);
        assert_eq!(bytes[0], 0);
        assert_eq!(&bytes[1..], &42_i32.to_le_bytes());
    }
}
