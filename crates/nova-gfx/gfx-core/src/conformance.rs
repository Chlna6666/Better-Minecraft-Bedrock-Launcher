//! GPU conformance scenarios shared by the desktop backend test suites.
//!
//! Each scenario is written once against the backend-neutral traits, so every backend
//! exercises the same code path instead of maintaining a copy per backend. The backend
//! crates call these from `#[ignore]`d hardware tests.
//!
//! Enabled by the `conformance` feature: the scenarios allocate GPU resources and submit
//! work, so they stay out of ordinary builds.

use std::num::NonZeroU32;

use crate::{
    BufferDescriptor, BufferUsage, ClearColor, ColorAttachmentDescriptor, CopyDirection,
    DepthAttachment, DepthAttachmentDescriptor, Device, Error, Extent2d, Format, LoadOp,
    MemoryLocation, Origin2d, PresentationDevice, RenderPassCommands, RenderPassDescriptor, Result,
    TextureBufferCopy, TextureDataLayout, TextureDescriptor, TextureDimension, TextureId,
    TextureTransferDevice, TextureUsage, TextureViewDescriptor, TextureViewId,
    TextureWriteDescriptor, texture_copy_buffer_bytes, texture_copy_row_pitch,
};

/// Clears a 4x multisampled color+depth pass and resolves the color into a single-sample
/// texture, then reads that texture back.
///
/// This covers the whole multisample chain a backend must implement: multisampled texture
/// allocation, attachment view creation, the multisampled pass itself, resolve, and the
/// post-resolve state needed for readback. It needs no shaders, so the same body runs on
/// every backend.
///
/// # Errors
/// Returns the first backend or validation error, or a mismatch between the resolved
/// pixels and the clear color.
pub fn msaa_resolve_roundtrip<D>(device: &mut D) -> Result<()>
where
    D: Device + TextureTransferDevice,
{
    const SAMPLES: u32 = 4;
    let extent = Extent2d::new(16, 16)?;
    let clear = ClearColor {
        red: 0.25,
        green: 0.5,
        blue: 0.75,
        alpha: 1.0,
    };

    let color = device.create_texture(&TextureDescriptor {
        label: Some("conformance msaa color".into()),
        size: extent,
        mip_level_count: 1,
        format: Format::Rgba8Unorm,
        usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
        sample_count: SAMPLES,
    })?;
    let color_view = device.create_texture_view(&TextureViewDescriptor {
        label: Some("conformance msaa color".into()),
        texture: color,
        base_mip_level: 0,
        mip_level_count: 1,
        format: Format::Rgba8Unorm,
    })?;
    let depth = device.create_texture(&TextureDescriptor {
        label: Some("conformance msaa depth".into()),
        size: extent,
        mip_level_count: 1,
        format: Format::Depth32Float,
        usage: TextureUsage::DEPTH_ATTACHMENT,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
        sample_count: SAMPLES,
    })?;
    let depth_view = device.create_texture_view(&TextureViewDescriptor {
        label: Some("conformance msaa depth".into()),
        texture: depth,
        base_mip_level: 0,
        mip_level_count: 1,
        format: Format::Depth32Float,
    })?;
    let resolved = device.create_texture(&TextureDescriptor {
        label: Some("conformance resolve".into()),
        size: extent,
        mip_level_count: 1,
        format: Format::Rgba8Unorm,
        usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC | TextureUsage::SAMPLED,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
        sample_count: 1,
    })?;
    let resolved_view = device.create_texture_view(&TextureViewDescriptor {
        label: Some("conformance resolve".into()),
        texture: resolved,
        base_mip_level: 0,
        mip_level_count: 1,
        format: Format::Rgba8Unorm,
    })?;
    let pass = device.create_render_pass(&RenderPassDescriptor {
        label: Some("conformance msaa".into()),
        color_attachments: vec![ColorAttachmentDescriptor {
            format: Format::Rgba8Unorm,
        }],
        depth_attachment: Some(DepthAttachmentDescriptor {
            format: Format::Depth32Float,
            stencil_format: None,
        }),
        sample_count: SAMPLES,
        resolve_target: None,
    })?;

    let commands = [color_view];
    let recorded = [RenderPassCommands {
        color_attachments: &commands,
        render_pass: pass,
        commands: &[],
        color_load_op: LoadOp::Clear(clear),
        clear_region: None,
        depth_attachment: Some(DepthAttachment {
            target: depth_view,
            depth_load_op: LoadOp::Clear(1.0),
            stencil_load_op: None,
        }),
        viewport: None,
        resolve_target: Some(resolved_view),
    }];
    let result = (|| -> Result<()> {
        PresentationDevice::render_to_textures(device, &recorded)?;
        let pixels = TextureTransferDevice::read_texture(device, resolved)?;
        // Tolerate one code unit of rounding in the resolve/readback path.
        let expect = |value: f32| (value * 255.0).round() as i32;
        let center = (8 * 16 + 8) * 4;
        let actual = &pixels.bytes[center..center + 4];
        for (index, value) in [clear.red, clear.green, clear.blue].into_iter().enumerate() {
            let delta = i32::from(actual[index]) - expect(value);
            if delta.abs() > 1 {
                return Err(Error::Backend(format!(
                    "resolved pixel channel {index} was {} but the clear color implies {}",
                    actual[index],
                    expect(value)
                )));
            }
        }
        if actual[3] != 255 {
            return Err(Error::Backend(format!(
                "resolved alpha was {} but the clear color implies 255",
                actual[3]
            )));
        }
        Ok(())
    })();

    let cleanup = (|| -> Result<()> {
        device.destroy_texture_view(resolved_view)?;
        device.destroy_texture(resolved)?;
        device.destroy_texture_view(depth_view)?;
        device.destroy_texture(depth)?;
        device.destroy_texture_view(color_view)?;
        device.destroy_texture(color)?;
        device.destroy_render_pass(pass)
    })();
    result.and(cleanup)
}

