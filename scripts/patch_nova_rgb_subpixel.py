from __future__ import annotations

import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8-sig")


def write(path: str, text: str) -> None:
    (ROOT / path).write_text(text, encoding="utf-8")


def replace_once(path: str, old: str, new: str) -> None:
    text = read(path)
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{path}: expected one replacement, found {count}: {old[:100]!r}")
    write(path, text.replace(old, new, 1))


def replace_all(path: str, old: str, new: str, expected: int) -> None:
    text = read(path)
    if new in text and old not in text:
        return
    count = text.count(old)
    if count != expected:
        raise RuntimeError(f"{path}: expected {expected} replacements, found {count}: {old[:100]!r}")
    write(path, text.replace(old, new))


def regex_once(path: str, pattern: str, replacement: str) -> None:
    text = read(path)
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        if replacement in text:
            return
        raise RuntimeError(f"{path}: regex replacement failed: {pattern[:100]!r}")
    write(path, updated)


TEXT_SHADER = r'''// Text rasterization correction helpers.
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

fn apply_contrast_and_gamma_correction3(sample: vec3<f32>, color: vec3<f32>, enhanced_contrast_factor: f32, gamma_ratios: vec4<f32>) -> vec3<f32> {
    let enhanced_contrast = light_on_dark_contrast(enhanced_contrast_factor, color);
    let brightness = color_brightness(color);
    let contrasted = vec3<f32>(
        enhance_contrast(sample.r, enhanced_contrast),
        enhance_contrast(sample.g, enhanced_contrast),
        enhance_contrast(sample.b, enhanced_contrast),
    );
    return vec3<f32>(
        apply_alpha_correction(contrasted.r, brightness, gamma_ratios),
        apply_alpha_correction(contrasted.g, brightness, gamma_ratios),
        apply_alpha_correction(contrasted.b, brightness, gamma_ratios),
    );
}

struct TextRasterParams {
    gamma_ratios: vec4<f32>,
    grayscale_enhanced_contrast: f32,
    subpixel_enhanced_contrast: f32,
    is_bgr: u32,
    pad0: u32,
}

@group(0) @binding(1) var<uniform> text_raster_params: TextRasterParams;
'''

