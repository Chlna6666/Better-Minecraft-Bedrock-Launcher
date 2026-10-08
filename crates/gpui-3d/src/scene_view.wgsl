struct PackedVertex {
    position: vec3<f32>,
    barycentric_u: f32,
    normal: vec3<f32>,
    barycentric_v: f32,
    uv: vec2<f32>,
    barycentric_w: f32,
    edge_mask: f32,
    color: vec4<f32>,
};

/// Camera and viewport values that change when the camera or the extension bounds move.
///
/// Keeping these out of `DrawParams` lets the renderer upload a few hundred bytes for a camera
/// change instead of re-encoding every draw, instance, and light record.
struct FrameParams {
    view_projection: mat4x4<f32>,
    camera_position: vec3<f32>,
    element_bounds_origin: vec2<f32>,
    element_bounds_size: vec2<f32>,
    render_target_size: vec2<f32>,
    blend_edge_feather_px: f32,
    _padding: f32,
};

struct DrawParams {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    metallic: f32,
    roughness: f32,
    alpha_cutoff: f32,
    normal_mapping_enabled: u32,
    occlusion_strength: f32,
    alpha_mode: u32,
    light_count: u32,
    shading_model: u32,
    texture_flags: u32,
};

struct Instance {
    model: mat4x4<f32>,
    normal_matrix: mat4x4<f32>,
    pixel_offset: vec2<f32>,
    depth_bias: f32,
};

struct Light {
    kind: u32,
    position: vec3<f32>,
    range: f32,
    direction: vec3<f32>,
    color: vec3<f32>,
    intensity: f32,
    outer_cone_cosine: f32,
    inner_cone_cosine: f32,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) vertex_color: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) barycentrics: vec3<f32>,
    @location(5) @interpolate(flat) edge_mask: u32,
    @location(6) world_tangent: vec4<f32>,
};

const ALPHA_MODE_OPAQUE: u32 = 0u;
const ALPHA_MODE_MASK: u32 = 1u;
const ALPHA_MODE_BLEND: u32 = 2u;
const SHADING_MODEL_METALLIC_ROUGHNESS: u32 = 0u;
const SHADING_MODEL_UNLIT: u32 = 1u;
const TEXTURE_FLAG_ALBEDO: u32 = 1u;
const TEXTURE_FLAG_OCCLUSION: u32 = 2u;
/// Draws whose mesh UVs address a texture region; meshes baked to vertex color leave this clear.
const TEXTURE_FLAG_UV: u32 = 4u;
const LIGHT_KIND_AMBIENT: u32 = 0u;
const LIGHT_KIND_DIRECTIONAL: u32 = 1u;
const LIGHT_KIND_POINT: u32 = 2u;
const LIGHT_KIND_SPOT: u32 = 3u;

