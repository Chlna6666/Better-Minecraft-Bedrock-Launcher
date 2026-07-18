use super::*;

pub(super) fn write_animation_binding(
    bytes: &mut Vec<u8>,
    animation_id: crate::SceneAnimationId,
    primitive_kind: NovaAnimatedPrimitiveKind,
    primitive_index: u32,
) {
    write_u32_vec(bytes, animation_id.0);
    write_u32_vec(bytes, primitive_kind as u32);
    write_u32_vec(bytes, primitive_index);
    write_u32_vec(bytes, 0);
}

pub(super) fn write_animation_value(
    bytes: &mut Vec<u8>,
    animation_id: crate::SceneAnimationId,
    property: NovaAnimationProperty,
    progress: f32,
    from: [f32; 4],
    to: [f32; 4],
) {
    let progress = if progress.is_finite() {
        progress.clamp(0.0, 1.0)
    } else {
        0.0
    };

    write_u32_vec(bytes, animation_id.0);
    write_u32_vec(bytes, property as u32);
    write_f32_vec(bytes, progress);
    write_u32_vec(bytes, 0);
    for value in from {
        write_f32_vec(bytes, value);
    }
    for value in to {
        write_f32_vec(bytes, value);
    }
    for _ in 0..4 {
        write_u32_vec(bytes, 0);
    }
}

pub(super) fn write_custom_mesh_3d_parameters(
    bytes: &mut Vec<u8>,
    painted: &crate::PaintGpuMesh3d,
) {
    write_bounds_scaled(bytes, &painted.bounds);
    write_bounds_scaled(bytes, &painted.content_mask.bounds);
    write_matrix(bytes, painted.parameters.view_projection_model);
}

pub(super) fn write_custom_mesh_3d_vertex(bytes: &mut Vec<u8>, vertex: crate::GpuMesh3dVertex) {
    for value in vertex.position {
        write_f32_vec(bytes, value);
    }
    for value in vertex.color {
        write_f32_vec(bytes, value);
    }
}

pub(super) fn write_custom_mesh_3d_index(bytes: &mut Vec<u8>, index: u32) {
    write_u32_vec(bytes, index);
}

pub(super) fn write_backdrop_blur_pass(bytes: &mut Vec<u8>, offset: f32) {
    write_f32_vec(bytes, offset);
    write_f32_vec(bytes, 0.0);
    write_f32_vec(bytes, 0.0);
    write_u32_vec(bytes, 0);
}

pub(super) fn write_backdrop_blur(
    bytes: &mut Vec<u8>,
    blur: &crate::PaintBackdropBlur,
    drawable_size: DrawableSize,
) {
    write_u32_vec(bytes, blur.order);
    write_u32_vec(bytes, u32::from(blur.downsample));
    write_u32_vec(bytes, u32::from(blur.levels.clamp(1, 6)));
    write_u32_vec(bytes, 0);
    write_bounds_scaled(bytes, &blur.bounds);
    write_content_mask(bytes, &blur.content_mask);
    write_corners(bytes, &blur.corner_radii);
    write_hsla(
        bytes,
        blur.tint.unwrap_or_else(crate::Hsla::transparent_black),
    );
    write_f32_vec(bytes, blur.radius.0);
    write_f32_vec(bytes, blur.saturation);
    write_f32_vec(bytes, drawable_size.width as f32);
    write_f32_vec(bytes, drawable_size.height as f32);
    write_u32_vec(bytes, 0);
    write_u32_vec(bytes, 0);
}

pub(super) fn backdrop_blur_offset(radius: f32, downsample: u8, levels: u8) -> f32 {
    let downsample = f32::from(downsample.max(1));
    let levels = f32::from(levels.clamp(1, 6));
    (radius / downsample / levels).clamp(0.5, 6.0)
}

pub(super) fn write_quad(bytes: &mut Vec<u8>, quad: &Quad) {
    write_u32_vec(bytes, quad.order);
    write_u32_vec(bytes, quad.border_style as u32);
    write_bounds_scaled(bytes, &quad.bounds);
    write_content_mask(bytes, &quad.content_mask);
    write_background(bytes, &quad.background);
    write_hsla(bytes, quad.border_color);
    write_corners(bytes, &quad.corner_radii);
    write_edges(bytes, &quad.border_widths);
}

pub(super) fn write_shadow(bytes: &mut Vec<u8>, shadow: &Shadow) {
    write_u32_vec(bytes, shadow.order);
    write_f32_vec(bytes, shadow.blur_radius.0);
    write_bounds_scaled(bytes, &shadow.bounds);
    write_corners(bytes, &shadow.corner_radii);
    write_content_mask(bytes, &shadow.content_mask);
    write_hsla(bytes, shadow.color);
}

pub(super) fn write_path_rasterization_vertex(
    bytes: &mut Vec<u8>,
    vertex: &crate::PathVertex_ScaledPixels,
    background: &crate::Background,
    content_mask: &crate::ContentMask<crate::ScaledPixels>,
) {
    write_f32_vec(bytes, vertex.xy_position.x.0);
    write_f32_vec(bytes, vertex.xy_position.y.0);
    write_f32_vec(bytes, vertex.st_position.x);
    write_f32_vec(bytes, vertex.st_position.y);
    write_background(bytes, background);
    write_content_mask(bytes, content_mask);
}

pub(super) fn write_path_sprite(bytes: &mut Vec<u8>, bounds: &Bounds<crate::ScaledPixels>) {
    write_bounds_scaled(bytes, bounds);
}

