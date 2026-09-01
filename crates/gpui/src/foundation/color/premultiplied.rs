use super::rgba::swap_rgba_pa_to_bgra;
use crate::{CpuVectorLevel, cpu_vector_level};
use std::sync::atomic::{AtomicBool, Ordering};

const VECTOR_MIN_BYTES: usize = 64;
const PARALLEL_MIN_BYTES: usize = 16 * 1024 * 1024;
const PARALLEL_MIN_BYTES_PER_WORKER: usize = 4 * 1024 * 1024;
const MAX_PARALLEL_WORKERS: usize = 4;

static PARALLEL_PIXEL_CONVERSION_IN_USE: AtomicBool = AtomicBool::new(false);

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

// Match the historical scalar denominator exactly while avoiding a first divide inside every
// vector loop. Using a reciprocal LUT would change a small number of 8-bit results by one LSB.
const ALPHA_NORM_LUT: [f32; 256] = build_alpha_norm_lut();

/// Converts an in-place RGBA premultiplied-alpha pixel buffer to BGRA straight alpha.
///
/// Medium and large buffers use an ISA-specific vector path. Very large buffers may be split across
/// a small number of scoped workers, and each worker still uses the best available SIMD kernel.
/// Only one conversion may fan out at a time; concurrent huge conversions fall back immediately to
/// single-threaded SIMD instead of oversubscribing the process. Small buffers and unsupported CPUs
/// retain the exact scalar conversion used historically by GPUI. Trailing bytes that do not form a
/// complete RGBA pixel are left untouched.
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

    match cpu_vector_level() {
        #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
        CpuVectorLevel::Avx2 => unsafe { avx2_buffer(buffer) },
        #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
        CpuVectorLevel::Sse2 => unsafe { sse2_buffer(buffer) },
        #[cfg(target_arch = "aarch64")]
        CpuVectorLevel::Neon => unsafe { neon_buffer(buffer) },
        _ => scalar_buffer(buffer),
    }
}

#[inline(always)]
fn scalar_buffer(buffer: &mut [u8]) {
    for pixel in buffer.chunks_exact_mut(4) {
        swap_rgba_pa_to_bgra(pixel);
    }
}

#[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
#[target_feature(enable = "avx2")]
unsafe fn avx2_buffer(buffer: &mut [u8]) {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    unsafe {
        let mut offset = 0usize;
        let alpha_indices = _mm256_set_epi32(7, 7, 7, 7, 3, 3, 3, 3);
        let bgra_indices = _mm256_set_epi32(7, 4, 5, 6, 3, 0, 1, 2);
        let one = _mm256_set1_ps(1.0);
        let zero_i = _mm256_setzero_si256();
        let min_i = _mm256_setzero_si256();
        let max_i = _mm256_set1_epi32(255);

        // Eight input bytes are two RGBA pixels. Expanding them to eight i32/f32 lanes lets one
        // AVX2 division unpremultiply both pixels at once while preserving each alpha channel.
        while offset + 8 <= buffer.len() {
            let source = buffer.as_mut_ptr().add(offset);
            let packed = std::ptr::read_unaligned(source.cast::<u64>());
            let bytes = _mm_cvtsi64_si128(packed as i64);
            let rgba_i = _mm256_cvtepu8_epi32(bytes);
            let alpha_i = _mm256_permutevar8x32_epi32(rgba_i, alpha_indices);
            let alpha_zero = _mm256_cmpeq_epi32(alpha_i, zero_i);
            let rgba_f = _mm256_cvtepi32_ps(rgba_i);
            let alpha0 = ALPHA_NORM_LUT[usize::from(*source.add(3))];
            let alpha1 = ALPHA_NORM_LUT[usize::from(*source.add(7))];
            let alpha_norm = _mm256_set_ps(
                alpha1, alpha1, alpha1, alpha1, alpha0, alpha0, alpha0, alpha0,
            );
            let denominator = _mm256_blendv_ps(alpha_norm, one, _mm256_castsi256_ps(alpha_zero));
            let straight_f = _mm256_div_ps(rgba_f, denominator);
            let straight_i = _mm256_cvttps_epi32(straight_f);
            // Rust's float->u8 cast saturates. Reproduce that behavior before the final lane store.
            let straight_i = _mm256_min_epi32(_mm256_max_epi32(straight_i, min_i), max_i);
            // Alpha is not unpremultiplied. Restore the original A lanes (3 and 7).
            let with_alpha = _mm256_blend_epi32(straight_i, rgba_i, 0b1000_1000);
            // RGBA -> BGRA for both pixels.
            let bgra_i = _mm256_permutevar8x32_epi32(with_alpha, bgra_indices);
            let mut lanes = [0i32; 8];
            _mm256_storeu_si256(lanes.as_mut_ptr().cast::<__m256i>(), bgra_i);
            for (index, value) in lanes.into_iter().enumerate() {
                *source.add(index) = value as u8;
            }
            offset += 8;
        }
        scalar_buffer(&mut buffer[offset..]);
    }
}

