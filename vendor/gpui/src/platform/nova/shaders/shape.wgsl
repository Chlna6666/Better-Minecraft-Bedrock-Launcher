// Shared rounded-rectangle SDF helpers.
// Selects corner radius based on quadrant.
fn pick_corner_radius(center_to_point: vec2<f32>, radii: Corners) -> f32 {
    let top_side = center_to_point.y < 0.0;
    let left_radius = select(radii.bottom_left, radii.top_left, top_side);
    let right_radius = select(radii.bottom_right, radii.top_right, top_side);
    return select(right_radius, left_radius, center_to_point.x < 0.0);
}

fn pick_corner_radius_from_packed(center_to_point: vec2<f32>, packed_radii: vec4<f32>) -> f32 {
    let top_side = center_to_point.y < 0.0;
    let left_radius = select(packed_radii.w, packed_radii.x, top_side);
    let right_radius = select(packed_radii.z, packed_radii.y, top_side);
    return select(right_radius, left_radius, center_to_point.x < 0.0);
}

// Signed distance of the point to the quad's border - positive outside the
// border, and negative inside.
//
// See comments on similar code using `quad_sdf_impl` in `fs_quad` for
// explanation.
fn quad_sdf(point: vec2<f32>, bounds: Bounds, corner_radii: Corners) -> f32 {
    let half_size = bounds.size / 2.0;
    let center = bounds.origin + half_size;
    let center_to_point = point - center;
    let corner_radius = pick_corner_radius(center_to_point, corner_radii);
    let corner_to_point = abs(center_to_point) - half_size;
    let corner_center_to_point = corner_to_point + corner_radius;
    return quad_sdf_impl(corner_center_to_point, corner_radius);
}

fn quad_sdf_from_packed(point: vec2<f32>, packed_bounds: vec4<f32>, packed_corner_radii: vec4<f32>) -> f32 {
    let half_size = packed_bounds.zw / 2.0;
    let center = packed_bounds.xy + half_size;
    let center_to_point = point - center;
    let corner_radius = pick_corner_radius_from_packed(center_to_point, packed_corner_radii);
    let corner_to_point = abs(center_to_point) - half_size;
    let corner_center_to_point = corner_to_point + corner_radius;
    return quad_sdf_impl(corner_center_to_point, corner_radius);
}

fn quad_sdf_impl(corner_center_to_point: vec2<f32>, corner_radius: f32) -> f32 {
    if (corner_radius == 0.0) {
        // Fast path for unrounded corners.
        return max(corner_center_to_point.x, corner_center_to_point.y);
    } else {
        // Signed distance of the point from a quad that is inset by corner_radius.
        // It is negative inside this quad, and positive outside.
        let signed_distance_to_inset_quad =
            // 0 inside the inset quad, and positive outside.
            length(max(vec2<f32>(0.0), corner_center_to_point)) +
            // 0 outside the inset quad, and negative inside.
            min(0.0, max(corner_center_to_point.x, corner_center_to_point.y));

        return signed_distance_to_inset_quad - corner_radius;
    }
}
