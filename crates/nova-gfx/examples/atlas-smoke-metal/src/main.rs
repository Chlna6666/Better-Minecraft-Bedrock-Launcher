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
        BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor, BindGroupLayoutEntry,
        BindingResource, BindingType, BufferBinding, BufferDescriptor, BufferUsage,
        DeviceDescriptor, Format, MemoryLocation, PipelineDevice, PipelineLayoutDescriptor,
        ResourceDevice, SamplerBinding, SamplerDescriptor, ShaderStage, ShaderStages,
        TextureBinding, TextureDescriptor, TextureDimension, TextureUsage, TextureViewDescriptor,
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
        let mut device = MetalDevice::new(&DeviceDescriptor {
            application_name: "nova-gfx atlas smoke metal".to_string(),
            ..DeviceDescriptor::default()
        })?;
        let _vertex_shader = compile_wgsl_to_msl(ATLAS_WGSL, ShaderStage::Vertex, "vs_main")?;
        let _fragment_shader = compile_wgsl_to_msl(ATLAS_WGSL, ShaderStage::Fragment, "fs_main")?;
        let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("atlas metal bind group layout".to_string()),
            entries: vec![
                BindGroupLayoutEntry {
                    binding: 0,
                    binding_type: BindingType::UniformBuffer,
                    stages: ShaderStages::FRAGMENT,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    binding_type: BindingType::SampledTexture,
                    stages: ShaderStages::FRAGMENT,
                },
                BindGroupLayoutEntry {
                    binding: 2,
                    binding_type: BindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
            ],
        })?;
        let _pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("atlas metal pipeline layout".to_string()),
            bind_group_layouts: vec![layout],
        })?;
        let uniform = device.create_buffer(&BufferDescriptor {
            label: Some("atlas metal uniform".to_string()),
            size: 16,
            usage: BufferUsage::UNIFORM,
            memory_location: MemoryLocation::CpuToGpu,
        })?;
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("atlas metal texture".to_string()),
            size: gfx_core::Extent2d::new(1, 1)?,
            mip_level_count: 1,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::SAMPLED,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        })?;
        let texture_view = device.create_texture_view(&TextureViewDescriptor {
            label: Some("atlas metal texture view".to_string()),
            texture,
            base_mip_level: 0,
            mip_level_count: 1,
            format: Format::Rgba8Unorm,
        })?;
        let sampler = device.create_sampler(&SamplerDescriptor::default())?;
        let _resource_set = device.create_bind_group(&BindGroupDescriptor {
            label: Some("atlas metal bind group".to_string()),
            layout,
            entries: vec![
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::Buffer(BufferBinding {
                        buffer: uniform,
                        offset: 0,
                        size: 16,
                        stride: None,
                    }),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Texture(TextureBinding { texture_view }),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Sampler(SamplerBinding { sampler }),
                },
            ],
        })?;
        eprintln!(
            "Metal atlas resource model initialized; native AppKit smoke test must run on macOS"
        );
        Ok(())
    }
}