MONO_SHADER = r'''// --- monochrome and RGB subpixel sprites --- //

struct MonochromeSprite {
    order: u32,
    pad: u32,
    bounds: Bounds,
    content_mask: ContentMask,
    color: Rgba,
    tile: AtlasTile,
    transformation: TransformationMatrix,
}
@group(0) @binding(8) var<storage, read> b_mono_sprites: array<MonochromeSprite>;

struct MonoSpriteVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) tile_position: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(3) clip_distances: vec4<f32>,
    @location(4) @interpolate(flat) content_mask_bounds: vec4<f32>,
    @location(5) @interpolate(flat) content_mask_radii: vec4<f32>,
}

struct SubpixelSpriteFragmentOutput {
    @location(0) @blend_src(0) foreground: vec4<f32>,
    @location(0) @blend_src(1) coverage: vec4<f32>,
}

fn mono_sprite_varying(vertex_id: u32, instance_id: u32) -> MonoSpriteVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let sprite = b_mono_sprites[instance_id];

    var out = MonoSpriteVarying();
    out.position = to_device_position_transformed(unit_vertex, sprite.bounds, sprite.transformation);
    out.tile_position = to_tile_position(unit_vertex, sprite.tile);
    out.color = rgba_to_vec4(sprite.color);
    out.clip_distances = distance_from_clip_rect_transformed(unit_vertex, sprite.bounds, sprite.content_mask.bounds, sprite.transformation);
    out.content_mask_bounds = vec4<f32>(sprite.content_mask.corner_bounds.origin, sprite.content_mask.corner_bounds.size);
    out.content_mask_radii = vec4<f32>(sprite.content_mask.corner_radii.top_left, sprite.content_mask.corner_radii.top_right, sprite.content_mask.corner_radii.bottom_right, sprite.content_mask.corner_radii.bottom_left);
    return out;
}

@vertex
fn vs_mono_sprite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> MonoSpriteVarying {
    return mono_sprite_varying(vertex_id, instance_id);
}

@vertex
fn vs_subpixel_sprite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> MonoSpriteVarying {
    return mono_sprite_varying(vertex_id, instance_id);
}

@fragment
fn fs_mono_sprite(input: MonoSpriteVarying) -> @location(0) vec4<f32> {
    let clip_coverage = content_mask_coverage_from_packed(input.position.xy, input.content_mask_bounds, input.content_mask_radii);
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }
    if (clip_coverage <= 0.0 || input.color.a <= 0.0) {
        return vec4<f32>(0.0);
    }

    let sample = textureSampleLevel(t_sprite, s_sprite, input.tile_position, 0.0).r;
    if (sample <= 0.0) {
        return vec4<f32>(0.0);
    }

    let alpha_corrected = apply_contrast_and_gamma_correction(
        sample,
        input.color.rgb,
        text_raster_params.grayscale_enhanced_contrast,
        text_raster_params.gamma_ratios
    );
    return blend_color(input.color, alpha_corrected * clip_coverage);
}

@fragment
fn fs_subpixel_sprite(input: MonoSpriteVarying) -> SubpixelSpriteFragmentOutput {
    let clip_coverage = content_mask_coverage_from_packed(input.position.xy, input.content_mask_bounds, input.content_mask_radii);
    if (any(input.clip_distances < vec4<f32>(0.0)) || clip_coverage <= 0.0 || input.color.a <= 0.0) {
        return SubpixelSpriteFragmentOutput(vec4<f32>(0.0), vec4<f32>(0.0));
    }

    var sample = textureSampleLevel(t_sprite, s_sprite, input.tile_position, 0.0).rgb;
    if (text_raster_params.is_bgr != 0u) {
        sample = sample.bgr;
    }
    let corrected = apply_contrast_and_gamma_correction3(
        sample,
        input.color.rgb,
        text_raster_params.subpixel_enhanced_contrast,
        text_raster_params.gamma_ratios
    );

    var out = SubpixelSpriteFragmentOutput();
    out.foreground = vec4<f32>(input.color.rgb, 1.0);
    out.coverage = vec4<f32>(input.color.a * corrected * clip_coverage, 1.0);
    return out;
}
'''

write("crates/gpui/src/platform/nova/shaders/text.wgsl", TEXT_SHADER)
write("crates/gpui/src/platform/nova/shaders/mono_sprite.wgsl", MONO_SHADER)

# Frame uniform: carry both contrast values and the display's RGB/BGR geometry.
replace_once(
    "crates/gpui/src/platform/nova/frame_upload/encode.rs",
    '''        write_f32_vec(\n            &mut self.text_raster_params,\n            rendering_parameters.grayscale_enhanced_contrast,\n        );\n        write_f32_vec(&mut self.text_raster_params, 0.0);\n        write_f32_vec(&mut self.text_raster_params, 0.0);\n        write_f32_vec(&mut self.text_raster_params, 0.0);''',
    '''        write_f32_vec(\n            &mut self.text_raster_params,\n            rendering_parameters.grayscale_enhanced_contrast,\n        );\n        write_f32_vec(\n            &mut self.text_raster_params,\n            rendering_parameters.subpixel_enhanced_contrast,\n        );\n        write_u32_vec(\n            &mut self.text_raster_params,\n            u32::from(rendering_parameters.is_bgr),\n        );\n        write_u32_vec(&mut self.text_raster_params, 0);''',
)

