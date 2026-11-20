//! Height-gradient and illumination algorithms for top-down terrain rendering.

use super::pipeline::TerrainLightingOptions;

/// Height-gradient estimator used to build the terrain normal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TerrainGradientAlgorithm {
    /// Horn's weighted 3×3 derivative, suitable for rough terrain.
    #[default]
    Horn,
    /// Zevenbergen–Thorne's cardinal derivative, suited to smoother terrain.
    ZevenbergenThorne,
    /// Scharr's 3×3 derivative, with stronger cardinal weighting.
    Scharr,
}

/// Illumination formula applied to the terrain normal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TerrainShadingMode {
    /// Shade from one directional light.
    #[default]
    Directional,
    /// Apply a fixed northwest-to-southeast drop shadow from neighboring heights.
    DirectionalDropShadow,
    /// Average four light directions to reduce azimuth-dependent contrast.
    MultiDirectional,
    /// Combine directional light with slope contrast.
    SlopeWeighted,
}

/// Returns the east-west and north-south gradients for a 3×3 height neighborhood.
///
/// Samples are ordered as center, northwest, north, northeast, west, east,
/// southwest, south, southeast. Missing neighbors must be filled by the caller
/// with the center height, matching the renderer's edge behavior.
pub(super) fn terrain_gradient(
    heights: [i16; 9],
    algorithm: TerrainGradientAlgorithm,
) -> (f32, f32) {
    let north_west = f32::from(heights[1]);
    let north = f32::from(heights[2]);
    let north_east = f32::from(heights[3]);
    let west = f32::from(heights[4]);
    let east = f32::from(heights[5]);
    let south_west = f32::from(heights[6]);
    let south = f32::from(heights[7]);
    let south_east = f32::from(heights[8]);

    match algorithm {
        TerrainGradientAlgorithm::Horn => {
            let dx =
                (north_east + 2.0 * east + south_east - north_west - 2.0 * west - south_west) / 8.0;
            let dz =
                (south_west + 2.0 * south + south_east - north_west - 2.0 * north - north_east)
                    / 8.0;
            (dx, dz)
        }
        TerrainGradientAlgorithm::ZevenbergenThorne => ((east - west) * 0.5, (south - north) * 0.5),
        TerrainGradientAlgorithm::Scharr => {
            let dx = (3.0 * (north_east - north_west)
                + 10.0 * (east - west)
                + 3.0 * (south_east - south_west))
                / 32.0;
            let dz = (3.0 * (south_west - north_west)
                + 10.0 * (south - north)
                + 3.0 * (south_east - north_east))
                / 32.0;
            (dx, dz)
        }
    }
}

/// Returns light-facing relief and its signed color adjustment.
pub(super) fn illumination(normal: [f32; 3], lighting: TerrainLightingOptions) -> (f32, f32) {
    let relative_light = match lighting.shading_mode {
        TerrainShadingMode::Directional => directional_relief(
            normal,
            lighting.light_azimuth_degrees,
            lighting.light_elevation_degrees,
        ),
        TerrainShadingMode::MultiDirectional => {
            let mut total = 0.0;
            for azimuth in [225.0_f32, 270.0, 315.0, 360.0] {
                total += directional_relief(normal, azimuth, lighting.light_elevation_degrees);
            }
            total * 0.25
        }
        TerrainShadingMode::SlopeWeighted => {
            let directional = directional_relief(
                normal,
                lighting.light_azimuth_degrees,
                lighting.light_elevation_degrees,
            );
            let slope = normal[0].hypot(normal[2]) / normal[1].max(f32::EPSILON);
            let slope_weight = slope / (1.0 + slope);
            directional * (0.7 + 0.3 * slope_weight) - slope_weight * 0.16
        }
        // `terrain_lit_color` handles this mode from the raw height samples.
        TerrainShadingMode::DirectionalDropShadow => 0.0,
    };
    let factor = if relative_light >= 0.0 {
        relative_light * lighting.highlight_strength * 100.0
    } else {
        relative_light * lighting.shadow_strength * 100.0
    };
    (relative_light, factor)
}