pub(super) fn write_monochrome_sprite(bytes: &mut Vec<u8>, sprite: &MonochromeSprite) {
    write_u32_vec(bytes, sprite.order);
    write_u32_vec(bytes, sprite.pad);
    write_bounds_scaled(bytes, &sprite.bounds);
    write_content_mask(bytes, &sprite.content_mask);
    write_hsla(bytes, sprite.color);
    write_atlas_tile(bytes, &sprite.tile);
    write_transformation(bytes, &sprite.transformation);
}

pub(super) fn write_polychrome_sprite(bytes: &mut Vec<u8>, sprite: &PolychromeSprite) {
    write_u32_vec(bytes, sprite.order);
    write_u32_vec(bytes, sprite.pad);
    write_u32_vec(bytes, u32::from(sprite.grayscale));
    write_f32_vec(bytes, sprite.opacity);
    write_bounds_scaled(bytes, &sprite.bounds);
    write_content_mask(bytes, &sprite.content_mask);
    write_corners(bytes, &sprite.corner_radii);
    write_atlas_tile(bytes, &sprite.tile);
}

pub(super) fn write_underline(bytes: &mut Vec<u8>, underline: &Underline) {
    write_u32_vec(bytes, underline.order);
    write_u32_vec(bytes, underline.pad);
    write_bounds_scaled(bytes, &underline.bounds);
    write_content_mask(bytes, &underline.content_mask);
    write_hsla(bytes, underline.color);
    write_f32_vec(bytes, underline.thickness.0);
    write_u32_vec(bytes, underline.wavy);
}

pub(super) fn write_background(bytes: &mut Vec<u8>, background: &crate::Background) {
    write_u32_vec(bytes, background.tag as u32);
    write_u32_vec(bytes, background.color_space as u32);
    write_hsla(bytes, background.solid);
    write_f32_vec(bytes, background.gradient_angle_or_pattern_height);
    for stop in background.colors {
        write_hsla(bytes, stop.color);
        write_f32_vec(bytes, stop.percentage);
    }
    write_u32_vec(bytes, 0);
}

pub(super) fn write_bounds_scaled(bytes: &mut Vec<u8>, bounds: &Bounds<crate::ScaledPixels>) {
    write_f32_vec(bytes, bounds.origin.x.0);
    write_f32_vec(bytes, bounds.origin.y.0);
    write_f32_vec(bytes, bounds.size.width.0);
    write_f32_vec(bytes, bounds.size.height.0);
}

pub(super) fn write_bounds_device(bytes: &mut Vec<u8>, bounds: &Bounds<DevicePixels>) {
    write_i32_vec(bytes, bounds.origin.x.0);
    write_i32_vec(bytes, bounds.origin.y.0);
    write_i32_vec(bytes, bounds.size.width.0);
    write_i32_vec(bytes, bounds.size.height.0);
}

pub(super) fn write_corners(bytes: &mut Vec<u8>, corners: &crate::Corners<crate::ScaledPixels>) {
    write_f32_vec(bytes, corners.top_left.0);
    write_f32_vec(bytes, corners.top_right.0);
    write_f32_vec(bytes, corners.bottom_right.0);
    write_f32_vec(bytes, corners.bottom_left.0);
}

pub(super) fn write_content_mask(
    bytes: &mut Vec<u8>,
    content_mask: &crate::ContentMask<crate::ScaledPixels>,
) {
    write_bounds_scaled(bytes, &content_mask.bounds);
    write_bounds_scaled(bytes, &content_mask.corner_bounds);
    write_corners(bytes, &content_mask.corner_radii);
}

pub(super) fn write_edges(bytes: &mut Vec<u8>, edges: &crate::Edges<crate::ScaledPixels>) {
    write_f32_vec(bytes, edges.top.0);
    write_f32_vec(bytes, edges.right.0);
    write_f32_vec(bytes, edges.bottom.0);
    write_f32_vec(bytes, edges.left.0);
}

pub(super) fn write_hsla(bytes: &mut Vec<u8>, color: crate::Hsla) {
    write_f32_vec(bytes, color.h);
    write_f32_vec(bytes, color.s);
    write_f32_vec(bytes, color.l);
    write_f32_vec(bytes, color.a);
}

pub(super) fn write_atlas_tile(bytes: &mut Vec<u8>, tile: &AtlasTile) {
    write_u32_vec(bytes, tile.texture_id.index);
    write_u32_vec(bytes, tile.texture_id.kind as u32);
    write_u32_vec(bytes, tile.tile_id.0);
    write_u32_vec(bytes, tile.padding);
    write_bounds_device(bytes, &tile.bounds);
}

pub(super) fn write_transformation(bytes: &mut Vec<u8>, transform: &crate::TransformationMatrix) {
    for row in transform.rotation_scale {
        for value in row {
            write_f32_vec(bytes, value);
        }
    }
    for value in transform.translation {
        write_f32_vec(bytes, value);
    }
}

pub(super) fn write_u32_vec(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_ne_bytes());
}

fn write_i32_vec(bytes: &mut Vec<u8>, value: i32) {
    bytes.extend_from_slice(&value.to_ne_bytes());
}

pub(super) fn write_f32_vec(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_ne_bytes());
}

fn write_matrix(bytes: &mut Vec<u8>, matrix: [[f32; 4]; 4]) {
    for column in matrix {
        for value in column {
            write_f32_vec(bytes, value);
        }
    }
}