/// Allocates, views, clears and reads back one texture of every [`Format`].
///
/// Integer formats are only allocated and viewed: a float clear cannot describe them.
/// This is the format-mapping gate: a backend that forgets a format arm fails here
/// instead of at first use in a product path.
///
/// # Errors
/// Returns the first backend or validation error, or a mismatch between the resolved
/// pixels and the clear color.
pub fn format_allocation_roundtrip<D>(device: &mut D) -> Result<()>
where
    D: Device + TextureTransferDevice,
{
    let extent = Extent2d::new(4, 4)?;
    let formats = [
        Format::R8Unorm,
        Format::Rg8Unorm,
        Format::Bgra8Unorm,
        Format::Bgra8UnormSrgb,
        Format::Rgba8Unorm,
        Format::Rgba8UnormSrgb,
        Format::Rgba16Float,
        Format::R16Float,
        Format::Rg16Float,
        Format::R32Float,
        Format::Rgba32Float,
        Format::R8Uint,
        Format::Rg8Uint,
        Format::Rgba8Uint,
        Format::Depth32Float,
    ];
    for format in formats {
        let depth = format.is_depth();
        let usage = if depth {
            TextureUsage::DEPTH_ATTACHMENT
        } else if format.is_integer() {
            TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED
        } else {
            TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC | TextureUsage::SAMPLED
        };
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("conformance format".into()),
            size: extent,
            mip_level_count: 1,
            format,
            usage,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
            sample_count: 1,
        })?;
        let view = device.create_texture_view(&TextureViewDescriptor {
            label: Some("conformance format".into()),
            texture,
            base_mip_level: 0,
            mip_level_count: 1,
            format,
        })?;
        device.destroy_texture_view(view)?;
        device.destroy_texture(texture)?;
    }
    Ok(())
}