# Windows Nova DX12/Vulkan advertise true RGB coverage; DirectWrite still obeys the
# user's system ClearType switch before generating 3-channel coverage.
replace_once(
    "crates/gpui/src/render_pipeline/renderer_backend.rs",
    '''    pub const fn capabilities(self) -> RendererCapabilities {\n        match self {\n            Self::Auto\n            | Self::NovaVulkan\n            | Self::NovaDx12\n            | Self::NovaMetal\n            | Self::HeadlessTest => RendererCapabilities {\n                text_rasterization: TextRasterizationMode::Grayscale,\n            },\n        }\n    }''',
    '''    pub const fn capabilities(self) -> RendererCapabilities {\n        let text_rasterization = match self {\n            #[cfg(target_os = "windows")]\n            Self::NovaVulkan | Self::NovaDx12 => TextRasterizationMode::RgbSubpixel,\n            Self::Auto\n            | Self::NovaVulkan\n            | Self::NovaDx12\n            | Self::NovaMetal\n            | Self::HeadlessTest => TextRasterizationMode::Grayscale,\n        };\n        RendererCapabilities { text_rasterization }\n    }''',
)
regex_once(
    "crates/gpui/src/render_pipeline/renderer_backend.rs",
    r'''    #\[test\]\n    fn nova_backends_report_grayscale_text_rasterization\(\) \{.*?\n    \}\n\n    #\[test\]\n    fn gpu_submission_mode_defaults_to_deferred''',
    '''    #[test]\n    fn nova_backends_report_platform_text_rasterization() {\n        #[cfg(target_os = "windows")]\n        for backend in [RendererBackend::NovaVulkan, RendererBackend::NovaDx12] {\n            assert_eq!(\n                backend.capabilities().text_rasterization,\n                TextRasterizationMode::RgbSubpixel\n            );\n        }\n        #[cfg(not(target_os = "windows"))]\n        for backend in [RendererBackend::NovaVulkan, RendererBackend::NovaDx12] {\n            assert_eq!(\n                backend.capabilities().text_rasterization,\n                TextRasterizationMode::Grayscale\n            );\n        }\n        assert_eq!(\n            RendererBackend::NovaMetal.capabilities().text_rasterization,\n            TextRasterizationMode::Grayscale\n        );\n    }\n\n    #[test]\n    fn gpu_submission_mode_defaults_to_deferred''',
)

# Naga must validate the @blend_src attributes used by the RGB pipeline.
replace_once(
    "crates/nova-gfx/gfx-shader/src/gfx_shader.rs",
    "Validator::new(ValidationFlags::all(), Capabilities::empty())",
    "Validator::new(ValidationFlags::all(), Capabilities::DUAL_SOURCE_BLENDING)",
)

# Add a target-specific, explicit blend mode instead of overloading an existing mode.
replace_once(
    "crates/nova-gfx/gfx-core/src/gfx_core.rs",
    '''    /// Preserve color-over behavior while accumulating alpha.\n    AdditiveAlpha,\n}''',
    '''    /// Preserve color-over behavior while accumulating alpha.\n    AdditiveAlpha,\n    /// Windows RGB subpixel text using the second fragment output as per-channel coverage.\n    #[cfg(target_os = "windows")]\n    SubpixelDualSource,\n}''',
)

