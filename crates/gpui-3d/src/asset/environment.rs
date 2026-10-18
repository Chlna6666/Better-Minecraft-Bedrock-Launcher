//! Scene environment maps for image-based lighting and sky background.

use crate::asset::image::decode_texture;
use crate::{TextureAsset, TextureAssetId};
use std::path::Path;
use std::sync::Arc;

/// Errors creating or loading a scene environment map.
#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    /// Environment intensity must be finite and non-negative.
    #[error("environment intensity must be finite and non-negative")]
    InvalidIntensity,
    /// Environment rotation must be finite.
    #[error("environment rotation must be finite")]
    InvalidRotation,
    /// The environment image could not be read.
    #[error("environment image read failed: {0}")]
    Io(#[from] std::io::Error),
    /// The environment image could not be decoded.
    #[error("environment image decode failed: {0}")]
    Image(String),
}

/// Equirectangular environment map plus intensity/rotation.
///
/// The texture is sampled as an equirectangular (latitude-longitude) map: U wraps around the
/// horizon and V runs from the zenith to the nadir. The renderer uses the chain's mip levels as a
/// cheap prefiltered radiance approximation: diffuse ambient samples a heavily blurred level and
/// roughness maps to progressively sharper reflection levels. A full CPU/GPU irradiance and
/// specular prefilter with a BRDF LUT is not required by this approximation.
#[derive(Debug)]
pub struct EnvironmentMap {
    texture: Arc<TextureAsset>,
    intensity: f32,
    rotation: f32,
}

impl EnvironmentMap {
    /// Creates an environment map from caller-provided equirectangular texture data.
    ///
    /// A complete mip chain produces smoother diffuse light and roughness-dependent reflections;
    /// a single-level texture still works but behaves like a mirror-sharp environment for every
    /// roughness. Both linear and sRGB texture assets are accepted; sRGB assets are decoded to
    /// linear by the sampler.
    ///
    /// # Errors
    ///
    /// Returns [`EnvironmentError::InvalidIntensity`] for a non-finite or negative intensity and
    /// [`EnvironmentError::InvalidRotation`] for a non-finite rotation.
    pub fn new(
        texture: Arc<TextureAsset>,
        intensity: f32,
        rotation: f32,
    ) -> Result<Self, EnvironmentError> {
        if !intensity.is_finite() || intensity < 0.0 {
            return Err(EnvironmentError::InvalidIntensity);
        }
        if !rotation.is_finite() {
            return Err(EnvironmentError::InvalidRotation);
        }
        Ok(Self {
            texture,
            intensity,
            rotation,
        })
    }

    /// Loads a PNG/JPEG equirectangular environment image from disk.
    ///
    /// Images larger than 4096 pixels are downscaled, and a full box-filtered mip chain is
    /// generated for roughness-dependent sampling. The image is treated as sRGB-encoded.
    ///
    /// # Errors
    ///
    /// Returns [`EnvironmentError::Io`] when the file cannot be read and
    /// [`EnvironmentError::Image`] when it is not a supported image.
    pub fn load_path(path: impl AsRef<Path>) -> Result<Self, EnvironmentError> {
        let bytes = std::fs::read(path)?;
        let (texture, resized_from) = decode_texture(&bytes, true, 4096, true, true)
            .map_err(|error| EnvironmentError::Image(error.to_string()))?;
        if resized_from.is_some() {
            // Downscaling is expected for large panoramas; the decoder already warns through the
            // import path, so only the resulting chain is used here.
        }
        Self::new(Arc::new(texture), 1.0, 0.0)
    }

