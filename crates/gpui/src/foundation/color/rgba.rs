use super::hsla::Hsla;
use anyhow::{Context as _, bail};
use fearless_simd::{Level, Simd, dispatch, prelude::*, u8x16};
use schemars::{JsonSchema, json_schema};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};
use std::{borrow::Cow, fmt, sync::LazyLock};

const SIMD_RGBA_ROW_BYTES: usize = 64;

static RGBA_SIMD_LEVEL: LazyLock<Level> = LazyLock::new(Level::new);

/// Convert an RGB hex color code number to a color type.
///
/// This is const-evaluable, so fixed palette colors can be materialized once instead
/// of reconstructed in every paint pass.
pub const fn rgb(hex: u32) -> Rgba {
    let [_, r, g, b] = hex.to_be_bytes();
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

/// Convert an RGBA hex color code number to [`Rgba`].
///
/// This is const-evaluable, so fixed palette colors can be materialized once instead
/// of reconstructed in every paint pass.
pub const fn rgba(hex: u32) -> Rgba {
    let [r, g, b, a] = hex.to_be_bytes();
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: a as f32 / 255.0,
    }
}

/// Swap from RGBA with premultiplied alpha to BGRA
pub(crate) fn swap_rgba_pa_to_bgra(color: &mut [u8]) {
    color.swap(0, 2);
    if color[3] > 0 {
        let a = color[3] as f32 / 255.;
        color[0] = (color[0] as f32 / a) as u8;
        color[1] = (color[1] as f32 / a) as u8;
        color[2] = (color[2] as f32 / a) as u8;
    }
}

/// An RGBA color
#[derive(PartialEq, Clone, Copy, Default)]
#[repr(C)]
pub struct Rgba {
    /// The red component of the color, in the range 0.0 to 1.0
    pub r: f32,
    /// The green component of the color, in the range 0.0 to 1.0
    pub g: f32,
    /// The blue component of the color, in the range 0.0 to 1.0
    pub b: f32,
    /// The alpha component of the color, in the range 0.0 to 1.0
    pub a: f32,
}

impl fmt::Debug for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "rgba({:#010x})", u32::from(*self))
    }
}

impl Rgba {
    /// Returns a copy with an absolute alpha value.
    pub fn alpha(self, alpha: f32) -> Self {
        Self {
            a: alpha.clamp(0.0, 1.0),
            ..self
        }
    }

    /// Multiplies the current alpha by `factor`.
    pub fn opacity(self, factor: f32) -> Self {
        Self {
            a: self.a * factor.clamp(0.0, 1.0),
            ..self
        }
    }

    /// Returns whether this color is fully transparent.
    pub fn is_transparent(self) -> bool {
        self.a == 0.0
    }

    /// Returns whether this color is fully opaque.
    pub fn is_opaque(self) -> bool {
        self.a == 1.0
    }

    /// Create a new [`Rgba`] color by blending this and another color together
    pub fn blend(&self, other: Rgba) -> Self {
        if other.a >= 1.0 {
            other
        } else if other.a <= 0.0 {
            *self
        } else {
            Rgba {
                r: (self.r * (1.0 - other.a)) + (other.r * other.a),
                g: (self.g * (1.0 - other.a)) + (other.g * other.a),
                b: (self.b * (1.0 - other.a)) + (other.b * other.a),
                a: self.a,
            }
        }
    }
}

impl From<Rgba> for u32 {
    fn from(rgba: Rgba) -> Self {
        let r = (rgba.r * 255.0).round() as u32;
        let g = (rgba.g * 255.0).round() as u32;
        let b = (rgba.b * 255.0).round() as u32;
        let a = (rgba.a * 255.0).round() as u32;
        (r << 24) | (g << 16) | (b << 8) | a
    }
}

