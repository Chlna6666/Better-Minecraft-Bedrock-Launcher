use image::{DynamicImage, GenericImageView as _, Rgba};

#[derive(Clone, Copy)]
pub(super) enum Face {
    Top,
    Bottom,
    Right,
    Front,
    Left,
    Back,
}

pub(super) fn sample_skin_color(
    image: &DynamicImage,
    unit: u32,
    skin_x: u32,
    skin_y: u32,
) -> [f32; 4] {
    let (width, height) = image.dimensions();
    let x = skin_x
        .saturating_mul(unit)
        .saturating_add(unit / 2)
        .min(width.saturating_sub(1));
    let y = skin_y
        .saturating_mul(unit)
        .saturating_add(unit / 2)
        .min(height.saturating_sub(1));
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

fn rgba_to_color(pixel: Rgba<u8>) -> [f32; 4] {
    [
        f32::from(pixel[0]) / 255.0,
        f32::from(pixel[1]) / 255.0,
        f32::from(pixel[2]) / 255.0,
        f32::from(pixel[3]) / 255.0,
    ]
}
