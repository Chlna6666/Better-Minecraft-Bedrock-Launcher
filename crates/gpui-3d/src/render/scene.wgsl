// Linear-space HDR PBR scene pass for the offscreen gpui-3d renderer.
//
// Lighting is accumulated in linear space, shadowed by the packed directional shadow map, and
// written to an RGBA16Float target without clamping. Exposure, tone mapping, bloom, FXAA, and
// gamma are resolved by the post pass so bright lights keep headroom until grading.
//
// Bind group 0:
//   0 packed vertices, 1 draw params, 2 lights, 3 albedo, 4 material sampler,
//   5 normal map, 6 occlusion map, 7 tangents, 8 instances, 9 frame uniform,
//   10 shadow map.

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
    offset: vec4<f32>,
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

struct Frame {
    view_projection: mat4x4<f32>,
    light_view_projection: mat4x4<f32>,
    camera_position: vec4<f32>,
    light_direction: vec4<f32>,
    light_color: vec4<f32>,
    ambient: vec4<f32>,
    // exposure, tonemap mode, debug view, shadow strength
    post: vec4<f32>,
    // shadow texel x, shadow texel y, depth bias, shadow enabled
    shadow: vec4<f32>,
    // target width, target height, normal bias, object-id seed
    misc: vec4<f32>,
    // bloom threshold, bloom strength, shadow softness, FXAA enabled
    effects: vec4<f32>,
    // vignette, film grain, dither, debug-view bypass
    effects2: vec4<f32>,
    // bloom radius, bloom threshold duplicate, unused, unused
    effects3: vec4<f32>,
};

@group(0) @binding(0) var<storage, read> vertices: array<PackedVertex>;
@group(0) @binding(1) var<storage, read> draws: array<DrawParams>;
@group(0) @binding(2) var<storage, read> lights: array<Light>;
@group(0) @binding(3) var albedo_texture: texture_2d<f32>;
@group(0) @binding(4) var material_sampler: sampler;
@group(0) @binding(5) var normal_map_texture: texture_2d<f32>;
@group(0) @binding(6) var occlusion_texture: texture_2d<f32>;
@group(0) @binding(7) var<storage, read> tangents: array<vec4<f32>>;
@group(0) @binding(8) var<storage, read> instances: array<Instance>;
@group(0) @binding(9) var<uniform> frame: Frame;
@group(0) @binding(10) var shadow_map_texture: texture_2d<f32>;
@group(0) @binding(11) var metallic_roughness_texture: texture_2d<f32>;

const ALPHA_MODE_OPAQUE: u32 = 0u;
const ALPHA_MODE_MASK: u32 = 1u;
const ALPHA_MODE_BLEND: u32 = 2u;
const SHADING_MODEL_METALLIC_ROUGHNESS: u32 = 0u;
const SHADING_MODEL_UNLIT: u32 = 1u;
const TEXTURE_FLAG_ALBEDO: u32 = 1u;
const TEXTURE_FLAG_OCCLUSION: u32 = 2u;
const TEXTURE_FLAG_UV: u32 = 4u;
const TEXTURE_FLAG_METALLIC_ROUGHNESS: u32 = 8u;
const LIGHT_KIND_AMBIENT: u32 = 0u;
const LIGHT_KIND_DIRECTIONAL: u32 = 1u;
const LIGHT_KIND_POINT: u32 = 2u;
const LIGHT_KIND_SPOT: u32 = 3u;
const TONEMAP_NONE: u32 = 0u;
const TONEMAP_REINHARD: u32 = 1u;
const TONEMAP_ACES: u32 = 2u;
const DEBUG_SHADED: u32 = 0u;
const DEBUG_ALBEDO: u32 = 1u;
const DEBUG_NORMAL: u32 = 2u;
const DEBUG_DEPTH: u32 = 3u;
const DEBUG_UV: u32 = 4u;
const DEBUG_SHADOW: u32 = 5u;
const DEBUG_OBJECT_ID: u32 = 6u;
const PI: f32 = 3.14159265359;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) vertex_color: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) world_tangent: vec4<f32>,
    @location(5) light_clip: vec4<f32>,
    @location(6) @interpolate(flat) instance_index: u32,
};

