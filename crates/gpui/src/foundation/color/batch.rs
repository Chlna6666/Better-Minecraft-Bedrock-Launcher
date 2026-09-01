use super::{Hsla, Rgba};
use crate::{CpuVectorLevel, cpu_vector_level};

const SIMD_BATCH_THRESHOLD: usize = 8;

/// Converts a batch of RGBA colors to HSLA colors.
///
/// The output slice must have the same length as the input slice. GPUI selects an
/// ISA-specialized implementation once the batch is large enough to amortize dispatch;
/// small batches and unsupported architectures use the scalar path.
pub fn rgba_to_hsla_batch(input: &[Rgba], output: &mut [Hsla]) {
    assert_eq!(input.len(), output.len(), "color batch lengths must match");
    if input.len() < SIMD_BATCH_THRESHOLD {
        rgba_to_hsla_scalar(input, output);
        return;
    }

    match cpu_vector_level() {
        #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
        CpuVectorLevel::Avx2 => unsafe { rgba_to_hsla_avx2(input, output) },
        #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
        CpuVectorLevel::Sse2 => unsafe { rgba_to_hsla_sse2(input, output) },
        #[cfg(target_arch = "aarch64")]
        CpuVectorLevel::Neon => unsafe { rgba_to_hsla_neon(input, output) },
        _ => rgba_to_hsla_scalar(input, output),
    }
}

/// Converts a batch of HSLA colors to RGBA colors.
///
/// The output slice must have the same length as the input slice. Runtime CPU feature
/// detection selects AVX2/SSE2/NEON-specialized loops where available, with a scalar fallback.
pub fn hsla_to_rgba_batch(input: &[Hsla], output: &mut [Rgba]) {
    assert_eq!(input.len(), output.len(), "color batch lengths must match");
    if input.len() < SIMD_BATCH_THRESHOLD {
        hsla_to_rgba_scalar(input, output);
        return;
    }

    match cpu_vector_level() {
        #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
        CpuVectorLevel::Avx2 => unsafe { hsla_to_rgba_avx2(input, output) },
        #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
        CpuVectorLevel::Sse2 => unsafe { hsla_to_rgba_sse2(input, output) },
        #[cfg(target_arch = "aarch64")]
        CpuVectorLevel::Neon => unsafe { hsla_to_rgba_neon(input, output) },
        _ => hsla_to_rgba_scalar(input, output),
    }
}

/// Interpolates two equally-sized HSLA batches using normalized shortest-path hue interpolation.
///
/// `t` is clamped to `0..=1`. AVX2 performs two `Hsla` values per vector, SSE2 and AArch64 NEON
/// process all four channels of one color per vector. Unsupported architectures fall back to
/// scalar code.
pub fn lerp_hsla_batch(from: &[Hsla], to: &[Hsla], t: f32, output: &mut [Hsla]) {
    assert_eq!(from.len(), to.len(), "color batch lengths must match");
    assert_eq!(from.len(), output.len(), "color batch lengths must match");
    let t = t.clamp(0.0, 1.0);
    if from.len() < SIMD_BATCH_THRESHOLD {
        lerp_hsla_scalar(from, to, t, output);
        return;
    }

    match cpu_vector_level() {
        #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
        CpuVectorLevel::Avx2 => unsafe { lerp_hsla_avx2(from, to, t, output) },
        #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
        CpuVectorLevel::Sse2 => unsafe { lerp_hsla_sse2(from, to, t, output) },
        #[cfg(target_arch = "aarch64")]
        CpuVectorLevel::Neon => unsafe { lerp_hsla_neon(from, to, t, output) },
        _ => lerp_hsla_scalar(from, to, t, output),
    }
}

