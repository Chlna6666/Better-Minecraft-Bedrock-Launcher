fn fresnel_schlick(cosine: f32, f0: vec3<f32>) -> vec3<f32> {
    // Schlick is a fifth-degree polynomial. Explicit multiplies avoid
    // general-purpose pow/log/exp instructions on each HDR, voxel and MRT
    // light evaluation, while preserving the same reflectance curve.
    let f = 1.0 - clamp(cosine, 0.0, 1.0);
    let f2 = f * f;
    let f5 = f2 * f2 * f;
    return f0 + (vec3<f32>(1.0) - f0) * f5;
}

// Rough dielectrics lose sharp environment detail; rough metals retain a broad sky response.
fn environment_reflectance(cosine:f32,roughness:f32,metallic:f32,f0:vec3<f32>)->vec3<f32>{
    let smoothness=1.0-clamp(roughness,0.0,1.0);
    return fresnel_schlick(cosine,f0)*mix(smoothness*smoothness,1.0,metallic);
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


// Diffuse-only light sources intentionally contribute no direct specular lobe.
fn local_brdf(base:vec3<f32>,metallic:f32,roughness:f32,n:vec3<f32>,v:vec3<f32>,l:vec3<f32>,radiance:vec3<f32>,diffuse_only:bool,source_radius:f32,distance:f32)->vec3<f32>{
    if(diffuse_only){return base*(1.0-metallic)*max(dot(n,l),0.0)*radiance/3.14159265359;}
    var spread=roughness;
    if(source_radius>0.0){spread=clamp(sqrt(roughness*roughness+source_radius*source_radius/max(distance*distance,0.001)),0.045,1.0);}
    return cook_torrance_lighting(base,metallic,spread,n,v,l,radiance);
}
