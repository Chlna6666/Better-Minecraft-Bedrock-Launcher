// Text rasterization correction helpers.
fn color_brightness(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.30, 0.59, 0.11));
}

fn light_on_dark_contrast(enhancedContrast: f32, color: vec3<f32>) -> f32 {
    let brightness = color_brightness(color);
    let multiplier = saturate(4.0 * (0.75 - brightness));
    return enhancedContrast * multiplier;
}

fn enhance_contrast(alpha: f32, k: f32) -> f32 {
    let safe_alpha = saturate(alpha);
    let safe_k = max(k, 0.0);
    return safe_alpha * (safe_k + 1.0) / max(safe_alpha * safe_k + 1.0, SHADER_EPSILON);
}

fn apply_alpha_correction(a: f32, b: f32, g: vec4<f32>) -> f32 {
    let brightness_adjustment = g.x * b + g.y;
    let correction = brightness_adjustment * a + (g.z * b + g.w);
    return a + a * (1.0 - a) * correction;
}

fn apply_contrast_and_gamma_correction(sample: f32, color: vec3<f32>, enhanced_contrast_factor: f32, gamma_ratios: vec4<f32>) -> f32 {
    let enhanced_contrast = light_on_dark_contrast(enhanced_contrast_factor, color);
    let brightness = color_brightness(color);

    let contrasted = enhance_contrast(sample, enhanced_contrast);
    return apply_alpha_correction(contrasted, brightness, gamma_ratios);
}

struct TextRasterParams {
    gamma_ratios: vec4<f32>,
    grayscale_enhanced_contrast: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
}

@group(0) @binding(1) var<uniform> text_raster_params: TextRasterParams;