/// Copies a texture into a buffer and back into a second texture, then compares the pixels.
///
/// This exercises both buffer copy directions, the device-reported row alignment, and the
/// staging paths each backend needs (DX11 staging objects, DX12 placed footprints, Vulkan
/// transfers, OpenGL pixel pack/unpack).
///
/// # Errors
/// Returns the first backend or validation error, or a mismatch between the two textures.
pub fn texture_buffer_roundtrip<D>(device: &mut D) -> Result<()>
where
    D: Device + TextureTransferDevice,
{
    let extent = Extent2d::new(8, 4)?;
    let format = Format::Rgba8Unorm;
    let limits = device.texture_copy_limits();
    // Texture upload and readback are tightly packed; only the buffer carries the device pitch.
    let row = usize::try_from(u64::from(extent.width()) * u64::from(format.bytes_per_pixel()))
        .map_err(|_| Error::InvalidInput("copy row pitch exceeds usize".into()))?;
    let height = extent.height() as usize;
    // Distinct per-texel pattern so a row-order or pitch mistake cannot pass by accident.
    let mut source = vec![0u8; row * height];
    for y in 0..height {
        for x in 0..extent.width() as usize {
            let at = y * row + x * 4;
            source[at] = (x * 16) as u8;
            source[at + 1] = (y * 32) as u8;
            source[at + 2] = 0x40;
            source[at + 3] = 0xFF;
        }
    }

    let create = |device: &mut D| -> Result<(TextureId, TextureViewId)> {
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("conformance buffer copy".into()),
            size: extent,
            mip_level_count: 1,
            format,
            usage: TextureUsage::COPY_SRC | TextureUsage::COPY_DST | TextureUsage::SAMPLED,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
            sample_count: 1,
        })?;
        let view = device.create_texture_view(&TextureViewDescriptor {
            label: Some("conformance buffer copy".into()),
            texture,
            base_mip_level: 0,
            mip_level_count: 1,
            format,
        })?;
        Ok((texture, view))
    };
    let (source_texture, source_view) = create(device)?;
    let (destination_texture, destination_view) = create(device)?;
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("conformance buffer copy".into()),
        size: texture_copy_buffer_bytes(extent, format, limits),
        usage: BufferUsage::COPY_SRC | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::GpuOnly,
    })?;

    let result =
        (|| -> Result<()> {
            device.write_texture(
                TextureWriteDescriptor {
                    texture: source_texture,
                    mip_level: 0,
                    layout: TextureDataLayout {
                        offset: 0,
                        bytes_per_row: NonZeroU32::new(u32::try_from(row).map_err(|_| {
                            Error::InvalidInput("copy row pitch exceeds u32".into())
                        })?)
                        .ok_or_else(|| Error::InvalidInput("copy row pitch is zero".into()))?,
                        rows_per_image: NonZeroU32::new(extent.height())
                            .ok_or_else(|| Error::InvalidInput("copy height is zero".into()))?,
                    },
                    origin: Origin2d { x: 0, y: 0 },
                    size: extent,
                },
                &source,
            )?;
            device.copy_texture_buffer_batch(&[
                TextureBufferCopy {
                    buffer,
                    texture: source_texture,
                    direction: CopyDirection::TextureToBuffer,
                    buffer_offset: 0,
                    texture_origin: Origin2d { x: 0, y: 0 },
                    size: extent,
                },
                TextureBufferCopy {
                    buffer,
                    texture: destination_texture,
                    direction: CopyDirection::BufferToTexture,
                    buffer_offset: 0,
                    texture_origin: Origin2d { x: 0, y: 0 },
                    size: extent,
                },
            ])?;
            let pixels = device.read_texture(destination_texture)?;
            if pixels.bytes.len() != row * height {
                return Err(Error::Backend(format!(
                    "readback returned {} bytes for a {row}x{height} copy",
                    pixels.bytes.len()
                )));
            }
            for y in 0..height {
                for x in 0..extent.width() as usize {
                    let at = y * row + x * 4;
                    if pixels.bytes[at..at + 4] != source[at..at + 4] {
                        return Err(Error::Backend(format!(
                            "texel ({x}, {y}) was {:?} after the buffer roundtrip, expected {:?}",
                            &pixels.bytes[at..at + 4],
                            &source[at..at + 4]
                        )));
                    }
                }
            }
            Ok(())
        })();

    let cleanup = (|| -> Result<()> {
        device.destroy_buffer(buffer)?;
        device.destroy_texture_view(destination_view)?;
        device.destroy_texture(destination_texture)?;
        device.destroy_texture_view(source_view)?;
        device.destroy_texture(source_texture)
    })();
    result.and(cleanup)
}


