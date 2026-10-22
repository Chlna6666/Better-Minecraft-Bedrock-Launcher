use super::*;

#[cfg(test)]
pub(super) fn write_animation_binding(
    bytes: &mut Vec<u8>,
    animation_id: crate::SceneAnimationId,
    primitive_kind: AnimatedPrimitiveKind,
    primitive_index: u32,
) {
    let mut record = [0_u8; PACKED_ANIMATION_BINDING_BYTES];
    record[0..4].copy_from_slice(&animation_id.0.to_ne_bytes());
    record[4..8].copy_from_slice(&(primitive_kind as u32).to_ne_bytes());
    record[8..12].copy_from_slice(&primitive_index.to_ne_bytes());
    bytes.extend_from_slice(&record);
}

pub(super) fn write_animation_value(
    bytes: &mut Vec<u8>,
    animation_id: crate::SceneAnimationId,
    property: AnimationProperty,
    progress: f32,
    from: [f32; 4],
    to: [f32; 4],
) {
    let mut record = [0_u8; PACKED_ANIMATION_VALUE_BYTES];
    write_animation_value_at(&mut record, animation_id, property, progress, from, to);
    bytes.extend_from_slice(&record);
}

pub(super) fn write_animation_value_at(
    record: &mut [u8],
    animation_id: crate::SceneAnimationId,
    property: AnimationProperty,
    progress: f32,
    from: [f32; 4],
    to: [f32; 4],
) {
    debug_assert_eq!(record.len(), PACKED_ANIMATION_VALUE_BYTES);
    record.fill(0);
    record[0..4].copy_from_slice(&animation_id.0.to_ne_bytes());
    record[4..8].copy_from_slice(&(property as u32).to_ne_bytes());
    let progress = if progress.is_finite() { progress } else { 0.0 };
    record[8..12].copy_from_slice(&progress.to_ne_bytes());

    let mut offset = 16;
    for value in from.into_iter().chain(to) {
        record[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
        offset += 4;
    }
    // The final 16 bytes are ABI padding and remain zeroed.
}

/// Packs a normalized Gaussian kernel as four sample pairs plus the center tap.
///
/// The old shader recomputed eight exponentials, four paired offsets, and normalization for every
/// fragment. These values depend only on the blur radius, so compute them once per pass on the CPU.
#[cfg(test)]
pub(super) fn write_backdrop_blur_pass(bytes: &mut Vec<u8>, radius: f32) {
    write_animated_blur_pass(bytes, radius, 0);
}

pub(super) fn write_animated_blur_pass(
    bytes: &mut Vec<u8>,
    radius: f32,
    animation_slot_plus_one: u32,
) {
    let radius = if radius.is_finite() {
        radius.max(1.0 / 4096.0)
    } else {
        1.0 / 4096.0
    };
    // CSS blur() measures the Gaussian standard deviation, not the kernel support.
    let sigma = radius;
    let support = 3.0 * sigma;
    let adjacent_taps = support <= 8.0;
    let tap_step = if adjacent_taps { 1.0 } else { support / 8.0 };
    let gaussian = |distance: f32| {
        if adjacent_taps && distance > support {
            0.0
        } else {
            (-(distance * distance) / (2.0 * sigma * sigma).max(1e-8)).exp()
        }
    };

    let center_weight = gaussian(0.0);
    let mut offsets = [0.0_f32; 4];
    let mut pair_weights = [0.0_f32; 4];
    let mut weight_sum = center_weight;
    for pair in 0..4 {
        let tap0 = (pair * 2 + 1) as f32 * tap_step;
        let tap1 = (pair * 2 + 2) as f32 * tap_step;
        let weight0 = gaussian(tap0);
        let weight1 = gaussian(tap1);
        let pair_weight = weight0 + weight1;
        offsets[pair] = if pair_weight > 1e-8 {
            (tap0 * weight0 + tap1 * weight1) / pair_weight
        } else {
            tap0
        };
        pair_weights[pair] = pair_weight;
        weight_sum += pair_weight * 2.0;
    }

    let normalization = weight_sum.max(1e-8).recip();
    for offset in offsets {
        write_f32_vec(bytes, offset);
    }
    for weight in pair_weights {
        write_f32_vec(bytes, weight * normalization);
    }
    write_f32_vec(bytes, center_weight * normalization);
    write_f32_vec(bytes, if adjacent_taps { 1.0 } else { 0.0 });
    // center_and_pad.z is an ABI-stable padding lane for animation_slot + 1.
    write_u32_vec(bytes, animation_slot_plus_one);
    write_u32_vec(bytes, 0);
}

pub(super) fn write_backdrop_blur(
    bytes: &mut Vec<u8>,
    blur: &crate::PaintBackdropBlur,
    target_size: DrawableSize,
) {
    write_u32_vec(bytes, blur.order);
    write_u32_vec(bytes, u32::from(blur.downsample));
    write_u32_vec(bytes, u32::from(blur.levels.clamp(1, 6)));
    write_u32_vec(bytes, u32::from(blur.recompute_overlap));
    write_bounds_scaled(bytes, &blur.bounds);
    write_content_mask(bytes, &blur.content_mask);
    write_corners(bytes, &blur.corner_radii);
    write_hsla(
        bytes,
        blur.tint.unwrap_or_else(crate::Hsla::transparent_black),
    );
    write_f32_vec(bytes, blur.radius.0);
    write_f32_vec(bytes, blur.saturation);
    write_f32_vec(bytes, target_size.width as f32);
    write_f32_vec(bytes, target_size.height as f32);
    write_f32_vec(bytes, blur.opacity);
    // Shared compositor kind: 0 keeps the auxiliary 16-byte slot interpreted as HSLA tint.
    write_u32_vec(bytes, 0);
}

/// Packs an element blur into the shared blur primitive layout.
///
/// Element blur uses the same Gaussian and composite pipelines as backdrop blur, but its source
/// is the isolated content range rendered between the matching upload markers. The 16-byte tint
/// slot is unused for element blur, so retain the 136-byte ABI and store the source sampling bounds
/// there. The final u32 tags the record so the shader can distinguish source bounds from HSLA tint.
pub(super) fn write_paint_blur(
    bytes: &mut Vec<u8>,
    blur: &crate::PaintBlur,
    target_size: DrawableSize,
) {
    write_paint_blur_fields(
        bytes,
        blur.order,
        blur.bounds,
        blur.content_mask,
        blur.radius,
        blur.opacity,
        target_size,
    );
}

/// Encode a compositor/filter record without constructing a synthetic Scene.
/// Shader ABI is identical for real element filters and GPU-owned root color.
pub(super) fn write_paint_blur_fields(
    bytes: &mut Vec<u8>,
    order: crate::DrawOrder,
    bounds: crate::Bounds<crate::ScaledPixels>,
    content_mask: crate::ContentMask<crate::ScaledPixels>,
    radius: crate::ScaledPixels,
    opacity: f32,
    target_size: DrawableSize,
) {
    write_u32_vec(bytes, order);
    write_u32_vec(bytes, 1);
    write_u32_vec(bytes, 1);
    // Zero-radius compositor outputs are independent GPU targets: merging
    // them would clear one layer while another still samples its contents.
    write_u32_vec(bytes, u32::from(radius.0 <= 0.0));
    write_bounds_scaled(bytes, &bounds);
    write_content_mask(bytes, &content_mask);
    write_corners(bytes, &Default::default());
    write_bounds_scaled(bytes, &bounds);
    write_f32_vec(bytes, radius.0);
    write_f32_vec(bytes, 1.0);
    write_f32_vec(bytes, target_size.width as f32);
    write_f32_vec(bytes, target_size.height as f32);
    write_f32_vec(bytes, opacity);
    write_u32_vec(bytes, 1);
}

#[inline]
fn write_animation_slot_placeholder(bytes: &mut Vec<u8>) {
    // Scene batching has already consumed draw order. Nova's ordinary primitive shaders use this
    // first u32 exclusively as animation_slot, so emit the zero sentinel directly instead of
    // writing order and clearing every packed record in a second promotion pass.
    write_u32_vec(bytes, 0);
}

pub(super) fn write_quad(bytes: &mut Vec<u8>, quad: &Quad) {
    write_animation_slot_placeholder(bytes);
    write_u32_vec(bytes, quad.border_style as u32);
    write_bounds_scaled(bytes, &quad.bounds);
    write_content_mask(bytes, &quad.content_mask);
    write_background(bytes, &quad.background);
    write_rgba(bytes, quad.border_color);
    write_corners(bytes, &quad.corner_radii);
    write_edges(bytes, &quad.border_widths);
}

pub(super) fn write_shadow(bytes: &mut Vec<u8>, shadow: &Shadow) {
    write_animation_slot_placeholder(bytes);
    write_f32_vec(bytes, shadow.blur_radius.0);
    write_bounds_scaled(bytes, &shadow.bounds);
    write_corners(bytes, &shadow.corner_radii);
    write_content_mask(bytes, &shadow.content_mask);
    write_rgba(bytes, shadow.color);
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
    write_animation_slot_placeholder(bytes);
    write_u32_vec(bytes, sprite.pad);
    write_bounds_scaled(bytes, &sprite.bounds);
    write_content_mask(bytes, &sprite.content_mask);
    write_rgba(bytes, sprite.color);
    write_atlas_tile(bytes, &sprite.tile);
    write_transformation(bytes, &sprite.transformation);
}

pub(super) fn write_polychrome_sprite(bytes: &mut Vec<u8>, sprite: &PolychromeSprite) {
    write_animation_slot_placeholder(bytes);
    write_u32_vec(bytes, sprite.sampling);
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
    write_rgba(bytes, underline.color);
    write_f32_vec(bytes, underline.thickness.0);
    write_u32_vec(bytes, underline.wavy);
}

pub(super) fn write_background(bytes: &mut Vec<u8>, background: &crate::Background) {
    write_u32_vec(bytes, background.tag as u32);
    write_u32_vec(bytes, background.color_space as u32);
    write_rgba(bytes, background.solid);
    write_f32_vec(bytes, background.gradient_angle_or_pattern_height);
    for stop in background.colors {
        write_rgba(bytes, stop.color);
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

pub(super) fn write_rgba(bytes: &mut Vec<u8>, color: crate::Rgba) {
    write_f32_vec(bytes, color.r);
    write_f32_vec(bytes, color.g);
    write_f32_vec(bytes, color.b);
    write_f32_vec(bytes, color.a);
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

#[cfg(test)]
mod compositor_record_tests {
    use super::*;

    #[test]
    fn field_encoder_matches_element_blur_shader_abi() {
        let size = DrawableSize {
            width: 1280,
            height: 720,
        };
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(12.0), crate::ScaledPixels(24.0)),
            crate::size(crate::ScaledPixels(300.0), crate::ScaledPixels(180.0)),
        );
        for radius in [0.0, 4.5] {
            let blur = crate::PaintBlur {
                order: 21,
                animation_id: None,
                layer_id: None,
                bounds,
                content_mask: crate::ContentMask::new(bounds),
                radius: crate::ScaledPixels(radius),
                opacity: 0.75,
                content: std::sync::Arc::new(crate::Scene::default()),
            };
            let mut element = Vec::new();
            write_paint_blur(&mut element, &blur, size);
            let mut direct = Vec::new();
            write_paint_blur_fields(
                &mut direct,
                blur.order,
                blur.bounds,
                blur.content_mask,
                blur.radius,
                blur.opacity,
                size,
            );
            assert_eq!(direct.len(), PACKED_BACKDROP_BLUR_BYTES);
            assert_eq!(direct, element);
        }
    }
}