#[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
#[target_feature(enable = "sse2")]
unsafe fn sse2_buffer(buffer: &mut [u8]) {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    unsafe {
        let zero = _mm_setzero_si128();
        for pixel in buffer.chunks_exact_mut(4) {
            let alpha = pixel[3];
            if alpha == 0 {
                pixel.swap(0, 2);
                continue;
            }
            let packed = std::ptr::read_unaligned(pixel.as_ptr().cast::<u32>());
            let bytes = _mm_cvtsi32_si128(packed as i32);
            let words = _mm_unpacklo_epi8(bytes, zero);
            let dwords = _mm_unpacklo_epi16(words, zero);
            let rgba_f = _mm_cvtepi32_ps(dwords);
            let alpha_norm = _mm_set1_ps(ALPHA_NORM_LUT[usize::from(alpha)]);
            let straight_f = _mm_div_ps(rgba_f, alpha_norm);
            let straight_i = _mm_cvttps_epi32(straight_f);
            let mut lanes = [0i32; 4];
            _mm_storeu_si128(lanes.as_mut_ptr().cast::<__m128i>(), straight_i);
            pixel[0] = lanes[2].clamp(0, 255) as u8;
            pixel[1] = lanes[1].clamp(0, 255) as u8;
            pixel[2] = lanes[0].clamp(0, 255) as u8;
            pixel[3] = alpha;
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn neon_buffer(buffer: &mut [u8]) {
    use std::arch::aarch64::*;

    unsafe {
        let mut offset = 0usize;
        while offset + 8 <= buffer.len() {
            let source = buffer.as_mut_ptr().add(offset);
            let bytes = vld1_u8(source);
            let words = vmovl_u8(bytes);
            let low = vmovl_u16(vget_low_u16(words));
            let high = vmovl_u16(vget_high_u16(words));
            neon_pixel(source, low);
            neon_pixel(source.add(4), high);
            offset += 8;
        }
        scalar_buffer(&mut buffer[offset..]);
    }
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn neon_pixel(pixel: *mut u8, rgba_i: std::arch::aarch64::uint32x4_t) {
    use std::arch::aarch64::*;

    unsafe {
        let alpha = *pixel.add(3);
        if alpha == 0 {
            let red = *pixel;
            *pixel = *pixel.add(2);
            *pixel.add(2) = red;
            return;
        }
        let rgba_f = vcvtq_f32_u32(rgba_i);
        let alpha_norm = vdupq_n_f32(ALPHA_NORM_LUT[usize::from(alpha)]);
        let straight_f = vdivq_f32(rgba_f, alpha_norm);
        let straight_i = vcvtq_u32_f32(straight_f);
        let mut lanes = [0u32; 4];
        vst1q_u32(lanes.as_mut_ptr(), straight_i);
        *pixel = lanes[2].min(255) as u8;
        *pixel.add(1) = lanes[1].min(255) as u8;
        *pixel.add(2) = lanes[0].min(255) as u8;
        *pixel.add(3) = alpha;
    }
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
    fn alpha_lut_matches_scalar_denominator() {
        for alpha in 0..=255u8 {
            assert_eq!(
                ALPHA_NORM_LUT[usize::from(alpha)].to_bits(),
                (alpha as f32 / 255.0).to_bits()
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
