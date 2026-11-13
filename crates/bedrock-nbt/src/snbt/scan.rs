//! Scan string runs without interpreting UTF-8 continuation bytes as syntax.

#[inline]
pub(super) fn string_run(bytes: &[u8], quote: u8) -> usize {
    #[cfg(feature = "simd")]
    if bytes.len() >= 256 {
        // Check a short prefix first: a tiny string may be followed by a very
        // large document. Remaining input length alone must not trigger SIMD.
        let prefix = string_run_scalar(&bytes[..64], quote);
        if prefix < 64 {
            return prefix;
        }
        return 64
            + fearless_simd::dispatch!(fearless_simd::Level::new(), simd => {
                string_run_simd(simd, &bytes[64..], quote)
            });
    }
    string_run_scalar(bytes, quote)
}

fn string_run_scalar(bytes: &[u8], quote: u8) -> usize {
    bytes
        .iter()
        .position(|byte| *byte == quote || *byte == b'\\')
        .unwrap_or(bytes.len())
}

#[cfg(feature = "simd")]
fn string_run_simd<S: fearless_simd::Simd>(simd: S, bytes: &[u8], quote: u8) -> usize {
    use fearless_simd::prelude::*;
    // Keep loads, comparison and reduction in the detected target-feature context.
    simd.vectorize(|| {
        let mut chunks = bytes.chunks_exact(S::u8s::LEN);
        let mut offset = 0;
        for chunk in &mut chunks {
            let values = S::u8s::from_slice(simd, chunk);
            let mask = (values.simd_eq(S::u8s::splat(simd, quote))
                | values.simd_eq(S::u8s::splat(simd, b'\\')))
            .to_bitmask();
            if mask != 0 {
                return offset + mask.trailing_zeros() as usize;
            }
            offset += S::u8s::LEN;
        }
        offset + string_run_scalar(chunks.remainder(), quote)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_scanner_matches_scalar_at_every_offset_and_alignment() {
        for quote in [b'\'', b'"'] {
            for special in [quote, b'\\'] {
                for len in 0..260 {
                    let mut bytes = vec![b'x'; len + 32];
                    for alignment in 0..32 {
                        for offset in 0..=len {
                            if offset < len {
                                bytes[alignment + offset] = special;
                            }
                            let input = &bytes[alignment..alignment + len];
                            assert_eq!(string_run(input, quote), string_run_scalar(input, quote));
                            if offset < len {
                                bytes[alignment + offset] = b'x';
                            }
                        }
                    }
                }
            }
        }
    }
}
