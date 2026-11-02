//! Product rendering presets translated into generic 3D shader parameters.
use gpui_3d::{Vec3, VoxelDisplay, VoxelIllumination, VoxelShading};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum LightingMode {
    #[default]
    NightVision,
    Vanilla,
    VibrantPreview,
}

impl LightingMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::NightVision => "夜视（默认）",
            Self::Vanilla => "普通原版",
            Self::VibrantPreview => "灵动视效预览（实验）",
        }
    }
    pub(super) fn is_lit(self) -> bool {
        self != Self::NightVision
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RenderSettings {
    pub lighting: LightingMode,
    pub display: VoxelDisplay,
    pub fog: bool,
    pub shadows: bool,
    pub time: f32,
    pub cycle: bool,
    pub clouds: bool,
    pub reflections: bool,
    pub resolution_scale: f32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            lighting: LightingMode::NightVision,
            display: VoxelDisplay::Textured,
            fog: false,
            shadows: false,
            time: 0.25,
            cycle: false,
            clouds: true,
            reflections: true,
            resolution_scale: 0.75,
        }
    }
}

impl RenderSettings {
    pub(super) fn shading_for_dimension(
        self,
        dimension: bedrock_render::Dimension,
    ) -> VoxelShading {
        let mut shading = self.shading();
        if dimension != bedrock_render::Dimension::Overworld {
            shading.sun_intensity = 0.;
            shading.shadow_strength = 0.;
            shading.sky_brightness = if dimension == bedrock_render::Dimension::Nether {
                0.25
            } else {
                0.15
            };
        }
        shading
    }
    pub(super) fn effects(
        self,
        cloud_texture: Option<gpui_3d::TextureAssetId>,
        origin: [i32; 3],
        overworld: bool,
    ) -> Option<gpui_3d::ScreenEffects> {
        (self.lighting == LightingMode::VibrantPreview && self.display == VoxelDisplay::Textured)
            .then_some(gpui_3d::ScreenEffects {
                cloud_texture,
                clouds: overworld && self.clouds,
                reflections: self.reflections,
                resolution_scale: self.resolution_scale,
                volumetric_fog: self.fog,
                world_origin: Vec3::new(origin[0] as f32, origin[1] as f32, origin[2] as f32),
                ..gpui_3d::ScreenEffects::default()
            })
    }
    pub(super) fn shading(self) -> VoxelShading {
        let angle = (self.time - 0.25) * std::f32::consts::TAU;
        let sun = Vec3::new(angle.sin(), angle.cos(), 0.3)
            .normalized()
            .unwrap_or(Vec3::Y);
        let daylight = ((sun.y + 0.1) / 0.6).clamp(0., 1.);
        VoxelShading {
            color_space: gpui_3d::TextureColorSpace::Srgb,
            display: self.display,
            illumination: match self.lighting {
                LightingMode::NightVision => VoxelIllumination::Unlit,
                LightingMode::Vanilla => VoxelIllumination::Vertex,
                LightingMode::VibrantPreview => VoxelIllumination::Sunlight,
            },
            sky_brightness: 0.08 + 0.92 * daylight,
            sun_direction: sun,
            sun_color: [1., 0.88 + 0.12 * daylight, 0.7 + 0.3 * daylight],
            sun_intensity: daylight,
            shadow_strength: if self.shadows
                && self.lighting == LightingMode::VibrantPreview
                && self.display != VoxelDisplay::Wireframe
            {
                0.75 * daylight
            } else {
                0.
            },
        }
    }
}

pub(super) fn linear_rgb(color: [f32; 3]) -> [f32; 3] {
    color.map(|value| {
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_is_unlit_and_night_does_not_dim_block_emission() {
        let settings = RenderSettings::default();
        assert!(!settings.lighting.is_lit());
        assert!(!settings.fog);
        assert!(!settings.shadows);
        assert_eq!(settings.shading().illumination, VoxelIllumination::Unlit);
        let day = RenderSettings {
            lighting: LightingMode::Vanilla,
            ..settings
        }
        .shading();
        let night = RenderSettings {
            lighting: LightingMode::Vanilla,
            time: 0.75,
            ..settings
        }
        .shading();
        assert!(day.sky_brightness > night.sky_brightness);
    }
}