# Compile separate vertex/fragment entry points for the RGB pipeline while reusing the
# existing mono sprite storage buffer layout.
path = "crates/gpui/src/platform/nova/shader.rs"
replace_all(
    path,
    "    pub(super) mono_fragment: gfx_core::ShaderBinary,\n",
    '''    pub(super) mono_fragment: gfx_core::ShaderBinary,\n    #[cfg(target_os = "windows")]\n    pub(super) subpixel_vertex: gfx_core::ShaderBinary,\n    #[cfg(target_os = "windows")]\n    pub(super) subpixel_fragment: gfx_core::ShaderBinary,\n''',
    1,
)
replace_all(
    path,
    "    pub(super) mono_fragment: gfx_core::ShaderModuleId,\n",
    '''    pub(super) mono_fragment: gfx_core::ShaderModuleId,\n    #[cfg(target_os = "windows")]\n    pub(super) subpixel_vertex: gfx_core::ShaderModuleId,\n    #[cfg(target_os = "windows")]\n    pub(super) subpixel_fragment: gfx_core::ShaderModuleId,\n''',
    1,
)
replace_once(
    path,
    '''        mono_fragment: compile(\n            NOVA_MONO_SPRITE_SHADER_SOURCE,\n            ShaderStage::Fragment,\n            "fs_mono_sprite",\n        )\n        .context("compiling nova mono sprite fragment shader")?,''',
    '''        mono_fragment: compile(\n            NOVA_MONO_SPRITE_SHADER_SOURCE,\n            ShaderStage::Fragment,\n            "fs_mono_sprite",\n        )\n        .context("compiling nova mono sprite fragment shader")?,\n        #[cfg(target_os = "windows")]\n        subpixel_vertex: compile(\n            NOVA_MONO_SPRITE_SHADER_SOURCE,\n            ShaderStage::Vertex,\n            "vs_subpixel_sprite",\n        )\n        .context("compiling nova RGB subpixel sprite vertex shader")?,\n        #[cfg(target_os = "windows")]\n        subpixel_fragment: compile(\n            NOVA_MONO_SPRITE_SHADER_SOURCE,\n            ShaderStage::Fragment,\n            "fs_subpixel_sprite",\n        )\n        .context("compiling nova RGB subpixel sprite fragment shader")?,''',
)

# Shader modules.
path = "crates/gpui/src/platform/nova/resources/shaders.rs"
replace_once(
    path,
    "    pub(super) mono_fragment: ShaderModuleId,\n",
    '''    pub(super) mono_fragment: ShaderModuleId,\n    #[cfg(target_os = "windows")]\n    pub(super) subpixel_vertex: ShaderModuleId,\n    #[cfg(target_os = "windows")]\n    pub(super) subpixel_fragment: ShaderModuleId,\n''',
)
replace_once(
    path,
    '''    let mono_fragment = device\n        .create_shader_module(&ShaderModuleDescriptor {\n            label: Some(format!("{label} mono sprite fragment shader")),\n            binary: shader_binaries.mono_fragment,\n        })\n        .context("creating nova mono sprite fragment shader module")?;''',
    '''    let mono_fragment = device\n        .create_shader_module(&ShaderModuleDescriptor {\n            label: Some(format!("{label} mono sprite fragment shader")),\n            binary: shader_binaries.mono_fragment,\n        })\n        .context("creating nova mono sprite fragment shader module")?;\n    #[cfg(target_os = "windows")]\n    let subpixel_vertex = device\n        .create_shader_module(&ShaderModuleDescriptor {\n            label: Some(format!("{label} RGB subpixel sprite vertex shader")),\n            binary: shader_binaries.subpixel_vertex,\n        })\n        .context("creating nova RGB subpixel sprite vertex shader module")?;\n    #[cfg(target_os = "windows")]\n    let subpixel_fragment = device\n        .create_shader_module(&ShaderModuleDescriptor {\n            label: Some(format!("{label} RGB subpixel sprite fragment shader")),\n            binary: shader_binaries.subpixel_fragment,\n        })\n        .context("creating nova RGB subpixel sprite fragment shader module")?;''',
)
replace_once(
    path,
    '''        mono_vertex,\n        mono_fragment,\n        poly_vertex,''',
    '''        mono_vertex,\n        mono_fragment,\n        #[cfg(target_os = "windows")]\n        subpixel_vertex,\n        #[cfg(target_os = "windows")]\n        subpixel_fragment,\n        poly_vertex,''',
)

# Thread subpixel modules through pipeline creation.
path = "crates/gpui/src/platform/nova/resources/pipelines.rs"
replace_all(
    path,
    '''            mono_vertex: shaders.mono_vertex,\n            mono_fragment: shaders.mono_fragment,\n            poly_vertex:''',
    '''            mono_vertex: shaders.mono_vertex,\n            mono_fragment: shaders.mono_fragment,\n            #[cfg(target_os = "windows")]\n            subpixel_vertex: shaders.subpixel_vertex,\n            #[cfg(target_os = "windows")]\n            subpixel_fragment: shaders.subpixel_fragment,\n            poly_vertex:''',
    2,
)