@group(0) @binding(0) var<storage, read> vertices: array<PackedVertex>;
@group(0) @binding(1) var<storage, read> draws: array<DrawParams>;
@group(0) @binding(2) var<storage, read> lights: array<Light>;
@group(0) @binding(3) var albedo_texture: texture_2d<f32>;
@group(0) @binding(4) var material_sampler: sampler;
@group(0) @binding(5) var normal_map_texture: texture_2d<f32>;
@group(0) @binding(6) var occlusion_texture: texture_2d<f32>;
@group(0) @binding(7) var<storage, read> tangents: array<vec4<f32>>;
@group(0) @binding(8) var<storage, read> instances: array<Instance>;
@group(0) @binding(9) var<uniform> frame: FrameParams;

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let vertex = vertices[vertex_index];
    let draw_params = draws[0u];
    let instance = instances[instance_index];
    let world_position_homogeneous = instance.model * vec4<f32>(vertex.position, 1.0);
    let clip_position = frame.view_projection * world_position_homogeneous;
    let safe_clip_w = select(
        min(clip_position.w, -0.0001),
        max(clip_position.w, 0.0001),
        clip_position.w >= 0.0,
    );
    let ndc = clip_position.xyz / safe_clip_w;
    let screen_position_pixels = frame.element_bounds_origin
        + (ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5)) * frame.element_bounds_size
        + instance.pixel_offset;
    let normalized_target_position = screen_position_pixels
        / max(frame.render_target_size, vec2<f32>(1.0));

    var output: VertexOutput;
    output.position = vec4<f32>(
        (normalized_target_position * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0))
            * clip_position.w,
        (ndc.z - instance.depth_bias) * clip_position.w,
        clip_position.w,
    );
    output.world_position = world_position_homogeneous.xyz;
    let world_normal = normalize(
        (instance.normal_matrix * vec4<f32>(vertex.normal, 0.0)).xyz,
    );
    output.world_normal = world_normal;
    output.world_tangent = vec4<f32>(1.0, 0.0, 0.0, 1.0);
    if (draw_params.normal_mapping_enabled != 0u) {
        let vertex_tangent = tangents[vertex_index];
        let world_tangent = (instance.model * vec4<f32>(vertex_tangent.xyz, 0.0)).xyz;
        let world_bitangent = (instance.model * vec4<f32>(
            cross(vertex.normal, vertex_tangent.xyz) * vertex_tangent.w,
            0.0,
        )).xyz;
        let orthogonal_world_tangent =
            normalize(world_tangent - world_normal * dot(world_normal, world_tangent));
        let tangent_handedness = select(
            -1.0,
            1.0,
            dot(cross(world_normal, orthogonal_world_tangent), world_bitangent) >= 0.0,
        );
        output.world_tangent = vec4<f32>(orthogonal_world_tangent, tangent_handedness);
    }
    output.vertex_color = vertex.color;
    output.uv = vertex.uv;
    output.barycentrics = vec3<f32>(
        vertex.barycentric_u,
        vertex.barycentric_v,
        vertex.barycentric_w,
    );
    output.edge_mask = u32(round(vertex.edge_mask));
    return output;
}

const PI: f32 = 3.14159265359;

fn fresnel_schlick(cosine: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - clamp(cosine, 0.0, 1.0), 5.0);
}

fn ggx_normal_distribution(normal_dot_half: f32, roughness: f32) -> f32 {
    let roughness_alpha = roughness * roughness;
    let roughness_alpha_squared = roughness_alpha * roughness_alpha;
    let denominator = normal_dot_half * normal_dot_half * (roughness_alpha_squared - 1.0) + 1.0;
    return roughness_alpha_squared / max(PI * denominator * denominator, 0.0001);
}

fn schlick_ggx_geometry(normal_dot_direction: f32, roughness: f32) -> f32 {
    let geometry_k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
    return normal_dot_direction
        / max(normal_dot_direction * (1.0 - geometry_k) + geometry_k, 0.0001);
}

fn cook_torrance_lighting(
    base_color: vec3<f32>,
    metallic: f32,
    roughness: f32,
    surface_normal: vec3<f32>,
    view_direction: vec3<f32>,
    light_direction: vec3<f32>,
    light_radiance: vec3<f32>,
) -> vec3<f32> {
    let normal_dot_light = max(dot(surface_normal, light_direction), 0.0);
    let normal_dot_view = max(dot(surface_normal, view_direction), 0.0);
    if (normal_dot_light <= 0.0 || normal_dot_view <= 0.0) {
        return vec3<f32>(0.0);
    }
    let half_vector = normalize(view_direction + light_direction);
    let normal_dot_half = max(dot(surface_normal, half_vector), 0.0);
    let view_dot_half = max(dot(view_direction, half_vector), 0.0);
    let base_reflectance = mix(vec3<f32>(0.04), base_color, metallic);
    let fresnel = fresnel_schlick(view_dot_half, base_reflectance);
    let normal_distribution = ggx_normal_distribution(normal_dot_half, roughness);
    let geometric_attenuation = schlick_ggx_geometry(normal_dot_view, roughness)
        * schlick_ggx_geometry(normal_dot_light, roughness);
    let specular = normal_distribution * geometric_attenuation * fresnel
        / max(4.0 * normal_dot_view * normal_dot_light, 0.0001);
    let diffuse = (vec3<f32>(1.0) - fresnel) * (1.0 - metallic) * base_color / PI;
    return (diffuse + specular) * light_radiance * normal_dot_light;
}