/// Converts straight-alpha RGBA rows to BGRA in place.
///
/// Rows shorter than one SIMD chunk remain on the scalar path. For larger rows, the Fearless
/// SIMD backend is selected once for the whole upload and the selected token is reused for every
/// row. Bytes after `row_bytes * row_count`, and incomplete pixels at the end of a row, remain
/// untouched.
pub(crate) fn swap_rgba_to_bgra_rows(buffer: &mut [u8], row_bytes: usize, row_count: usize) {
    if row_bytes == 0 || row_count == 0 {
        return;
    }

    let Some(total_bytes) = row_bytes.checked_mul(row_count) else {
        return;
    };
    let Some(buffer) = buffer.get_mut(..total_bytes) else {
        return;
    };

    if row_bytes < SIMD_RGBA_ROW_BYTES || total_bytes < SIMD_RGBA_ROW_BYTES {
        scalar_rgba_rows(buffer, row_bytes, row_count);
        return;
    }

    dispatch_rgba_rows(buffer, row_bytes, row_count);
}

#[cfg(any(test, feature = "bench"))]
pub(crate) fn swap_rgba_to_bgra_rows_scalar(buffer: &mut [u8], row_bytes: usize, row_count: usize) {
    let Some(total_bytes) = row_bytes.checked_mul(row_count) else {
        return;
    };
    let Some(buffer) = buffer.get_mut(..total_bytes) else {
        return;
    };
    scalar_rgba_rows(buffer, row_bytes, row_count);
}

#[cfg(feature = "bench")]
pub(crate) fn swap_rgba_to_bgra_rows_simd(buffer: &mut [u8], row_bytes: usize, row_count: usize) {
    if row_bytes == 0 || row_count == 0 {
        return;
    }
    let Some(total_bytes) = row_bytes.checked_mul(row_count) else {
        return;
    };
    let Some(buffer) = buffer.get_mut(..total_bytes) else {
        return;
    };
    dispatch_rgba_rows(buffer, row_bytes, row_count);
}

#[inline]
fn dispatch_rgba_rows(buffer: &mut [u8], row_bytes: usize, row_count: usize) {
    let level = *RGBA_SIMD_LEVEL;
    if level.is_fallback() {
        scalar_rgba_rows(buffer, row_bytes, row_count);
        return;
    }

    // Keep AVX-512 out of the default UI path. Fearless SIMD proves the selected feature level;
    // AVX2 is the wider x86 path used here to avoid the frequency and power trade-offs of AVX-512.
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if let Some(avx2) = level.as_avx2() {
        dispatch!(Level::Avx2(avx2), simd => simd_rgba_rows(simd, buffer, row_bytes, row_count));
        return;
    }

    dispatch!(level, simd => simd_rgba_rows(simd, buffer, row_bytes, row_count));
}

#[inline]
fn scalar_rgba_rows(buffer: &mut [u8], row_bytes: usize, row_count: usize) {
    for row in buffer.chunks_exact_mut(row_bytes).take(row_count) {
        for pixel in row.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
    }
}

#[inline(always)]
fn simd_rgba_rows<S: Simd>(simd: S, buffer: &mut [u8], row_bytes: usize, row_count: usize) {
    for row in buffer.chunks_exact_mut(row_bytes).take(row_count) {
        let mut chunks = row.chunks_exact_mut(SIMD_RGBA_ROW_BYTES);
        for chunk in &mut chunks {
            let [red, green, blue, alpha] = u8x16::load_four_interleaved(simd, chunk);
            u8x16::store_four_interleaved([blue, green, red, alpha], chunk);
        }
        for pixel in chunks.into_remainder().chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
    }
}

struct RgbaVisitor;

impl Visitor<'_> for RgbaVisitor {
    type Value = Rgba;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a string in the format #rrggbb or #rrggbbaa")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Rgba, E> {
        Rgba::try_from(value).map_err(E::custom)
    }
}

impl JsonSchema for Rgba {
    fn schema_name() -> Cow<'static, str> {
        "Rgba".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        json_schema!({
            "type": "string",
            "pattern": "^#([0-9a-fA-F]{3}|[0-9a-fA-F]{4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$"
        })
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(RgbaVisitor)
    }
}

impl Serialize for Rgba {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let r = (self.r * 255.0).round() as u8;
        let g = (self.g * 255.0).round() as u8;
        let b = (self.b * 255.0).round() as u8;
        let a = (self.a * 255.0).round() as u8;

        let s = format!("#{r:02x}{g:02x}{b:02x}{a:02x}");
        serializer.serialize_str(&s)
    }
}