# Dedicated dual-source text PSO.
path = "crates/gpui/src/platform/nova/pipeline.rs"
replace_once(
    path,
    "    pub(super) mono_sprites: RenderPipelineId,\n",
    '''    pub(super) mono_sprites: RenderPipelineId,\n    #[cfg(target_os = "windows")]\n    pub(super) subpixel_sprites: RenderPipelineId,\n''',
)
replace_once(
    path,
    '''        .with_context(|| {\n            format!(\n                "creating nova {} mono sprite render pipeline",\n                descriptor.suffix\n            )\n        })?;\n    let poly_sprites = device''',
    '''        .with_context(|| {\n            format!(\n                "creating nova {} mono sprite render pipeline",\n                descriptor.suffix\n            )\n        })?;\n    #[cfg(target_os = "windows")]\n    let subpixel_sprites = device\n        .create_render_pipeline(\n            &RenderPipelineDescriptor {\n                label: Some(format!("{} RGB subpixel sprite pipeline", descriptor.label)),\n                vertex_shader: descriptor.subpixel_vertex,\n                vertex_entry_point: "vs_subpixel_sprite".to_string(),\n                fragment_shader: descriptor.subpixel_fragment,\n                fragment_entry_point: "fs_subpixel_sprite".to_string(),\n                vertex_buffers: Vec::new(),\n                render_pass: descriptor.render_pass,\n                pipeline_layout: Some(descriptor.mono_pipeline_layout),\n                color_format: descriptor.color_format,\n                blend_mode: BlendMode::SubpixelDualSource,\n                primitive_topology: PrimitiveTopology::TriangleStrip,\n                depth_state: None,\n            },\n            descriptor.size,\n        )\n        .context("creating nova RGB subpixel sprite render pipeline")?;\n    let poly_sprites = device''',
)
replace_once(
    path,
    '''        shadows,\n        mono_sprites,\n        poly_sprites,''',
    '''        shadows,\n        mono_sprites,\n        #[cfg(target_os = "windows")]\n        subpixel_sprites,\n        poly_sprites,''',
)

# Select the pipeline from the atlas texture kind; no duplicate Scene primitive is needed.
path = "crates/gpui/src/platform/nova/draw.rs"
replace_once(
    path,
    '''                if let Some(resource_set) = sprite_resource_set(texture_id) {\n                    push_draw_step(\n                        steps,\n                        DrawStepDescriptor {\n                            pipeline: blend_pipelines.mono_sprites,''',
    '''                if let Some(resource_set) = sprite_resource_set(texture_id) {\n                    #[cfg(target_os = "windows")]\n                    let pipeline = if texture_id.kind == AtlasTextureKind::Subpixel {\n                        blend_pipelines.subpixel_sprites\n                    } else {\n                        blend_pipelines.mono_sprites\n                    };\n                    #[cfg(not(target_os = "windows"))]\n                    let pipeline = blend_pipelines.mono_sprites;\n                    push_draw_step(\n                        steps,\n                        DrawStepDescriptor {\n                            pipeline,''',
)

# DX12: match gpui-ce's no-scaling HWND presentation and dual-source blend factors.
path = "crates/nova-gfx/gfx-dx12/src/device.rs"
replace_once(
    path,
    "D3D12_BLEND_ONE, D3D12_BLEND_OP_ADD, D3D12_BLEND_SRC_ALPHA, D3D12_BLEND_ZERO,",
    "D3D12_BLEND_INV_SRC1_COLOR, D3D12_BLEND_ONE, D3D12_BLEND_OP_ADD, D3D12_BLEND_SRC1_COLOR,\n                D3D12_BLEND_SRC_ALPHA, D3D12_BLEND_ZERO,",
)
replace_once(
    path,
    "DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,",
    "DXGI_SCALING_NONE, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,",
)
replace_all(path, "DXGI_SCALING::default()", "DXGI_SCALING_NONE", 2)
replace_once(
    path,
    '''                BlendMode::AdditiveAlpha => (\n                    true.into(),\n                    D3D12_BLEND_ONE,\n                    D3D12_BLEND_INV_SRC_ALPHA,\n                    D3D12_BLEND_ONE,\n                    D3D12_BLEND_ONE,\n                ),''',
    '''                BlendMode::AdditiveAlpha => (\n                    true.into(),\n                    D3D12_BLEND_ONE,\n                    D3D12_BLEND_INV_SRC_ALPHA,\n                    D3D12_BLEND_ONE,\n                    D3D12_BLEND_ONE,\n                ),\n                BlendMode::SubpixelDualSource => (\n                    true.into(),\n                    D3D12_BLEND_SRC1_COLOR,\n                    D3D12_BLEND_INV_SRC1_COLOR,\n                    D3D12_BLEND_ONE,\n                    D3D12_BLEND_ZERO,\n                ),''',
)