fn apply_normal_map(input: VertexOutput, surface_normal: vec3<f32>) -> vec3<f32> {
    let projected_world_tangent = input.world_tangent.xyz
        - surface_normal * dot(surface_normal, input.world_tangent.xyz);
    if (length(projected_world_tangent) <= 0.00001) {
        return surface_normal;
    }
    let tangent = normalize(projected_world_tangent);
    let bitangent = normalize(cross(surface_normal, tangent)) * input.world_tangent.w;
    let tangent_space_normal = textureSample(normal_map_texture, material_sampler, input.uv).xyz
        * 2.0 - vec3<f32>(1.0);
    return normalize(
        tangent * tangent_space_normal.x
            + bitangent * tangent_space_normal.y
            + surface_normal * tangent_space_normal.z,
    );
}

fn element_bounds_coverage(
    screen_position: vec2<f32>,
    element_bounds_origin: vec2<f32>,
    element_bounds_size: vec2<f32>,
    edge_feather_px: f32,
) -> f32 {
    if (edge_feather_px <= 0.0) {
        return 1.0;
    }
    let edge_distance_pixels = min(
        min(
            screen_position.x - element_bounds_origin.x,
            element_bounds_origin.x + element_bounds_size.x - screen_position.x,
        ),
        min(
            screen_position.y - element_bounds_origin.y,
            element_bounds_origin.y + element_bounds_size.y - screen_position.y,
        ),
    );
    return clamp(edge_distance_pixels / edge_feather_px, 0.0, 1.0);
}

