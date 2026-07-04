#[cfg(target_vendor = "apple")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    apple_example::run()
}

#[cfg(not(target_vendor = "apple"))]
fn main() {
    eprintln!("nova-atlas-smoke-metal requires an Apple target with Metal");
}

#[cfg(target_vendor = "apple")]
mod apple_example {
    use gfx_core::{
        BufferBinding, BufferDesc, BufferUsage, DeviceDesc, Format, GfxPipelineDevice,
        GfxResourceDevice, MemoryLocation, PipelineLayoutDesc, ResourceBinding,
        ResourceBindingResource, ResourceBindingType, ResourceSetDesc, ResourceSetLayoutDesc,
        ResourceSetLayoutEntry, SamplerBinding, SamplerDesc, ShaderStage, ShaderStages,
        TextureBinding, TextureDesc, TextureDimension, TextureUsage, TextureViewDesc,
    };
    use gfx_metal::MetalDevice;
    use gfx_shader::compile_wgsl_to_msl;

    const ATLAS_WGSL: &str = r"
struct Uniforms {
    tint: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

@group(0) @binding(1)
var atlas_texture: texture_2d<f32>;

@group(0) @binding(2)
var atlas_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-0.62, -0.62),
        vec2<f32>( 0.62, -0.62),
        vec2<f32>( 0.62,  0.62),
        vec2<f32>(-0.62, -0.62),
        vec2<f32>( 0.62,  0.62),
        vec2<f32>(-0.62,  0.62),
    );
    return vec4<f32>(positions[vertex_index], 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return uniforms.tint;
}
";

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let mut device = MetalDevice::new(&DeviceDesc {
            application_name: "nova-gfx atlas smoke metal".to_string(),
            ..DeviceDesc::default()
        })?;
        let _vertex_shader = compile_wgsl_to_msl(ATLAS_WGSL, ShaderStage::Vertex, "vs_main")?;
        let _fragment_shader = compile_wgsl_to_msl(ATLAS_WGSL, ShaderStage::Fragment, "fs_main")?;
        let layout = device.create_resource_set_layout(&ResourceSetLayoutDesc {
            label: Some("atlas metal resource set layout".to_string()),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 1,
                    binding_type: ResourceBindingType::SampledTexture,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 2,
                    binding_type: ResourceBindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
            ],
        })?;
        let _pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDesc {
            label: Some("atlas metal pipeline layout".to_string()),
            resource_set_layouts: vec![layout],
        })?;
        let uniform = device.create_buffer(&BufferDesc {
            label: Some("atlas metal uniform".to_string()),
            size: 16,
            usage: BufferUsage::UNIFORM,
            memory_location: MemoryLocation::CpuToGpu,
        })?;
        let texture = device.create_texture(&TextureDesc {
            label: Some("atlas metal texture".to_string()),
            size: gfx_core::Extent2d::new(1, 1)?,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::SAMPLED,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        })?;
        let texture_view = device.create_texture_view(&TextureViewDesc {
            label: Some("atlas metal texture view".to_string()),
            texture,
            format: Format::Rgba8Unorm,
        })?;
        let sampler = device.create_sampler(&SamplerDesc::default())?;
        let _resource_set = device.create_resource_set(&ResourceSetDesc {
            label: Some("atlas metal resource set".to_string()),
            layout,
            bindings: vec![
                ResourceBinding {
                    binding: 0,
                    resource: ResourceBindingResource::Buffer(BufferBinding {
                        buffer: uniform,
                        offset: 0,
                        size: 16,
                        stride: None,
                    }),
                },
                ResourceBinding {
                    binding: 1,
                    resource: ResourceBindingResource::Texture(TextureBinding { texture_view }),
                },
                ResourceBinding {
                    binding: 2,
                    resource: ResourceBindingResource::Sampler(SamplerBinding { sampler }),
                },
            ],
        })?;
        eprintln!(
            "Metal atlas resource model initialized; native AppKit smoke test must run on macOS"
        );
        Ok(())
    }
}
