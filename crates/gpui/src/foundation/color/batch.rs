use super::{Hsla, Rgba};
use fearless_simd::{Level, Simd, dispatch};

const SIMD_BATCH_THRESHOLD: usize = 8;

/// Converts a batch of RGBA colors to HSLA colors.
///
/// The output slice must have the same length as the input slice. GPUI keeps tiny batches on the
/// scalar path and uses Fearless SIMD runtime multiversioning once there is enough work to amortize
/// dispatch. Branch-heavy hue selection remains ordinary safe Rust so LLVM can specialize it for
/// the selected ISA without GPUI owning target-feature or intrinsic safety contracts.
pub fn rgba_to_hsla_batch(input: &[Rgba], output: &mut [Hsla]) {
    assert_eq!(input.len(), output.len(), "color batch lengths must match");
    if input.len() < SIMD_BATCH_THRESHOLD {
        rgba_to_hsla_scalar(input, output);
        return;
    }

    let level = Level::new();
    dispatch!(level, simd => rgba_to_hsla_multiversioned(simd, input, output));
}

/// Converts a batch of HSLA colors to RGBA colors.
///
/// The output slice must have the same length as the input slice. Large batches are compiled into
/// ISA-specialized safe-Rust loops and selected at runtime by Fearless SIMD.
pub fn hsla_to_rgba_batch(input: &[Hsla], output: &mut [Rgba]) {
    assert_eq!(input.len(), output.len(), "color batch lengths must match");
    if input.len() < SIMD_BATCH_THRESHOLD {
        hsla_to_rgba_scalar(input, output);
        return;
    }

    let level = Level::new();
    dispatch!(level, simd => hsla_to_rgba_multiversioned(simd, input, output));
}

/// Interpolates two equally-sized HSLA batches using normalized shortest-path hue interpolation.
///
/// `t` is clamped to `0..=1`. Tiny batches stay scalar; larger batches use Fearless SIMD runtime
/// multiversioning so x86/x86-64 and AArch64 receive the strongest supported code path without
/// handwritten feature detection or architecture-specific unsafe blocks in GPUI.
pub fn lerp_hsla_batch(from: &[Hsla], to: &[Hsla], t: f32, output: &mut [Hsla]) {
    assert_eq!(from.len(), to.len(), "color batch lengths must match");
    assert_eq!(from.len(), output.len(), "color batch lengths must match");
    let t = t.clamp(0.0, 1.0);
    if from.len() < SIMD_BATCH_THRESHOLD {
        lerp_hsla_scalar(from, to, t, output);
        return;
    }

    let level = Level::new();
    dispatch!(level, simd => lerp_hsla_multiversioned(simd, from, to, t, output));
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

#[inline(always)]
fn rgba_to_hsla_multiversioned<S: Simd>(_: S, input: &[Rgba], output: &mut [Hsla]) {
    rgba_to_hsla_scalar(input, output);
}

#[inline(always)]
fn hsla_to_rgba_multiversioned<S: Simd>(_: S, input: &[Hsla], output: &mut [Rgba]) {
    hsla_to_rgba_scalar(input, output);
}

#[inline(always)]
fn lerp_hsla_multiversioned<S: Simd>(_: S, from: &[Hsla], to: &[Hsla], t: f32, output: &mut [Hsla]) {
    lerp_hsla_scalar(from, to, t, output);
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