fn triangle_edge_coverage(barycentrics: vec3<f32>, edge_mask: u32) -> f32 {
    let edge_transition_width = max(
        fwidth(barycentrics) * 1.25,
        vec3<f32>(0.00001),
    );
    let edge_coverage = smoothstep(
        vec3<f32>(0.0),
        edge_transition_width,
        barycentrics,
    );
    var visible_edge_coverage = 1.0;
    if ((edge_mask & 1u) != 0u) {
        visible_edge_coverage = min(visible_edge_coverage, edge_coverage.x);
    }
    if ((edge_mask & 2u) != 0u) {
        visible_edge_coverage = min(visible_edge_coverage, edge_coverage.y);
    }
    if ((edge_mask & 4u) != 0u) {
        visible_edge_coverage = min(visible_edge_coverage, edge_coverage.z);
    }
    return visible_edge_coverage;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let draw_params = draws[0u];
    var base_color = draw_params.base_color * input.vertex_color;
    if ((draw_params.texture_flags & TEXTURE_FLAG_ALBEDO) != 0u
        && (draw_params.texture_flags & TEXTURE_FLAG_UV) != 0u) {
        base_color *= textureSample(albedo_texture, material_sampler, input.uv);
    } else if ((draw_params.texture_flags & TEXTURE_FLAG_ALBEDO) != 0u) {
        // Baked vertex-color meshes still bind the texture table, but their zero UVs would sample
        // one arbitrary texel; keep them on their authored colors instead.
        base_color *= textureSample(albedo_texture, material_sampler, vec2<f32>(0.5, 0.5));
    }
    let alpha = clamp(base_color.a, 0.0, 1.0);
    let alpha_mode = draw_params.alpha_mode;
    var edge_coverage = 1.0;
    if (alpha_mode == ALPHA_MODE_BLEND) {
        edge_coverage = element_bounds_coverage(
            input.position.xy,
            frame.element_bounds_origin,
            frame.element_bounds_size,
            frame.blend_edge_feather_px,
        ) * triangle_edge_coverage(
            input.barycentrics,
            input.edge_mask,
        );
    }
    if (alpha_mode == ALPHA_MODE_MASK && alpha < draw_params.alpha_cutoff) {
        discard;
    }

    if (draw_params.shading_model == SHADING_MODEL_UNLIT) {
        let unlit_color = base_color.rgb
            + draw_params.emissive.rgb * draw_params.emissive.a;
        if (alpha_mode == ALPHA_MODE_BLEND) {
            let output_alpha = alpha * edge_coverage;
            return vec4<f32>(unlit_color * output_alpha, output_alpha);
        }
        return vec4<f32>(unlit_color, 1.0);
    }

    var surface_normal = normalize(input.world_normal);
    if (draw_params.normal_mapping_enabled != 0u) {
        surface_normal = apply_normal_map(input, surface_normal);
    }
    let view_direction = normalize(frame.camera_position.xyz - input.world_position);
    let metallic = clamp(draw_params.metallic, 0.0, 1.0);
    let roughness = clamp(draw_params.roughness, 0.045, 1.0);
    var radiance = draw_params.emissive.rgb * draw_params.emissive.a;
    var ambient_radiance = vec3<f32>(0.03);
    for (var light_index = 0u; light_index < draw_params.light_count; light_index += 1u) {
        let light = lights[light_index];
        if (light.kind == LIGHT_KIND_AMBIENT) {
            ambient_radiance += light.color * light.intensity;
        } else if (light.kind == LIGHT_KIND_DIRECTIONAL) {
            let light_direction = normalize(-light.direction);
            radiance += cook_torrance_lighting(
                base_color.rgb,
                metallic,
                roughness,
                surface_normal,
                view_direction,
                light_direction,
                light.color * light.intensity,
            );
        } else if (light.kind == LIGHT_KIND_POINT) {
            let surface_to_light = light.position - input.world_position;
            let light_distance = length(surface_to_light);
            let light_range = max(light.range, 0.0001);
            let attenuation = pow(max(1.0 - light_distance / light_range, 0.0), 2.0)
                / max(light_distance * light_distance, 0.01);
            radiance += cook_torrance_lighting(
                base_color.rgb,
                metallic,
                roughness,
                surface_normal,
                view_direction,
                normalize(surface_to_light),
                light.color * light.intensity * attenuation,
            );
        } else if (light.kind == LIGHT_KIND_SPOT) {
            let surface_to_light = light.position - input.world_position;
            let light_distance = length(surface_to_light);
            let light_range = max(light.range, 0.0001);
            let attenuation = pow(max(1.0 - light_distance / light_range, 0.0), 2.0)
                / max(light_distance * light_distance, 0.01);
            let light_to_surface = -normalize(surface_to_light);
            let cone_attenuation = smoothstep(
                light.outer_cone_cosine,
                light.inner_cone_cosine,
                dot(normalize(light.direction), light_to_surface),
            );
            radiance += cook_torrance_lighting(
                base_color.rgb,
                metallic,
                roughness,
                surface_normal,
                view_direction,
                normalize(surface_to_light),
                light.color * light.intensity * attenuation * cone_attenuation,
            );
        }
    }
    var occlusion_factor = 1.0;
    if ((draw_params.texture_flags & TEXTURE_FLAG_OCCLUSION) != 0u) {
        let occlusion = textureSample(occlusion_texture, material_sampler, input.uv).r;
        occlusion_factor = mix(1.0, occlusion, draw_params.occlusion_strength);
    }
    radiance += base_color.rgb * ambient_radiance * (1.0 - metallic) * occlusion_factor;
    let output_color = pow(
        max(radiance / (radiance + vec3<f32>(1.0)), vec3<f32>(0.0)),
        vec3<f32>(1.0 / 2.2),
    );

    if (alpha_mode == ALPHA_MODE_BLEND) {
        let output_alpha = alpha * edge_coverage;
        return vec4<f32>(output_color * output_alpha, output_alpha);
    }
    return vec4<f32>(output_color, 1.0);
}