/// Shader fixture for the compute resource-set conformance writer.
pub const COMPUTE_RESOURCE_SET_WRITER_WGSL: &str = r#"
@group(0) @binding(0) var<storage, read_write> values: array<vec4<u32>>;
@compute @workgroup_size(1)
fn write_data() {
    values[0] = vec4<u32>(0u, 255u, 0u, 255u);
}
"#;

/// Shader fixture for the compute resource-set conformance image reader.
pub const COMPUTE_RESOURCE_SET_READER_WGSL: &str = r#"
@group(0) @binding(0) var<storage, read> values: array<vec4<u32>>;
@group(0) @binding(1) var output_image: texture_storage_2d<rgba8unorm, write>;
@compute @workgroup_size(1)
fn write_image(@builtin(global_invocation_id) id: vec3<u32>) {
    let color = values[0];
    textureStore(
        output_image, vec2<i32>(id.xy),
        vec4<f32>(
            f32(color.x) / 255.0, f32(color.y) / 255.0,
            f32(color.z) / 255.0, f32(color.w) / 255.0,
        )
    );
}
"#;

/// Verify native compute buffer writes, subsequent compute buffer reads, and
/// storage-image writes in order, without any CPU readback between dispatches.
///
/// Each backend provides shader binaries compiled from the same two WGSL
/// entry points. Test both storage-only images (GENERAL on Vulkan) and images
/// that can also be sampled (read-only layout after a Vulkan dispatch).
/// Only the final texture readback synchronizes the CPU.
///
/// Writer WGSL:
/// `@group(0) @binding(0) var<storage, read_write> values: array<vec4<u32>>;`
/// `@compute @workgroup_size(1) fn write_data() { values[0] = vec4<u32>(0u, 255u, 0u, 255u); }`
///
/// Reader WGSL:
/// `@group(0) @binding(0) var<storage, read> values: array<vec4<u32>>;`
/// `@group(0) @binding(1) var output_image: texture_storage_2d<rgba8unorm, write>;`
/// `@compute @workgroup_size(1) fn write_image(@builtin(global_invocation_id) id: vec3<u32>) {`
/// `  let color = values[0];`
/// `  textureStore(output_image, vec2<i32>(id.xy), vec4<f32>(f32(color.x) / 255.0, f32(color.y) / 255.0, f32(color.z) / 255.0, f32(color.w) / 255.0));`
/// `}`
///
/// # Errors
/// Returns the first backend error or a pixel mismatch.
pub fn compute_resource_set_roundtrip<D>(
    device: &mut D,
    writer: crate::ShaderBinary,
    reader: crate::ShaderBinary,
) -> Result<()>
where
    D: Device + TextureTransferDevice,
{
    use crate::{
        BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor, BindGroupLayoutEntry,
        BindingResource, BindingType, BufferBinding, ComputeBindGroupDispatch,
        ShaderModuleDescriptor, ShaderStages, TextureBinding,
    };

    let writer_shader = device.create_shader_module(&ShaderModuleDescriptor {
        label: Some("compute conformance writer".into()),
        binary: writer,
    })?;
    let reader_shader = device.create_shader_module(&ShaderModuleDescriptor {
        label: Some("compute conformance reader".into()),
        binary: reader,
    })?;
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("compute conformance storage".into()),
        size: 16,
        usage: BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE,
        memory_location: MemoryLocation::GpuOnly,
    })?;
    let write_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: None,
        entries: vec![BindGroupLayoutEntry {
            binding: 0,
            binding_type: BindingType::StorageBufferReadWrite,
            stages: ShaderStages::COMPUTE,
        }],
    })?;
    let read_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: None,
        entries: vec![
            BindGroupLayoutEntry {
                binding: 0,
                binding_type: BindingType::StorageBuffer,
                stages: ShaderStages::COMPUTE,
            },
            BindGroupLayoutEntry {
                binding: 1,
                binding_type: BindingType::StorageTexture {
                    format: Format::Rgba8Unorm,
                    read_only: false,
                },
                stages: ShaderStages::COMPUTE,
            },
        ],
    })?;
    let buffer_range = BufferBinding {
        buffer,
        offset: 0,
        size: 16,
        stride: None,
    };
    let write_set = device.create_bind_group(&BindGroupDescriptor {
        label: None,
        layout: write_layout,
        entries: vec![BindGroupEntry {
            binding: 0,
            resource: BindingResource::Buffer(buffer_range),
        }],
    })?;
    const SIDE: u32 = 8;
    for sampled in [false, true] {
        let image = device.create_texture(&TextureDescriptor {
            label: Some("compute conformance output".into()),
            size: Extent2d::new(SIDE, SIDE)?,
            mip_level_count: 1,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::STORAGE
                | TextureUsage::COPY_SRC
                | (if sampled { TextureUsage::SAMPLED } else { TextureUsage::empty() }),
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
            sample_count: 1,
        })?;
        let image_view = device.create_texture_view(&TextureViewDescriptor {
            label: None,
            texture: image,
            base_mip_level: 0,
            mip_level_count: 1,
            format: Format::Rgba8Unorm,
        })?;
        let read_set = device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: read_layout,
            entries: vec![
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::Buffer(buffer_range),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Texture(TextureBinding { texture_view: image_view }),
                },
            ],
        })?;
        let result = (|| -> Result<()> {
            device.dispatch_compute_bind_group(&ComputeBindGroupDispatch {
                shader: writer_shader,
                bind_group: write_set,
                workgroups: [1, 1, 1],
            })?;
            device.dispatch_compute_bind_group(&ComputeBindGroupDispatch {
                shader: reader_shader,
                bind_group: read_set,
                workgroups: [SIDE, SIDE, 1],
            })?;
            let pixels = device.read_texture(image)?;
            let row = pixels.bytes_per_row as usize;
            if row < SIDE as usize * 4 || pixels.bytes.len() < row * SIDE as usize {
                return Err(Error::Backend("compute storage image readback is too short".into()));
            }
            for y in 0..SIDE as usize {
                for x in 0..SIDE as usize {
                    let offset = y * row + x * 4;
                    if pixels.bytes[offset..offset + 4] != [0, 255, 0, 255] {
                        return Err(Error::Backend(format!(
                            "compute image ({x}, {y}) sampled={sampled}: got {:?}",
                            &pixels.bytes[offset..offset + 4]
                        )));
                    }
                }
            }
            Ok(())
        })();
        device.destroy_bind_group(read_set)?;
        device.destroy_texture_view(image_view)?;
        device.destroy_texture(image)?;
        result?;
    }
    device.destroy_bind_group(write_set)?;
    device.destroy_bind_group_layout(read_layout)?;
    device.destroy_bind_group_layout(write_layout)?;
    device.destroy_buffer(buffer)?;
    device.destroy_shader_module(reader_shader)?;
    device.destroy_shader_module(writer_shader)?;
    Ok(())
}