#[inline(always)]
fn rgba_to_hsla_one(color: Rgba) -> Hsla {
    let r = color.r;
    let g = color.g;
    let b = color.b;
    let max = r.max(g.max(b));
    let min = r.min(g.min(b));
    let delta = max - min;
    let l = (max + min) * 0.5;
    let denominator = 1.0 - (2.0 * l - 1.0).abs();
    let s = if denominator > 0.0 {
        delta / denominator
    } else {
        0.0
    };
    let h = if delta == 0.0 {
        0.0
    } else if max == r {
        let segment = (g - b) / delta;
        (if segment < 0.0 { segment + 6.0 } else { segment }) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    Hsla {
        h,
        s,
        l,
        a: color.a,
    }
}

#[inline(always)]
fn hsla_to_rgba_one(color: Hsla) -> Rgba {
    let h = color.h;
    let s = color.s;
    let l = color.l;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h6 = h * 6.0;
    let x = c * (1.0 - (h6 % 2.0 - 1.0).abs());
    let m = l - c * 0.5;
    let cm = c + m;
    let xm = x + m;
    let (r, g, b) = match h6.floor() as i32 {
        0 | 6 => (cm, xm, m),
        1 => (xm, cm, m),
        2 => (m, cm, xm),
        3 => (m, xm, cm),
        4 => (xm, m, cm),
        _ => (cm, m, xm),
    };
    Rgba {
        r: r.clamp(0.0, 1.0),
        g: g.clamp(0.0, 1.0),
        b: b.clamp(0.0, 1.0),
        a: color.a,
    }
}

#[inline(always)]
fn lerp_hsla_one(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let mut hue_delta = b.h - a.h;
    if hue_delta > 0.5 {
        hue_delta -= 1.0;
    } else if hue_delta < -0.5 {
        hue_delta += 1.0;
    }
    let mut h = a.h + hue_delta * t;
    if h >= 1.0 {
        h -= 1.0;
    } else if h < 0.0 {
        h += 1.0;
    }
    Hsla {
        h,
        s: a.s + (b.s - a.s) * t,
        l: a.l + (b.l - a.l) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

#[inline(always)]
fn rgba_to_hsla_scalar(input: &[Rgba], output: &mut [Hsla]) {
    for (source, target) in input.iter().copied().zip(output.iter_mut()) {
        *target = rgba_to_hsla_one(source);
    }
}

#[inline(always)]
fn hsla_to_rgba_scalar(input: &[Hsla], output: &mut [Rgba]) {
    for (source, target) in input.iter().copied().zip(output.iter_mut()) {
        *target = hsla_to_rgba_one(source);
    }
}

#[inline(always)]
fn lerp_hsla_scalar(from: &[Hsla], to: &[Hsla], t: f32, output: &mut [Hsla]) {
    for ((a, b), target) in from
        .iter()
        .copied()
        .zip(to.iter().copied())
        .zip(output.iter_mut())
    {
        *target = lerp_hsla_one(a, b, t);
    }
}

// RGBA<->HSLA contains lane-dependent hue selection. Keeping the loops in target-feature
// functions lets LLVM emit ISA-specific compare/select code where profitable without charging
// every scalar conversion for runtime dispatch.
#[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
#[target_feature(enable = "avx2")]
unsafe fn rgba_to_hsla_avx2(input: &[Rgba], output: &mut [Hsla]) {
    rgba_to_hsla_scalar(input, output);
}

#[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
#[target_feature(enable = "sse2")]
unsafe fn rgba_to_hsla_sse2(input: &[Rgba], output: &mut [Hsla]) {
    rgba_to_hsla_scalar(input, output);
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn rgba_to_hsla_neon(input: &[Rgba], output: &mut [Hsla]) {
    rgba_to_hsla_scalar(input, output);
}

#[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
#[target_feature(enable = "avx2")]
unsafe fn hsla_to_rgba_avx2(input: &[Hsla], output: &mut [Rgba]) {
    hsla_to_rgba_scalar(input, output);
}

#[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
#[target_feature(enable = "sse2")]
unsafe fn hsla_to_rgba_sse2(input: &[Hsla], output: &mut [Rgba]) {
    hsla_to_rgba_scalar(input, output);
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn hsla_to_rgba_neon(input: &[Hsla], output: &mut [Rgba]) {
    hsla_to_rgba_scalar(input, output);
}

#[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
#[target_feature(enable = "avx2")]
unsafe fn lerp_hsla_avx2(from: &[Hsla], to: &[Hsla], t: f32, output: &mut [Hsla]) {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    unsafe {
        let t8 = _mm256_set1_ps(t);
        let half = _mm256_set1_ps(0.5);
        let neg_half = _mm256_set1_ps(-0.5);
        let one = _mm256_set1_ps(1.0);
        let neg_one = _mm256_set1_ps(-1.0);
        let zero = _mm256_setzero_ps();
        // Hsla is repr(C): hue occupies lanes 0 and 4 when two colors are loaded together.
        let hue_mask = _mm256_castsi256_ps(_mm256_set_epi32(0, 0, 0, -1, 0, 0, 0, -1));
        let mut index = 0usize;
        while index + 2 <= from.len() {
            let a = _mm256_loadu_ps(from.as_ptr().add(index).cast::<f32>());
            let b = _mm256_loadu_ps(to.as_ptr().add(index).cast::<f32>());
            let mut delta = _mm256_sub_ps(b, a);
            let gt = _mm256_and_ps(_mm256_cmp_ps(delta, half, _CMP_GT_OQ), hue_mask);
            let lt = _mm256_and_ps(_mm256_cmp_ps(delta, neg_half, _CMP_LT_OQ), hue_mask);
            delta = _mm256_add_ps(delta, _mm256_and_ps(gt, neg_one));
            delta = _mm256_add_ps(delta, _mm256_and_ps(lt, one));
            let mut value = _mm256_add_ps(a, _mm256_mul_ps(delta, t8));
            let wrap_hi = _mm256_and_ps(_mm256_cmp_ps(value, one, _CMP_GE_OQ), hue_mask);
            let wrap_lo = _mm256_and_ps(_mm256_cmp_ps(value, zero, _CMP_LT_OQ), hue_mask);
            value = _mm256_add_ps(value, _mm256_and_ps(wrap_hi, neg_one));
            value = _mm256_add_ps(value, _mm256_and_ps(wrap_lo, one));
            _mm256_storeu_ps(output.as_mut_ptr().add(index).cast::<f32>(), value);
            index += 2;
        }
        lerp_hsla_scalar(&from[index..], &to[index..], t, &mut output[index..]);
    }
}

#[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
#[target_feature(enable = "sse2")]
unsafe fn lerp_hsla_sse2(from: &[Hsla], to: &[Hsla], t: f32, output: &mut [Hsla]) {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    unsafe {
        let t4 = _mm_set1_ps(t);
        let half = _mm_set1_ps(0.5);
        let neg_half = _mm_set1_ps(-0.5);
        let one = _mm_set1_ps(1.0);
        let neg_one = _mm_set1_ps(-1.0);
        let zero = _mm_setzero_ps();
        let hue_mask = _mm_castsi128_ps(_mm_set_epi32(0, 0, 0, -1));
        for index in 0..from.len() {
            let a = _mm_loadu_ps(from.as_ptr().add(index).cast::<f32>());
            let b = _mm_loadu_ps(to.as_ptr().add(index).cast::<f32>());
            let mut delta = _mm_sub_ps(b, a);
            let gt = _mm_and_ps(_mm_cmpgt_ps(delta, half), hue_mask);
            let lt = _mm_and_ps(_mm_cmplt_ps(delta, neg_half), hue_mask);
            delta = _mm_add_ps(delta, _mm_and_ps(gt, neg_one));
            delta = _mm_add_ps(delta, _mm_and_ps(lt, one));
            let mut value = _mm_add_ps(a, _mm_mul_ps(delta, t4));
            let wrap_hi = _mm_and_ps(_mm_cmpge_ps(value, one), hue_mask);
            let wrap_lo = _mm_and_ps(_mm_cmplt_ps(value, zero), hue_mask);
            value = _mm_add_ps(value, _mm_and_ps(wrap_hi, neg_one));
            value = _mm_add_ps(value, _mm_and_ps(wrap_lo, one));
            _mm_storeu_ps(output.as_mut_ptr().add(index).cast::<f32>(), value);
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn lerp_hsla_neon(from: &[Hsla], to: &[Hsla], t: f32, output: &mut [Hsla]) {
    use std::arch::aarch64::*;

    unsafe {
        let t4 = vdupq_n_f32(t);
        let half = vdupq_n_f32(0.5);
        let neg_half = vdupq_n_f32(-0.5);
        let one = vdupq_n_f32(1.0);
        let neg_one = vdupq_n_f32(-1.0);
        let zero = vdupq_n_f32(0.0);
        let hue_mask = vsetq_lane_u32(u32::MAX, vdupq_n_u32(0), 0);

        for index in 0..from.len() {
            let a = vld1q_f32(from.as_ptr().add(index).cast::<f32>());
            let b = vld1q_f32(to.as_ptr().add(index).cast::<f32>());
            let mut delta = vsubq_f32(b, a);

            let gt = vandq_u32(vcgtq_f32(delta, half), hue_mask);
            let lt = vandq_u32(vcltq_f32(delta, neg_half), hue_mask);
            delta = vaddq_f32(delta, vbslq_f32(gt, neg_one, zero));
            delta = vaddq_f32(delta, vbslq_f32(lt, one, zero));

            let mut value = vaddq_f32(a, vmulq_f32(delta, t4));
            let wrap_hi = vandq_u32(vcgeq_f32(value, one), hue_mask);
            let wrap_lo = vandq_u32(vcltq_f32(value, zero), hue_mask);
            value = vaddq_f32(value, vbslq_f32(wrap_hi, neg_one, zero));
            value = vaddq_f32(value, vbslq_f32(wrap_lo, one, zero));
            vst1q_f32(output.as_mut_ptr().add(index).cast::<f32>(), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_hsla_close(actual: Hsla, expected: Hsla) {
        assert!((actual.h - expected.h).abs() <= 1e-5, "h: {actual:?} != {expected:?}");
        assert!((actual.s - expected.s).abs() <= 1e-5, "s: {actual:?} != {expected:?}");
        assert!((actual.l - expected.l).abs() <= 1e-5, "l: {actual:?} != {expected:?}");
        assert!((actual.a - expected.a).abs() <= 1e-5, "a: {actual:?} != {expected:?}");
    }

    fn assert_rgba_close(actual: Rgba, expected: Rgba) {
        assert!((actual.r - expected.r).abs() <= 1e-5, "r: {actual:?} != {expected:?}");
        assert!((actual.g - expected.g).abs() <= 1e-5, "g: {actual:?} != {expected:?}");
        assert!((actual.b - expected.b).abs() <= 1e-5, "b: {actual:?} != {expected:?}");
        assert!((actual.a - expected.a).abs() <= 1e-5, "a: {actual:?} != {expected:?}");
    }

    #[test]
    fn batch_rgba_hsla_matches_scalar_conversion() {
        let input: Vec<_> = (0..32)
            .map(|index| Rgba {
                r: ((index * 37) % 255) as f32 / 255.0,
                g: ((index * 73 + 11) % 255) as f32 / 255.0,
                b: ((index * 19 + 101) % 255) as f32 / 255.0,
                a: ((index * 29 + 127) % 255) as f32 / 255.0,
            })
            .collect();
        let mut hsla = vec![Hsla::default(); input.len()];
        rgba_to_hsla_batch(&input, &mut hsla);
        for (actual, source) in hsla.iter().copied().zip(input.iter().copied()) {
            assert_hsla_close(actual, Hsla::from(source));
        }

        let mut rgba = vec![Rgba::default(); input.len()];
        hsla_to_rgba_batch(&hsla, &mut rgba);
        for (actual, source) in rgba.iter().copied().zip(hsla.iter().copied()) {
            assert_rgba_close(actual, Rgba::from(source));
        }
    }

    #[test]
    fn batch_hsla_lerp_wraps_hue_on_shortest_path() {
        let from = vec![
            Hsla {
                h: 0.95,
                s: 0.2,
                l: 0.3,
                a: 0.4,
            };
            16
        ];
        let to = vec![
            Hsla {
                h: 0.05,
                s: 0.8,
                l: 0.9,
                a: 1.0,
            };
            16
        ];
        let mut output = vec![Hsla::default(); 16];
        lerp_hsla_batch(&from, &to, 0.5, &mut output);
        for color in output {
            assert!(
                color.h <= 1e-5 || (1.0 - color.h) <= 1e-5,
                "unexpected hue {color:?}"
            );
            assert!((color.s - 0.5).abs() <= 1e-5);
            assert!((color.l - 0.6).abs() <= 1e-5);
            assert!((color.a - 0.7).abs() <= 1e-5);
        }
    }
}