/// Returns the reference drop-shadow multiplier for a 3×3 height neighborhood.
///
/// Samples are ordered as center, northwest, north, northeast, west, east,
/// southwest, south, southeast. This reproduces BedrockMapRender's fixed
/// northwest-to-southeast neighbor rule and elevation multiplier.
pub(super) fn directional_drop_shadow_multiplier(heights: [i16; 9]) -> f32 {
    let center = f32::from(heights[0]);
    let shade_drop = [heights[1], heights[2], heights[4]]
        .into_iter()
        .map(|height| (f32::from(height) - center).max(0.0))
        .fold(0.0_f32, f32::max)
        .min(12.0);
    let light_drop = [heights[8], heights[7], heights[5]]
        .into_iter()
        .map(|height| (center - f32::from(height)).max(0.0))
        .fold(0.0_f32, f32::max)
        .min(12.0);
    let shadow = (1.0 - shade_drop * 0.18 + light_drop * 0.081).clamp(0.55, 1.18);
    let height_factor = ((center + 64.0) / 400.0 + 0.8).clamp(0.8, 1.05);
    shadow * height_factor
}

fn directional_relief(normal: [f32; 3], azimuth_degrees: f32, elevation_degrees: f32) -> f32 {
    let azimuth = azimuth_degrees.to_radians();
    let elevation = elevation_degrees.to_radians().clamp(0.01, 1.55);
    let light_horizontal = elevation.cos();
    let light_x = azimuth.sin() * light_horizontal;
    let light_y = elevation.sin();
    let light_z = -azimuth.cos() * light_horizontal;
    normal[0].mul_add(light_x, normal[1].mul_add(light_y, normal[2] * light_z)) - light_y
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ridge() -> [i16; 9] {
        [70, 62, 64, 68, 66, 78, 61, 72, 84]
    }

    #[test]
    fn gradient_estimators_produce_distinct_diagonal_slopes() {
        let heights = ridge();
        let horn = terrain_gradient(heights, TerrainGradientAlgorithm::Horn);
        let zevenbergen_thorne =
            terrain_gradient(heights, TerrainGradientAlgorithm::ZevenbergenThorne);
        let scharr = terrain_gradient(heights, TerrainGradientAlgorithm::Scharr);

        assert_ne!(horn, zevenbergen_thorne);
        assert_ne!(horn, scharr);
        assert_ne!(zevenbergen_thorne, scharr);
    }

    #[test]
    fn multidirectional_shading_ignores_single_light_azimuth() {
        let normal = [-0.35, 0.88, -0.31];
        let first = TerrainLightingOptions {
            shading_mode: TerrainShadingMode::MultiDirectional,
            light_azimuth_degrees: 30.0,
            ..TerrainLightingOptions::soft()
        };
        let second = TerrainLightingOptions {
            light_azimuth_degrees: 250.0,
            ..first
        };

        assert_eq!(illumination(normal, first), illumination(normal, second));
    }

    #[test]
    fn slope_weighted_shading_changes_steep_relief() {
        let normal = [-0.35, 0.88, -0.31];
        let directional = TerrainLightingOptions::soft();
        let slope_weighted = TerrainLightingOptions {
            shading_mode: TerrainShadingMode::SlopeWeighted,
            ..directional
        };

        assert_ne!(
            illumination(normal, directional),
            illumination(normal, slope_weighted)
        );
    }

    #[test]
    fn directional_drop_shadow_uses_northwest_shadow_and_southeast_highlight() {
        let flat = [64; 9];
        let mut northwest_ridge = flat;
        northwest_ridge[1] = 76;
        let mut southeast_slope = flat;
        southeast_slope[8] = 52;

        let flat_multiplier = directional_drop_shadow_multiplier(flat);
        assert!(directional_drop_shadow_multiplier(northwest_ridge) < flat_multiplier);
        assert!(directional_drop_shadow_multiplier(southeast_slope) > flat_multiplier);
    }

    #[test]
    fn directional_drop_shadow_clamps_neighbor_deltas_and_height_factor() {
        let mut moderate_ridge = [64; 9];
        moderate_ridge[1] = 76;
        let mut taller_ridge = moderate_ridge;
        taller_ridge[1] = 100;

        let below_world = [-64; 9];
        let above_world = [400; 9];

        assert_eq!(
            directional_drop_shadow_multiplier(moderate_ridge),
            directional_drop_shadow_multiplier(taller_ridge)
        );
        assert_eq!(directional_drop_shadow_multiplier(below_world), 0.8);
        assert_eq!(directional_drop_shadow_multiplier(above_world), 1.05);
    }
}