@vertex
fn vs_scene(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let vertex = vertices[vertex_index];
    let instance = instances[instance_index];
    let world_position = instance.model * vec4<f32>(vertex.position, 1.0);
    let clip_position = frame.view_projection * world_position;
    var output: VertexOutput;
    // Nudge the depth by the per-instance bias to keep coplanar preview geometry stable.
    var clip = clip_position;
    clip.z = clip_position.z - instance.offset.z * clip_position.w;
    output.position = clip;
    output.world_position = world_position.xyz;
    let world_normal = normalize(
        (instance.normal_matrix * vec4<f32>(vertex.normal, 0.0)).xyz,
    );
    output.world_normal = world_normal;
    output.world_tangent = vec4<f32>(1.0, 0.0, 0.0, 1.0);
    if (draws[0u].normal_mapping_enabled != 0u) {
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
    output.light_clip = frame.light_view_projection * world_position;
    output.instance_index = instance_index;
    return output;
}

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

fn unpack_depth(color: vec4<f32>) -> f32 {
    let value = color.r * 255.0 * 65536.0 + color.g * 255.0 * 256.0 + color.b * 255.0;
    return floor(value + 0.5) / 16777215.0;
}

fn shadow_visibility(light_clip: vec4<f32>, surface_normal: vec3<f32>) -> f32 {
    if (frame.shadow.w < 0.5) {
        return 1.0;
    }
    let ndc = light_clip.xyz / max(light_clip.w, 0.0001);
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (uv.x <= 0.0 || uv.x >= 1.0 || uv.y <= 0.0 || uv.y >= 1.0
        || ndc.z <= 0.0 || ndc.z >= 1.0) {
        return 1.0;
    }
    let light_direction = normalize(-frame.light_direction.xyz);
    let slope = 1.0 - clamp(dot(surface_normal, light_direction), 0.0, 1.0);
    let bias = frame.shadow.z + frame.misc.z * slope;
    // Keep the 3x3 comparison grid but scale its radius with the quality softness so the same
    // packed depth target can be crisp or soft. Offsets stay within one texel group for stability.
    let softness = clamp(frame.effects.z, 0.2, 4.0);
    let step = frame.shadow.xy * softness;
    let sum = textureSample(shadow_map_texture, material_sampler, uv + vec2<f32>(-1.0, -1.0) * step)
        + textureSample(shadow_map_texture, material_sampler, uv + vec2<f32>(0.0, -1.0) * step)
        + textureSample(shadow_map_texture, material_sampler, uv + vec2<f32>(1.0, -1.0) * step)
        + textureSample(shadow_map_texture, material_sampler, uv + vec2<f32>(-1.0, 0.0) * step)
        + textureSample(shadow_map_texture, material_sampler, uv)
        + textureSample(shadow_map_texture, material_sampler, uv + vec2<f32>(1.0, 0.0) * step)
        + textureSample(shadow_map_texture, material_sampler, uv + vec2<f32>(-1.0, 1.0) * step)
        + textureSample(shadow_map_texture, material_sampler, uv + vec2<f32>(0.0, 1.0) * step)
        + textureSample(shadow_map_texture, material_sampler, uv + vec2<f32>(1.0, 1.0) * step);
    let stored = unpack_depth(sum / 9.0);
    let visible = select(0.0, 1.0, ndc.z - bias <= stored + 0.00002);
    return mix(1.0, visible, frame.post.w);
}

fn tonemap_aces(color: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((color * (a * color + vec3<f32>(b))) / (color * (c * color + vec3<f32>(d)) + vec3<f32>(e)), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn tonemap(color: vec3<f32>, mode: u32) -> vec3<f32> {
    if (mode == TONEMAP_REINHARD) {
        return color / (color + vec3<f32>(1.0));
    }
    if (mode == TONEMAP_ACES) {
        return tonemap_aces(color);
    }
    return clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn encode_object_id(instance_index: u32) -> vec3<f32> {
    // One-based 24-bit instance id in RGB so the CPU can decode the submitted draw exactly.
    // Zero remains the cleared background.
    let id = instance_index + 1u;
    let red = f32(id & 255u) / 255.0;
    let green = f32((id >> 8u) & 255u) / 255.0;
    let blue = f32((id >> 16u) & 255u) / 255.0;
    return vec3<f32>(red, green, blue);
}

@fragment
fn fs_scene(input: VertexOutput) -> @location(0) vec4<f32> {
    let draw_params = draws[0u];
    var base_color = draw_params.base_color * input.vertex_color;
    if ((draw_params.texture_flags & TEXTURE_FLAG_ALBEDO) != 0u) {
        if ((draw_params.texture_flags & TEXTURE_FLAG_UV) != 0u) {
            base_color *= textureSample(albedo_texture, material_sampler, input.uv);
        } else {
            base_color *= textureSample(albedo_texture, material_sampler, vec2<f32>(0.5, 0.5));
        }
    }
    let alpha = clamp(base_color.a, 0.0, 1.0);
    let alpha_mode = draw_params.alpha_mode;
    if (alpha_mode == ALPHA_MODE_MASK && alpha < draw_params.alpha_cutoff) {
        discard;
    }

    var surface_normal = normalize(input.world_normal);
    if (draw_params.normal_mapping_enabled != 0u) {
        surface_normal = apply_normal_map(input, surface_normal);
    }
    let view_direction = normalize(frame.camera_position.xyz - input.world_position);
    var metallic = clamp(draw_params.metallic, 0.0, 1.0);
    var roughness = clamp(draw_params.roughness, 0.045, 1.0);
    if ((draw_params.texture_flags & TEXTURE_FLAG_METALLIC_ROUGHNESS) != 0u) {
        let metallic_roughness_sample =
            textureSample(metallic_roughness_texture, material_sampler, input.uv);
        metallic = clamp(metallic * metallic_roughness_sample.b, 0.0, 1.0);
        roughness = clamp(roughness * metallic_roughness_sample.g, 0.045, 1.0);
    }
    let shadow_visibility_value = shadow_visibility(input.light_clip, surface_normal);

    var radiance = draw_params.emissive.rgb * draw_params.emissive.a;
    var ambient_radiance = vec3<f32>(0.03);
    if (draw_params.alpha_mode == ALPHA_MODE_BLEND) {
        // Blended surfaces keep their authored shading but must not be darkened by shadows
        // meant for the opaque depth writers behind them.
    }
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
                light.color * light.intensity * shadow_visibility_value,
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
    // Analytic hemisphere environment: there is no environment map or irradiance probe yet, so
    // direct lights alone leave metals black. A sky/ground gradient gives ambient diffuse fill,
    // and a Fresnel-weighted reflection term keeps metal and low-roughness surfaces readable.
    let sky_color = vec3<f32>(0.4, 0.49, 0.68);
    let ground_color = vec3<f32>(0.15, 0.13, 0.11);
    let hemisphere = mix(ground_color, sky_color, clamp(surface_normal.y * 0.5 + 0.5, 0.0, 1.0));
    let environment_radiance = ambient_radiance + hemisphere * 0.8;
    let ambient_fresnel = fresnel_schlick(
        max(dot(surface_normal, view_direction), 0.0),
        mix(vec3<f32>(0.04), base_color.rgb, metallic),
    );
    let reflection_direction = reflect(-view_direction, surface_normal);
    let reflected_environment = mix(
        ground_color,
        sky_color,
        clamp(reflection_direction.y * 0.5 + 0.5, 0.0, 1.0),
    ) * 0.95;
    let ambient_diffuse = environment_radiance * base_color.rgb * (1.0 - metallic)
        * occlusion_factor;
    let ambient_specular = reflected_environment * ambient_fresnel
        * (1.0 - roughness * 0.75) * 1.3 * occlusion_factor;
    radiance += ambient_diffuse + ambient_specular;

    let debug_view = u32(round(frame.post.z));
    if (debug_view == DEBUG_ALBEDO) {
        return vec4<f32>(pow(clamp(base_color.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2)), 1.0);
    }
    if (debug_view == DEBUG_NORMAL) {
        return vec4<f32>(surface_normal * 0.5 + vec3<f32>(0.5), 1.0);
    }
    if (debug_view == DEBUG_DEPTH) {
        let depth = clamp(input.position.z, 0.0, 1.0);
        return vec4<f32>(vec3<f32>(depth), 1.0);
    }
    if (debug_view == DEBUG_UV) {
        return vec4<f32>(input.uv, 0.0, 1.0);
    }
    if (debug_view == DEBUG_SHADOW) {
        return vec4<f32>(vec3<f32>(shadow_visibility_value), 1.0);
    }
    if (debug_view == DEBUG_OBJECT_ID) {
        return vec4<f32>(encode_object_id(input.instance_index), 1.0);
    }

    if (draw_params.shading_model == SHADING_MODEL_UNLIT) {
        let unlit_color = clamp(base_color.rgb, vec3<f32>(0.0), vec3<f32>(1.0))
            + draw_params.emissive.rgb * draw_params.emissive.a;
        return vec4<f32>(unlit_color, 1.0);
    }

    // HDR scene target: keep linear radiance. Exposure, tone mapping, bloom, FXAA, and gamma are
    // resolved by the post pass so highlights still have headroom when they reach grading.
    if (alpha_mode == ALPHA_MODE_BLEND) {
        return vec4<f32>(radiance * alpha, alpha);
    }
    return vec4<f32>(max(radiance, vec3<f32>(0.0)), 1.0);
}