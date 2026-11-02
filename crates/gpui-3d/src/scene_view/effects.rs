//! GPU-only screen effects and their bounded quality budget.
use crate::{TextureAssetId, Vec3};

/// Screen-space and atmospheric options. Disabled by default on ordinary SceneViews.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenEffects {
    /// Enables bounded screen-space reflection tracing, with sky fallback on misses.
    pub reflections: bool,
    /// Enables water refraction, absorption, Fresnel reflection and animated wave normals.
    pub water: bool,
    /// Enables height-dependent participating-media integration.
    pub volumetric_fog: bool,
    /// Enables material-driven cloud volumes; requires a cloud texture.
    pub clouds: bool,
    /// Optional density/coverage material. Its alpha channel defines cloud coverage.
    pub cloud_texture: Option<TextureAssetId>,
    /// Optional linear RGB background, replacing the analytic sky and celestial disk.
    pub background_color: Option<[f32; 3]>,
    /// Internal scene resolution relative to the element, in 0.25..=1.
    pub resolution_scale: f32,
    /// Linear exposure before tone mapping.
    pub exposure: f32,
    /// Atmospheric extinction per world unit, in 0..=1.
    pub fog_density: f32,
    /// Cloud layer base height in world units.
    pub cloud_height: f32,
    /// Absolute origin for origin-rebased scenes.
    pub world_origin: Vec3,
    /// Whether the camera is submerged in water.
    pub underwater: bool,
}
impl Default for ScreenEffects {
    fn default() -> Self {
        Self {
            reflections: true,
            water: true,
            volumetric_fog: true,
            clouds: true,
            cloud_texture: None,
            background_color: None,
            resolution_scale: 0.75,
            exposure: 1.,
            fog_density: 0.004,
            cloud_height: 192.,
            world_origin: Vec3::ZERO,
            underwater: false,
        }
    }
}
impl ScreenEffects {
    pub(super) fn validate(self) -> gpui::Result<()> {
        if !self.resolution_scale.is_finite()
            || !(0.25..=1.).contains(&self.resolution_scale)
            || !self.exposure.is_finite()
            || self.exposure <= 0.
            || !self.fog_density.is_finite()
            || !(0. ..=1.).contains(&self.fog_density)
            || !self.cloud_height.is_finite()
            || !self.world_origin.length_squared().is_finite()
            || self.background_color.is_some_and(|color| {
                color
                    .into_iter()
                    .any(|value| !value.is_finite() || value < 0.)
            })
        {
            anyhow::bail!(
                "screen effects require finite parameters and a normalized resolution scale"
            );
        }
        Ok(())
    }
}

mod compositor;
mod construction;
mod light_tiles;
mod renderer;
mod scene;
mod targets;
pub(super) use renderer::EffectsRenderer;
