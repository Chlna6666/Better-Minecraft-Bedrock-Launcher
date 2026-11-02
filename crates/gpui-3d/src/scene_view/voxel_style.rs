//! Frame-wide voxel presentation, independent of Minecraft world policy.
use crate::{TextureColorSpace, Vec3};

/// Visible voxel geometry representation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VoxelDisplay {
    /// Material textures and authored vertex tint.
    #[default]
    Textured = 0,
    /// Authored vertex colors without texture sampling.
    VertexColor = 1,
    /// Quad edges without texture sampling; occlusion proxies are disabled.
    Wireframe = 2,
}

/// How authored voxel lighting is combined with frame lighting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VoxelIllumination {
    /// Authored lighting with separate sky/block brightness.
    #[default]
    Vertex = 0,
    /// Full brightness; ignores cell lighting and generated corner AO.
    Unlit = 1,
    /// Directional raster lighting with optional opaque-face shadows.
    Sunlight = 2,
}

/// Frame-only voxel controls; changing these does not replace scene assets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelShading {
    /// Encoding of the combined voxel base color before PBR. Keep Linear for standard materials
    /// with hardware-decoded sRGB textures; use Srgb for display bytes stored in linear textures.
    pub color_space: TextureColorSpace,
    /// Geometry representation.
    pub display: VoxelDisplay,
    /// Lighting combination.
    pub illumination: VoxelIllumination,
    /// Daylight scale, independent of block emission, in 0..=1.
    pub sky_brightness: f32,
    /// Unit direction from a surface towards the sun.
    pub sun_direction: Vec3,
    /// Non-negative linear sun color.
    pub sun_color: [f32; 3],
    /// Non-negative direct-light intensity.
    pub sun_intensity: f32,
    /// Shadow opacity in 0..=1; zero skips shadow work, retaining existing cached resources.
    pub shadow_strength: f32,
}

impl Default for VoxelShading {
    fn default() -> Self {
        Self {
            color_space: TextureColorSpace::Linear,
            display: VoxelDisplay::Textured,
            illumination: VoxelIllumination::Vertex,
            sky_brightness: 1.,
            sun_direction: Vec3::Y,
            sun_color: [1.; 3],
            sun_intensity: 0.,
            shadow_strength: 0.,
        }
    }
}

/// Invalid non-finite or out-of-range voxel presentation parameters.
#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("voxel shading requires finite colors, unit sun direction and normalized brightness")]
pub struct VoxelShadingError;

impl VoxelShading {
    pub(super) fn validate(self) -> Result<(), VoxelShadingError> {
        if !self.sky_brightness.is_finite()
            || !(0. ..=1.).contains(&self.sky_brightness)
            || !self.shadow_strength.is_finite()
            || !(0. ..=1.).contains(&self.shadow_strength)
            || !self.sun_intensity.is_finite()
            || self.sun_intensity < 0.
            || self.sun_color.iter().any(|v| !v.is_finite() || *v < 0.)
            || !self.sun_direction.length_squared().is_finite()
            || (self.sun_direction.length_squared() - 1.).abs() > 0.001
        {
            return Err(VoxelShadingError);
        }
        Ok(())
    }
    pub(super) fn encoded(self) -> [f32; 12] {
        [
            self.display as u32 as f32,
            self.illumination as u32 as f32,
            self.sky_brightness,
            1.,
            self.sun_direction.x,
            self.sun_direction.y,
            self.sun_direction.z,
            self.sun_intensity,
            self.sun_color[0],
            self.sun_color[1],
            self.sun_color[2],
            if self.color_space == TextureColorSpace::Srgb {
                1.
            } else {
                0.
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_frame_values_are_rejected() {
        assert!(VoxelShading::default().validate().is_ok());
        assert!(
            VoxelShading {
                sky_brightness: f32::NAN,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            VoxelShading {
                sun_direction: Vec3::ZERO,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            VoxelShading {
                shadow_strength: 2.,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
}
