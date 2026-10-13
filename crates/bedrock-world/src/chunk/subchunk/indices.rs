use crate::error::{BedrockWorldError, Result};

#[cfg(all(feature = "simd", any(target_arch = "x86", target_arch = "x86_64")))]
use fearless_simd::{Avx2, Level, Simd, dispatch, prelude::*, u32x8};
#[cfg(all(feature = "simd", any(target_arch = "x86", target_arch = "x86_64")))]
static AVX2: std::sync::LazyLock<Option<Avx2>> =
    std::sync::LazyLock::new(|| Level::new().as_avx2());

pub(super) fn unpack(words: &[u8], bits: u8, palette_len: usize) -> Result<Vec<u16>> {
    // Only this complete 4096-index/4-bit AVX2 path has a measured crossover.
    // Other widths, CPUs and truncated input retain the original decoder.
    #[cfg(all(feature = "simd", any(target_arch = "x86", target_arch = "x86_64")))]
    if bits == 4
        && words.len() >= 2048
        && let Some(avx2) = *AVX2
    {
        return dispatch!(Level::Avx2(avx2), simd => four_bit(simd, words, palette_len));
    }
    scalar(words, bits, palette_len)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "caller validates Bedrock widths at most 16 bits before unpacking"
)]
pub(super) fn scalar(words: &[u8], bits: u8, palette_len: usize) -> Result<Vec<u16>> {
    if bits == 0 {
        return Ok(vec![0; 4096]);
    }
    let values_per_word = usize::from(32 / bits);
    let mask = (1_u32 << bits) - 1;
    let mut indices = Vec::with_capacity(4096);
    for word_bytes in words.as_chunks::<4>().0 {
        let word = u32::from_le_bytes(*word_bytes);
        for item_index in 0..values_per_word {
            if indices.len() == 4096 {
                break;
            }
            let value = ((word >> (item_index * usize::from(bits))) & mask) as u16;
            if palette_len > 0 && usize::from(value) >= palette_len {
                return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "palette index {value} exceeds palette length {palette_len}"
                )));
            }
            indices.push(value);
        }
    }
    if indices.len() != 4096 {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "palette produced {} block indices instead of 4096",
            indices.len()
        )));
    }
    Ok(indices)
}

#[cfg(all(feature = "simd", any(target_arch = "x86", target_arch = "x86_64")))]
#[expect(clippy::inline_always, reason = "fearless dispatch ISA boundary")]
#[inline(always)]
fn four_bit<S: Simd>(simd: S, words: &[u8], palette_len: usize) -> Result<Vec<u16>> {
    let shifts = u32x8::from_slice(simd, &[0, 4, 8, 12, 16, 20, 24, 28]);
    let limit = u32x8::splat(
        simd,
        u32::try_from(palette_len.min(16)).expect("bounded by 16"),
    );
    let mut indices = vec![0; 4096];
    for (chunk, output) in words
        .as_chunks::<8>()
        .0
        .iter()
        .zip(indices.as_chunks_mut::<16>().0)
    {
        let low_word = u32::from_le_bytes(chunk[..4].try_into().expect("exact four-byte range"));
        let high_word = u32::from_le_bytes(chunk[4..].try_into().expect("exact four-byte range"));
        let low = (u32x8::splat(simd, low_word) >> shifts) & 15;
        let high = (u32x8::splat(simd, high_word) >> shifts) & 15;
        if (1..16).contains(&palette_len) && (low.simd_ge(limit) | high.simd_ge(limit)).any_true() {
            // Preserve the scalar decoder's first invalid index and error text.
            return scalar(words, 4, palette_len);
        }
        // Explicit vector narrowing/store also works with release opt-level=s;
        // array::map narrowing depended on opt-level=3 autovectorization.
        low.narrow(high).store_slice(output);
    }
    Ok(indices)
}

#[cfg(test)]
mod tests {
    use super::{scalar, unpack};

    #[test]
    fn four_bit_matches_scalar_values_lengths_and_first_error() {
        let bytes: Vec<u8> = (0..2057)
            .map(|index| u8::try_from(index % 256).expect("remainder fits u8"))
            .collect();
        for alignment in 0..8 {
            for length in [0, 1, 4, 2043, 2047, 2048, 2049] {
                for palette_len in 0..=17 {
                    let words = &bytes[alignment..alignment + length];
                    let canonical =
                        |result: crate::Result<Vec<u16>>| result.map_err(|error| error.to_string());
                    assert_eq!(
                        canonical(unpack(words, 4, palette_len)),
                        canonical(scalar(words, 4, palette_len))
                    );
                }
            }
        }
        for position in [0, 15, 127, 2047] {
            let mut words = vec![0; 2048];
            words[position] = 0x10;
            assert_eq!(
                unpack(&words, 4, 1).unwrap_err().to_string(),
                scalar(&words, 4, 1).unwrap_err().to_string()
            );
        }
    }
}
