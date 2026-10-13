#[cfg(feature = "simd")]
use fearless_simd::{Level, Simd, dispatch, prelude::*, u8x16};
#[cfg(feature = "simd")]
use std::sync::LazyLock;

#[cfg(feature = "simd")]
static LEVEL: LazyLock<Level> = LazyLock::new(|| {
    let level = Level::new();
    // Match the existing BMCBL policy: use AVX2 rather than AVX-512 on x86.
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if let Some(avx2) = level.as_avx2() {
        return Level::Avx2(avx2);
    }
    level
});

#[inline]
pub(crate) fn contains_forbidden(name: &str) -> bool {
    #[cfg(feature = "simd")]
    if name.len() >= 16 {
        let level = *LEVEL;
        if !level.is_fallback() {
            return dispatch!(level, simd => scan(simd, name.as_bytes()));
        }
    }
    name.contains(['\0', ':', '\\'])
}

// Required by fearless_simd's manual-inlining contract so dispatch! establishes
// the instruction-set boundary for the generic vector operations.
#[cfg(feature = "simd")]
#[expect(
    clippy::inline_always,
    reason = "fearless_simd manual inlining keeps operations inside dispatch's ISA boundary"
)]
#[inline(always)]
fn scan<S: Simd>(simd: S, bytes: &[u8]) -> bool {
    let zero = u8x16::splat(simd, 0);
    let colon = u8x16::splat(simd, b':');
    let backslash = u8x16::splat(simd, b'\\');
    let (chunks, remainder) = bytes.as_chunks::<16>();
    for chunk in chunks {
        let vector = u8x16::from_slice(simd, chunk);
        if (vector.simd_eq(zero) | vector.simd_eq(colon) | vector.simd_eq(backslash)).any_true() {
            return true;
        }
    }
    remainder
        .iter()
        .any(|byte| matches!(byte, 0 | b':' | b'\\'))
}

#[cfg(test)]
mod tests {
    use super::contains_forbidden;

    #[test]
    fn all_alignments_tails_and_reserved_positions_match_scalar() {
        let mut bytes = vec![b'a'; 320];
        for alignment in 0..32 {
            for length in 0..=247 {
                let slice = &mut bytes[alignment..alignment + length];
                assert!(!contains_forbidden(std::str::from_utf8(slice).unwrap()));
                for position in 0..length {
                    for forbidden in [0, b':', b'\\'] {
                        slice[position] = forbidden;
                        assert!(contains_forbidden(std::str::from_utf8(slice).unwrap()));
                    }
                    slice[position] = b'a';
                }
            }
        }
    }

    #[test]
    fn unicode_text_does_not_match_reserved_ascii_characters() {
        for length in 0..80 {
            assert!(!contains_forbidden(&"材质é".repeat(length)));
        }
    }
}