# Vulkan: Windows Nova requires and enables dualSrcBlend, then uses SRC1 coverage exactly as DX12.
path = "crates/nova-gfx/gfx-vulkan/src/device.rs"
replace_once(
    path,
    '''    let create_info = vk::DeviceCreateInfo::default()\n        .queue_create_infos(&queue_infos)\n        .enabled_extension_names(&device_extensions);''',
    '''    let mut enabled_features = vk::PhysicalDeviceFeatures::default();\n    #[cfg(target_os = "windows")]\n    {\n        // Windows Nova has a single strict text contract: RGB subpixel composition requires\n        // independent destination attenuation for R/G/B, which Vulkan exposes through dualSrcBlend.\n        let supported_features = unsafe { instance.get_physical_device_features(physical_device) };\n        if supported_features.dual_src_blend != vk::TRUE {\n            return Err(VulkanError::Unavailable(\n                "Windows Nova Vulkan requires dualSrcBlend for RGB subpixel text".to_string(),\n            )\n            .into());\n        }\n        enabled_features.dual_src_blend = vk::TRUE;\n    }\n    let create_info = vk::DeviceCreateInfo::default()\n        .queue_create_infos(&queue_infos)\n        .enabled_extension_names(&device_extensions)\n        .enabled_features(&enabled_features);''',
)
regex_once(
    path,
    r'''        BlendMode::AdditiveAlpha => \{\n            color_blend_attachment = color_blend_attachment\n                \.blend_enable\(true\)\n                \.src_color_blend_factor\(vk::BlendFactor::ONE\)\n                \.dst_color_blend_factor\(vk::BlendFactor::ONE_MINUS_SRC_ALPHA\)\n                \.color_blend_op\(vk::BlendOp::ADD\)\n                \.src_alpha_blend_factor\(vk::BlendFactor::ONE\)\n                \.dst_alpha_blend_factor\(vk::BlendFactor::ONE\)\n                \.alpha_blend_op\(vk::BlendOp::ADD\);\n        \}''',
    '''        BlendMode::AdditiveAlpha => {\n            color_blend_attachment = color_blend_attachment\n                .blend_enable(true)\n                .src_color_blend_factor(vk::BlendFactor::ONE)\n                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)\n                .color_blend_op(vk::BlendOp::ADD)\n                .src_alpha_blend_factor(vk::BlendFactor::ONE)\n                .dst_alpha_blend_factor(vk::BlendFactor::ONE)\n                .alpha_blend_op(vk::BlendOp::ADD);\n        }\n        #[cfg(target_os = "windows")]\n        BlendMode::SubpixelDualSource => {\n            color_blend_attachment = color_blend_attachment\n                .blend_enable(true)\n                .src_color_blend_factor(vk::BlendFactor::SRC1_COLOR)\n                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC1_COLOR)\n                .color_blend_op(vk::BlendOp::ADD)\n                .src_alpha_blend_factor(vk::BlendFactor::ONE)\n                .dst_alpha_blend_factor(vk::BlendFactor::ZERO)\n                .alpha_blend_op(vk::BlendOp::ADD);\n        }''',
)

print("Nova RGB Subpixel migration applied")
