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

pub(super) fn shade_face_color(color: [f32; 4], face: Face) -> [f32; 4] {
    let factor = match face {
        Face::Top => 1.08,
        Face::Bottom => 0.64,
        Face::Right | Face::Left => 0.78,
        Face::Back => 0.70,
        Face::Front => 1.0,
    };
    [
        (color[0] * factor).min(1.0),
        (color[1] * factor).min(1.0),
        (color[2] * factor).min(1.0),
        color[3],
    ]
}

pub(super) fn shade_layer_edge_color(color: [f32; 4], normal: [f32; 3]) -> [f32; 4] {
    let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    let normal_y = if length <= f32::EPSILON {
        0.0
    } else {
        normal[1] / length
    };
    let factor = if normal_y > 0.45 {
        0.94
    } else if normal_y < -0.45 {
        0.52
    } else {
        0.70
    };

    [
        (color[0] * factor).min(1.0),
        (color[1] * factor).min(1.0),
        (color[2] * factor).min(1.0),
        color[3],
    ]
}

fn rgba_to_color(pixel: Rgba<u8>) -> [f32; 4] {
    [
        f32::from(pixel[0]) / 255.0,
        f32::from(pixel[1]) / 255.0,
        f32::from(pixel[2]) / 255.0,
        f32::from(pixel[3]) / 255.0,
    ]
}
