// Text rasterization correction helpers.
fn color_brightness(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.30, 0.59, 0.11));
}

fn color_intensity(color: vec3<f32>) -> f32 {
    // DirectWrite uses this channel weighting for grayscale gamma correction. It is distinct from
    // perceived brightness above, which is used only to scale light-on-dark enhanced contrast.
    return dot(color, vec3<f32>(0.25, 0.50, 0.25));
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
    let intensity = color_intensity(color);
    let contrasted = enhance_contrast(sample, enhanced_contrast);
    return apply_alpha_correction(contrasted, intensity, gamma_ratios);
}

fn apply_contrast_and_gamma_correction3(sample: vec3<f32>, color: vec3<f32>, enhanced_contrast_factor: f32, gamma_ratios: vec4<f32>) -> vec3<f32> {
    let enhanced_contrast = light_on_dark_contrast(enhanced_contrast_factor, color);
    let contrasted = vec3<f32>(
        enhance_contrast(sample.r, enhanced_contrast),
        enhance_contrast(sample.g, enhanced_contrast),
        enhance_contrast(sample.b, enhanced_contrast),
    );
    // DirectWrite's ClearType gamma correction is channel-sensitive: the red, green, and blue
    // glyph coverages are corrected against the corresponding foreground-color channel rather
    // than one shared scalar intensity. Match the DirectWrite/Microsoft Terminal model here.
    return vec3<f32>(
        apply_alpha_correction(contrasted.r, color.r, gamma_ratios),
        apply_alpha_correction(contrasted.g, color.g, gamma_ratios),
        apply_alpha_correction(contrasted.b, color.b, gamma_ratios),
    );
}

struct TextRasterParams {
    gamma_ratios: vec4<f32>,
    grayscale_enhanced_contrast: f32,
    subpixel_enhanced_contrast: f32,
    subpixel_config: u32,
    pad0: u32,
}

@group(0) @binding(1) var<uniform> text_raster_params: TextRasterParams;
