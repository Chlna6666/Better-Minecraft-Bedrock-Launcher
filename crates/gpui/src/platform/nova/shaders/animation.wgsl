// Renderer-owned indexed visual animation shared by ordinary 2D Nova primitives.
// One static primitive stores animation_slot + 1; many primitives may point at one 64-byte value.

struct AnimationValue {
    animation_id: u32,
    property: u32,
    progress: f32,
    enabled: u32,
    from_value: vec4<f32>,
    to_value: vec4<f32>,
    pad: vec4<u32>,
}
@group(0) @binding(17) var<storage, read> b_animation_values: array<AnimationValue>;

struct VisualAnimation {
    translation: vec2<f32>,
    origin: vec2<f32>,
    scale: f32,
    opacity: f32,
    scales_geometry: u32,
    clip_top: f32,
    clip_bottom: f32,
    clips_geometry: u32,
}

fn resolve_visual_animation(slot_plus_one: u32, bounds: Bounds) -> VisualAnimation {
    var animation: VisualAnimation;
    animation.translation = vec2<f32>(0.0);
    animation.origin = bounds.origin + bounds.size * 0.5;
    animation.scale = 1.0;
    animation.opacity = 1.0;
    animation.scales_geometry = 0u;
    animation.clip_top = 0.0;
    animation.clip_bottom = 0.0;
    animation.clips_geometry = 0u;

    // GlobalParams.pad is renderer-owned in Nova and becomes an ABI feature gate. This keeps the
    // same shader binaries safe for static streams whose first u32 still contains legacy draw order.
    if (globals.pad == 0u || slot_plus_one == 0u) {
        return animation;
    }

    let value = b_animation_values[slot_plus_one - 1u];
    if (value.enabled == 0u) {
        return animation;
    }
    // Rust's animation packer normalizes non-finite progress to zero before upload.
    let sampled = value.from_value + (value.to_value - value.from_value) * value.progress;

    switch value.property {
        // Opacity
        case 0u: {
            animation.opacity = clamp(sampled.x, 0.0, 1.0);
        }
        // Transform: scale + opacity around a shared explicit origin.
        case 1u: {
            animation.scale = max(sampled.x, 0.0);
            animation.opacity = clamp(sampled.y, 0.0, 1.0);
            animation.origin = sampled.zw;
            animation.scales_geometry = 1u;
        }
        // Translation moves primitive bounds only. The clip mask intentionally stays fixed, which
        // matches the existing CPU animation semantics.
        case 2u: {
            animation.translation = sampled.xy;
        }
        // Scale around the primitive's static center.
        case 3u: {
            animation.scale = max(sampled.x, 0.0);
            animation.scales_geometry = 1u;
        }
        // Paint-only vertical reveal. Values are absolute device-pixel edges shared by the
        // retained subtree composite, so individual glyph and image bounds do not move.
        case 8u: {
            animation.clip_top = min(sampled.x, sampled.y);
            animation.clip_bottom = max(sampled.x, sampled.y);
            animation.clips_geometry = 1u;
        }
        // Rotation is promoted to a retained subtree composite; raw 2D primitives must not rotate
        // independently. Other transition properties are likewise no-ops in the old CPU path.
        default: {}
    }
    return animation;
}

fn animation_bounds(bounds: Bounds, animation: VisualAnimation) -> Bounds {
    var result = bounds;
    if (animation.scales_geometry != 0u) {
        result.origin = animation.origin + (bounds.origin - animation.origin) * animation.scale;
        result.size = bounds.size * animation.scale;
    }
    result.origin += animation.translation;
    return result;
}

fn animation_content_mask(mask: ContentMask, animation: VisualAnimation) -> ContentMask {
    var result = mask;
    if (animation.scales_geometry != 0u) {
        result.bounds.origin =
            animation.origin + (mask.bounds.origin - animation.origin) * animation.scale;
        result.bounds.size = mask.bounds.size * animation.scale;
        result.corner_bounds.origin =
            animation.origin + (mask.corner_bounds.origin - animation.origin) * animation.scale;
        result.corner_bounds.size = mask.corner_bounds.size * animation.scale;
        result.corner_radii.top_left *= animation.scale;
        result.corner_radii.top_right *= animation.scale;
        result.corner_radii.bottom_right *= animation.scale;
        result.corner_radii.bottom_left *= animation.scale;
    }
    if (animation.clips_geometry != 0u) {
        let mask_bottom = result.bounds.origin.y + result.bounds.size.y;
        let clipped_top = max(result.bounds.origin.y, animation.clip_top);
        let clipped_bottom = min(mask_bottom, animation.clip_bottom);
        result.bounds.origin.y = clipped_top;
        result.bounds.size.y = max(clipped_bottom - clipped_top, 0.0);
    }
    return result;
}

fn animation_corners(corners: Corners, animation: VisualAnimation) -> Corners {
    var result = corners;
    if (animation.scales_geometry != 0u) {
        result.top_left *= animation.scale;
        result.top_right *= animation.scale;
        result.bottom_right *= animation.scale;
        result.bottom_left *= animation.scale;
    }
    return result;
}

fn animation_edges(edges: Edges, animation: VisualAnimation) -> Edges {
    var result = edges;
    if (animation.scales_geometry != 0u) {
        result.top *= animation.scale;
        result.right *= animation.scale;
        result.bottom *= animation.scale;
        result.left *= animation.scale;
    }
    return result;
}
