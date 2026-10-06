use image::{DynamicImage, GenericImageView as _, Rgba};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Face {
    Top,
    Bottom,
    Right,
    Front,
    Left,
    Back,
}

pub(super) fn sample_image_color(image: &DynamicImage, image_x: u32, image_y: u32) -> [f32; 4] {
    let (width, height) = image.dimensions();
    let x = image_x.min(width.saturating_sub(1));
    let y = image_y.min(height.saturating_sub(1));
    rgba_to_color(image.get_pixel(x, y))
}

/// Skin preview is intentionally flat/unlit: texture pixels must be displayed without any
/// view-dependent or face-dependent darkening. Keeping this helper neutral also covers the
/// CPU-baked geometry/layer paths that share the same color pipeline.
pub(super) fn shade_face_color(color: [f32; 4], _face: Face) -> [f32; 4] {
    color
}

/// Neutral face multiplier for texture-mapped cuboids.
///
/// The material already uses `ShadingModel::Unlit`; a non-1.0 vertex multiplier would reintroduce
/// fake lighting even though the renderer itself is unlit.
pub(super) fn shade_cuboid_face(_face: Face) -> f32 {
    1.0
}

/// Keep custom-geometry and extruded-layer texels at their authored color.
///
/// Normals remain available for geometry/raycast work, but they must not alter skin preview color.
pub(super) fn shade_layer_edge_color(color: [f32; 4], _normal: [f32; 3]) -> [f32; 4] {
    color
}

fn rgba_to_color(pixel: Rgba<u8>) -> [f32; 4] {
    [
        f32::from(pixel[0]) / 255.0,
        f32::from(pixel[1]) / 255.0,
        f32::from(pixel[2]) / 255.0,
        f32::from(pixel[3]) / 255.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skin_preview_color_helpers_are_flat_unlit() {
        let color = [0.25, 0.5, 0.75, 0.4];
        for face in [
            Face::Top,
            Face::Bottom,
            Face::Right,
            Face::Front,
            Face::Left,
            Face::Back,
        ] {
            assert_eq!(shade_face_color(color, face), color);
            assert_eq!(shade_cuboid_face(face), 1.0);
        }

        for normal in [
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
        ] {
            assert_eq!(shade_layer_edge_color(color, normal), color);
        }
    }
}