impl From<Hsla> for Rgba {
    fn from(color: Hsla) -> Self {
        let h = color.h;
        let s = color.s;
        let l = color.l;

        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0).abs());
        let m = l - c / 2.0;
        let cm = c + m;
        let xm = x + m;

        let (r, g, b) = match (h * 6.0).floor() as i32 {
            0 | 6 => (cm, xm, m),
            1 => (xm, cm, m),
            2 => (m, cm, xm),
            3 => (m, xm, cm),
            4 => (xm, m, cm),
            _ => (cm, m, xm),
        };

        Rgba {
            r: r.clamp(0., 1.),
            g: g.clamp(0., 1.),
            b: b.clamp(0., 1.),
            a: color.a,
        }
    }
}

impl TryFrom<&'_ str> for Rgba {
    type Error = anyhow::Error;

    fn try_from(value: &'_ str) -> Result<Self, Self::Error> {
        const RGB: usize = "rgb".len();
        const RGBA: usize = "rgba".len();
        const RRGGBB: usize = "rrggbb".len();
        const RRGGBBAA: usize = "rrggbbaa".len();

        const EXPECTED_FORMATS: &str = "Expected #rgb, #rgba, #rrggbb, or #rrggbbaa";
        const INVALID_UNICODE: &str = "invalid unicode characters in color";

        let Some(("", hex)) = value.trim().split_once('#') else {
            bail!("invalid RGBA hex color: '{value}'. {EXPECTED_FORMATS}");
        };

        let (r, g, b, a) = match hex.len() {
            RGB | RGBA => {
                let r = u8::from_str_radix(
                    hex.get(0..1).with_context(|| {
                        format!("{INVALID_UNICODE}: r component of #rgb/#rgba for value: '{value}'")
                    })?,
                    16,
                )?;
                let g = u8::from_str_radix(
                    hex.get(1..2).with_context(|| {
                        format!("{INVALID_UNICODE}: g component of #rgb/#rgba for value: '{value}'")
                    })?,
                    16,
                )?;
                let b = u8::from_str_radix(
                    hex.get(2..3).with_context(|| {
                        format!("{INVALID_UNICODE}: b component of #rgb/#rgba for value: '{value}'")
                    })?,
                    16,
                )?;
                let a = if hex.len() == RGBA {
                    u8::from_str_radix(
                        hex.get(3..4).with_context(|| {
                            format!("{INVALID_UNICODE}: a component of #rgba for value: '{value}'")
                        })?,
                        16,
                    )?
                } else {
                    0xf
                };

                /// Duplicates a given hex digit.
                /// E.g., `0xf` -> `0xff`.
                const fn duplicate(value: u8) -> u8 {
                    (value << 4) | value
                }

                (duplicate(r), duplicate(g), duplicate(b), duplicate(a))
            }
            RRGGBB | RRGGBBAA => {
                let r = u8::from_str_radix(
                    hex.get(0..2).with_context(|| {
                        format!(
                            "{}: r component of #rrggbb/#rrggbbaa for value: '{}'",
                            INVALID_UNICODE, value
                        )
                    })?,
                    16,
                )?;
                let g = u8::from_str_radix(
                    hex.get(2..4).with_context(|| {
                        format!(
                            "{INVALID_UNICODE}: g component of #rrggbb/#rrggbbaa for value: '{value}'"
                        )
                    })?,
                    16,
                )?;
                let b = u8::from_str_radix(
                    hex.get(4..6).with_context(|| {
                        format!(
                            "{INVALID_UNICODE}: b component of #rrggbb/#rrggbbaa for value: '{value}'"
                        )
                    })?,
                    16,
                )?;
                let a = if hex.len() == RRGGBBAA {
                    u8::from_str_radix(
                        hex.get(6..8).with_context(|| {
                            format!(
                                "{INVALID_UNICODE}: a component of #rrggbbaa for value: '{value}'"
                            )
                        })?,
                        16,
                    )?
                } else {
                    0xff
                };
                (r, g, b, a)
            }
            _ => bail!("invalid RGBA hex color: '{value}'. {EXPECTED_FORMATS}"),
        };

        Ok(Rgba {
            r: r as f32 / 255.,
            g: g as f32 / 255.,
            b: b as f32 / 255.,
            a: a as f32 / 255.,
        })
    }
}
