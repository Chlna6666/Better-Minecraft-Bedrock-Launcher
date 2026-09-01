use super::rgba::swap_rgba_pa_to_bgra;
use fearless_simd::{
    Level, Simd, dispatch, f32x16, prelude::*, u8x16, u16x8, u32x4, u32x8, u32x16,
};
use std::sync::{
    LazyLock,
    atomic::{AtomicBool, Ordering},
};

const VECTOR_MIN_BYTES: usize = 64;
const SIMD_PIXELS_PER_CHUNK: usize = 16;
const SIMD_BYTES_PER_CHUNK: usize = SIMD_PIXELS_PER_CHUNK * 4;
const PARALLEL_MIN_BYTES: usize = 16 * 1024 * 1024;
const PARALLEL_MIN_BYTES_PER_WORKER: usize = 4 * 1024 * 1024;
const MAX_PARALLEL_WORKERS: usize = 4;

static PARALLEL_PIXEL_CONVERSION_IN_USE: AtomicBool = AtomicBool::new(false);
static PIXEL_SIMD_LEVEL: LazyLock<Level> = LazyLock::new(Level::new);

struct ParallelPixelPermit;

impl ParallelPixelPermit {
    #[inline]
    fn try_acquire() -> Option<Self> {
        PARALLEL_PIXEL_CONVERSION_IN_USE
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for ParallelPixelPermit {
    fn drop(&mut self) {
        PARALLEL_PIXEL_CONVERSION_IN_USE.store(false, Ordering::Release);
    }
}

const fn build_alpha_norm_lut() -> [f32; 256] {
    let mut lut = [0.0; 256];
    let mut index = 0usize;
    while index < lut.len() {
        lut[index] = index as f32 / 255.0;
        index += 1;
    }
    lut
}

const fn build_alpha_divisor_lut() -> [f32; 256] {
    let mut lut = build_alpha_norm_lut();
    // A fully transparent premultiplied pixel has zero color channels. Dividing by one preserves
    // those zeros and avoids a special branch in the SIMD kernel; the original alpha channel is
    // carried through untouched.
    lut[0] = 1.0;
    lut
}

// Match the historical scalar denominator exactly. A reciprocal or algebraic rewrite changes a
// small number of 8-bit results by one LSB, so the safe SIMD kernel still uses the exact table.
const ALPHA_NORM_LUT: [f32; 256] = build_alpha_norm_lut();
const ALPHA_DIVISOR_LUT: [f32; 256] = build_alpha_divisor_lut();

/// Converts an in-place RGBA premultiplied-alpha pixel buffer to BGRA straight alpha.
///
/// Medium and large buffers use Fearless SIMD runtime multiversioning. Very large buffers may be
/// split across a small number of scoped workers, and each worker still dispatches to the strongest
/// available safe SIMD backend. Only one conversion may fan out at a time; concurrent huge
/// conversions fall back immediately to single-threaded SIMD instead of oversubscribing the
/// process. Small buffers retain the historical scalar conversion. Trailing bytes that do not form
/// a complete RGBA pixel are left untouched.
pub(crate) fn swap_rgba_pa_to_bgra_buffer(buffer: &mut [u8]) {
    let pixel_bytes = buffer.len() & !3;
    let (pixels, _) = buffer.split_at_mut(pixel_bytes);
    let workers = parallel_worker_count(pixels.len());
    if workers > 1 {
        if let Some(_permit) = ParallelPixelPermit::try_acquire() {
            parallel_buffer(pixels, workers);
            return;
        }
    }
    serial_buffer(pixels);
}

#[inline]
fn parallel_worker_count(buffer_len: usize) -> usize {
    let cpus = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    parallel_worker_count_for(buffer_len, cpus)
}

#[inline]
fn parallel_worker_count_for(buffer_len: usize, cpus: usize) -> usize {
    if buffer_len < PARALLEL_MIN_BYTES || cpus < 3 {
        return 1;
    }

    // Leave one logical CPU available for the UI/platform thread and cap fan-out so a large image
    // decode cannot monopolize high-core-count desktops. Require enough bytes per worker to amortize
    // scoped-thread creation and cache handoff costs.
    cpus.saturating_sub(1)
        .min(MAX_PARALLEL_WORKERS)
        .min(buffer_len / PARALLEL_MIN_BYTES_PER_WORKER)
        .max(1)
}

fn parallel_buffer(buffer: &mut [u8], workers: usize) {
    debug_assert!(workers > 1);
    // Keep every chunk aligned to both whole RGBA pixels and a cache line. The final chunk may be
    // shorter but the caller already removed any incomplete pixel tail.
    let target = buffer.len().div_ceil(workers);
    let chunk_bytes = target.saturating_add(63) & !63;
    std::thread::scope(|scope| {
        for chunk in buffer.chunks_mut(chunk_bytes) {
            scope.spawn(move || serial_buffer(chunk));
        }
    });
}

#[inline]
fn serial_buffer(buffer: &mut [u8]) {
    if buffer.len() < VECTOR_MIN_BYTES {
        scalar_buffer(buffer);
        return;
    }

    let level = *PIXEL_SIMD_LEVEL;
    if level.is_fallback() {
        scalar_buffer(buffer);
        return;
    }

    // Keep AVX-512 out of the default UI path. Fearless SIMD still supplies the safe runtime
    // feature proof, while AVX2 avoids the frequency and power trade-offs of wide AVX-512 code.
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if let Some(avx2) = level.as_avx2() {
        dispatch!(Level::Avx2(avx2), simd => simd_buffer(simd, buffer));
        return;
    }

    dispatch!(level, simd => simd_buffer(simd, buffer));
}

#[inline(always)]
fn scalar_buffer(buffer: &mut [u8]) {
    for pixel in buffer.chunks_exact_mut(4) {
        swap_rgba_pa_to_bgra(pixel);
    }
}

#[inline(always)]
fn simd_buffer<S: Simd>(simd: S, buffer: &mut [u8]) {
    let mut chunks = buffer.chunks_exact_mut(SIMD_BYTES_PER_CHUNK);
    for chunk in &mut chunks {
        let [red, green, blue, alpha] = u8x16::load_four_interleaved(simd, chunk);
        let alpha_wide = widen_u8_to_u32x16(alpha);
        let divisor = f32x16::from_fn(simd, |lane| ALPHA_DIVISOR_LUT[alpha_wide[lane] as usize]);

        let red = unpremultiply_channel(simd, red, divisor);
        let green = unpremultiply_channel(simd, green, divisor);
        let blue = unpremultiply_channel(simd, blue, divisor);

        // Preserve the original alpha bytes and write BGRA directly from the deinterleaved vectors.
        u8x16::store_four_interleaved([blue, green, red, alpha], chunk);
    }

    scalar_buffer(chunks.into_remainder());
}

#[inline(always)]
fn widen_u8_to_u32x16<S: Simd>(value: u8x16<S>) -> u32x16<S> {
    let (low16, high16) = value.widen();
    let (q0, q1) = low16.widen();
    let (q2, q3) = high16.widen();
    let low32 = q0.combine(q1);
    let high32 = q2.combine(q3);
    low32.combine(high32)
}

#[inline(always)]
fn narrow_u32x16_to_u8<S: Simd>(value: u32x16<S>) -> u8x16<S> {
    let (low32, high32): (u32x8<S>, u32x8<S>) = value.split();
    let (q0, q1): (u32x4<S>, u32x4<S>) = low32.split();
    let (q2, q3): (u32x4<S>, u32x4<S>) = high32.split();
    let low16: u16x8<S> = q0.saturating_narrow(q1);
    let high16: u16x8<S> = q2.saturating_narrow(q3);
    low16.saturating_narrow(high16)
}

#[inline(always)]
fn unpremultiply_channel<S: Simd>(simd: S, channel: u8x16<S>, divisor: f32x16<S>) -> u8x16<S> {
    let channel_wide = widen_u8_to_u32x16(channel);
    let channel_f: f32x16<S> = channel_wide.to_float();
    let straight: u32x16<S> = (channel_f / divisor).to_int_precise();
    let _ = simd;
    narrow_u32x16_to_u8(straight)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar_reference(mut input: Vec<u8>) -> Vec<u8> {
        scalar_buffer(&mut input);
        input
    }

    #[test]
    fn buffer_conversion_matches_scalar_for_all_alpha_values() {
        let mut input = Vec::with_capacity(256 * 4 * 4);
        for alpha in 0..=255u8 {
            for seed in [0u8, 37, 127, 255] {
                let red = seed.min(alpha);
                let green = seed.wrapping_mul(3).min(alpha);
                let blue = seed.wrapping_mul(7).min(alpha);
                input.extend_from_slice(&[red, green, blue, alpha]);
            }
        }
        let expected = scalar_reference(input.clone());
        swap_rgba_pa_to_bgra_buffer(&mut input);
        assert_eq!(input, expected);
    }

    #[test]
    fn vector_conversion_saturates_non_premultiplied_inputs() {
        let mut input = Vec::with_capacity(256 * 4 * 4);
        for alpha in 0..=255u8 {
            for seed in [0u8, 37, 127, 255] {
                input.extend_from_slice(&[
                    seed,
                    seed.wrapping_mul(3),
                    255u8.wrapping_sub(seed),
                    alpha,
                ]);
            }
        }
        let expected = scalar_reference(input.clone());
        swap_rgba_pa_to_bgra_buffer(&mut input);
        assert_eq!(input, expected);
    }

    #[test]
    fn alpha_lut_matches_scalar_denominator() {
        for alpha in 0..=255u8 {
            assert_eq!(
                ALPHA_NORM_LUT[usize::from(alpha)].to_bits(),
                (alpha as f32 / 255.0).to_bits()
            );
            assert_eq!(
                ALPHA_DIVISOR_LUT[usize::from(alpha)].to_bits(),
                (if alpha == 0 {
                    1.0
                } else {
                    alpha as f32 / 255.0
                })
                .to_bits()
            );
        }
    }

    #[test]
    fn parallel_policy_keeps_small_and_low_core_work_serial() {
        assert_eq!(parallel_worker_count_for(PARALLEL_MIN_BYTES - 4, 16), 1);
        assert_eq!(parallel_worker_count_for(PARALLEL_MIN_BYTES, 2), 1);
    }

    #[test]
    fn parallel_policy_caps_workers_and_leaves_one_cpu_free() {
        assert_eq!(parallel_worker_count_for(PARALLEL_MIN_BYTES, 4), 3);
        assert_eq!(parallel_worker_count_for(64 * 1024 * 1024, 32), 4);
    }

    #[test]
    fn parallel_permit_is_non_blocking_and_exclusive() {
        let first = ParallelPixelPermit::try_acquire().expect("first permit should succeed");
        assert!(ParallelPixelPermit::try_acquire().is_none());
        drop(first);
        assert!(ParallelPixelPermit::try_acquire().is_some());
    }

    #[test]
    fn parallel_chunks_match_scalar_reference() {
        const TEST_BYTES: usize = 256 * 1024;
        let mut input = Vec::with_capacity(TEST_BYTES);
        while input.len() < TEST_BYTES {
            let alpha = ((input.len() / 4) % 256) as u8;
            input.extend_from_slice(&[
                alpha / 3,
                alpha / 2,
                alpha.saturating_sub(alpha / 4),
                alpha,
            ]);
        }
        let expected = scalar_reference(input.clone());
        parallel_buffer(&mut input, 4);
        assert_eq!(input, expected);
    }

    #[test]
    fn buffer_conversion_leaves_incomplete_tail_untouched() {
        let mut input = vec![10, 20, 30, 40, 7, 8];
        swap_rgba_pa_to_bgra_buffer(&mut input);
        assert_eq!(&input[4..], &[7, 8]);
    }
}