    /// Builds a deterministic procedural studio environment.
    ///
    /// The panorama contains a blue sky gradient, a warm sun disk near the horizon, and a darker
    /// ground gradient. It exercises sky background and image-based lighting without external
    /// assets or licensing constraints.
    #[must_use]
    pub fn procedural_studio() -> Self {
        const WIDTH: u32 = 1024;
        const HEIGHT: u32 = 512;
        let mut base = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];
        for y in 0..HEIGHT {
            let v = y as f32 / (HEIGHT - 1) as f32;
            for x in 0..WIDTH {
                let u = x as f32 / (WIDTH - 1) as f32;
                let horizon = (v - 0.5).abs();
                let sky = [
                    0.42 + 0.2 * (1.0 - v),
                    0.55 + 0.2 * (1.0 - v),
                    0.82 + 0.14 * (1.0 - v),
                ];
                let ground = [0.16, 0.14, 0.12];
                let base_color = if v < 0.52 {
                    let blend = (0.52 - v) / 0.52;
                    [
                        sky[0] * blend + 0.4 * (1.0 - blend),
                        sky[1] * blend + 0.38 * (1.0 - blend),
                        sky[2] * blend + 0.34 * (1.0 - blend),
                    ]
                } else {
                    [
                        ground[0] * (1.0 - horizon * 0.4),
                        ground[1] * (1.0 - horizon * 0.4),
                        ground[2] * (1.0 - horizon * 0.4),
                    ]
                };
                // Warm sun disk near the horizon at U=0.72, plus a soft glow.
                let sun_u = 0.72;
                let sun_v = 0.32;
                let du = ((u - sun_u).abs() * WIDTH as f32 / 32.0).min(1.0);
                let dv = ((v - sun_v).abs() * HEIGHT as f32 / 16.0).min(1.0);
                let sun = (1.0 - (du * du + dv * dv).sqrt()).max(0.0).powi(4);
                let glow = (1.0 - ((du * 0.6).powi(2) + (dv * 0.6).powi(2)).sqrt()).max(0.0) * 0.35;
                let offset = ((y * WIDTH + x) * 4) as usize;
                base[offset] = ((base_color[0] + sun * 1.4 + glow) * 255.0).clamp(0.0, 255.0) as u8;
                base[offset + 1] =
                    ((base_color[1] + sun * 1.25 + glow) * 255.0).clamp(0.0, 255.0) as u8;
                base[offset + 2] =
                    ((base_color[2] + sun * 0.9 + glow) * 255.0).clamp(0.0, 255.0) as u8;
                base[offset + 3] = 255;
            }
        }
        let mut levels: Vec<Arc<[u8]>> = vec![Arc::from(base)];
        let (mut width, mut height) = (WIDTH, HEIGHT);
        while width > 1 || height > 1 {
            let next_width = (width / 2).max(1);
            let next_height = (height / 2).max(1);
            let previous = levels
                .last()
                .expect("environment mip chain starts with level zero");
            let mut next = vec![0_u8; (next_width * next_height * 4) as usize];
            for y in 0..next_height {
                for x in 0..next_width {
                    let mut accumulator = [0_u32; 4];
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let source_x = (x * 2 + dx).min(width - 1);
                            let source_y = (y * 2 + dy).min(height - 1);
                            let offset = ((source_y * width + source_x) * 4) as usize;
                            for channel in 0..4 {
                                accumulator[channel] += u32::from(previous[offset + channel]);
                            }
                            // Each source texel participates in exactly four samples; the fixed
                            // divisor keeps the average correct even at odd extents.
                        }
                    }
                    let offset = ((y * next_width + x) * 4) as usize;
                    for channel in 0..4 {
                        next[offset + channel] =
                            u8::try_from(accumulator[channel] / 4).unwrap_or(255);
                    }
                }
            }
            levels.push(Arc::from(next));
            width = next_width;
            height = next_height;
        }
        let texture = TextureAsset::rgba8_mip_chain(WIDTH, HEIGHT, levels)
            .expect("procedural environment texture is valid");
        Self {
            texture: Arc::new(texture),
            intensity: 1.25,
            rotation: 0.0,
        }
    }

    /// Environment texture.
    #[must_use]
    pub fn texture(&self) -> &Arc<TextureAsset> {
        &self.texture
    }

    /// Environment asset identity.
    #[must_use]
    pub fn id(&self) -> TextureAssetId {
        self.texture.id()
    }

    /// Linear radiance multiplier applied to sampled environment light and sky background.
    #[must_use]
    pub const fn intensity(&self) -> f32 {
        self.intensity
    }

    /// Yaw rotation of the panorama in radians.
    #[must_use]
    pub const fn rotation(&self) -> f32 {
        self.rotation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_validation_rejects_invalid_parameters() {
        let texture = Arc::new(TextureAsset::rgba8(2, 1, vec![255; 8]).expect("texture"));
        assert!(matches!(
            EnvironmentMap::new(texture.clone(), -1.0, 0.0),
            Err(EnvironmentError::InvalidIntensity)
        ));
        assert!(matches!(
            EnvironmentMap::new(texture.clone(), f32::NAN, 0.0),
            Err(EnvironmentError::InvalidIntensity)
        ));
        assert!(matches!(
            EnvironmentMap::new(texture, 1.0, f32::INFINITY),
            Err(EnvironmentError::InvalidRotation)
        ));
    }

    #[test]
    fn procedural_environment_has_a_full_mip_chain() {
        let environment = EnvironmentMap::procedural_studio();
        assert_eq!(environment.texture().width(), 1024);
        assert_eq!(environment.texture().height(), 512);
        assert_eq!(environment.texture().mip_level_count(), 11);
        assert!(environment.intensity() > 0.0);
        assert_eq!(environment.rotation(), 0.0);
        assert_ne!(environment.texture().id(), TextureAssetId(0));
    }
}
