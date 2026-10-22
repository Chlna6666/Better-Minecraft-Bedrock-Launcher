use super::{
    ALBEDO_BINDING, DRAW_BINDING, DRAW_PARAMS_STRIDE, DRAW_SLOT_STRIDE, FRAME_BINDING,
    FRAME_PARAMS_STRIDE, INSTANCE_BINDING, INSTANCE_STRIDE, LIGHT_BINDING, LIGHT_STRIDE,
    METALLIC_ROUGHNESS_BINDING, NORMAL_MAP_BINDING, OCCLUSION_BINDING, PACKED_VERTEX_STRIDE,
    SAMPLER_BINDING, TANGENT_BINDING, VERTEX_BINDING,
};
use crate::{
    AlphaMode, Camera, Mat4, Mesh, PreparedLight, PreparedScene, ShadingModel, TextureAsset,
    TextureColorSpace, TriangleEdgeMask, Vertex,
};
use anyhow::{Context as _, anyhow};
use gfx_core::{
    BackendKind, BindGroupDescriptor, BindGroupEntry, BindGroupId, BindGroupLayoutId,
    BindingResource, BufferBinding, BufferDescriptor, BufferId, BufferUsage, Extent2d, Format,
    MemoryLocation, Origin2d, SamplerBinding, SamplerId, TextureBinding, TextureDataLayout,
    TextureDescriptor, TextureDimension, TextureId, TextureUsage, TextureViewDescriptor,
    TextureViewId, TextureWrite, TextureWriteDescriptor,
};
use gpui::Bounds;
use gpui::ExtensionDevice;
use std::convert::TryFrom;

const ALPHA_MODE_OPAQUE: u32 = 0;
const ALPHA_MODE_MASK: u32 = 1;
const ALPHA_MODE_BLEND: u32 = 2;
const SHADING_MODEL_METALLIC_ROUGHNESS: u32 = 0;
const SHADING_MODEL_UNLIT: u32 = 1;
const TEXTURE_FLAG_ALBEDO: u32 = 1;
const TEXTURE_FLAG_OCCLUSION: u32 = 2;
const TEXTURE_FLAG_UV: u32 = 4;
const TEXTURE_FLAG_METALLIC_ROUGHNESS: u32 = 8;
const LIGHT_KIND_AMBIENT: u32 = 0;
const LIGHT_KIND_DIRECTIONAL: u32 = 1;
const LIGHT_KIND_POINT: u32 = 2;
const LIGHT_KIND_SPOT: u32 = 3;

pub(super) struct MeshResources {
    pub(super) generation: u64,
    pub(super) vertices: BufferId,
    pub(super) vertex_bytes: usize,
    pub(super) tangents: Option<BufferId>,
    pub(super) tangent_bytes: usize,
    pub(super) indices: BufferId,
}

pub(super) struct TextureResources {
    pub(super) texture: TextureId,
    pub(super) view: TextureViewId,
}

pub(super) struct DrawBindingResources<'a> {
    pub(super) bind_group_layout: BindGroupLayoutId,
    pub(super) mesh: &'a MeshResources,
    pub(super) draw_buffer: BufferId,
    pub(super) draw_index: usize,
    pub(super) instance_buffer: BufferId,
    pub(super) instance_count: usize,
    pub(super) light_buffer: BufferId,
    pub(super) light_capacity: usize,
    pub(super) albedo_view: TextureViewId,
    pub(super) normal_texture_view: TextureViewId,
    pub(super) occlusion_view: TextureViewId,
    pub(super) metallic_roughness_view: TextureViewId,
    pub(super) material_sampler: SamplerId,
    pub(super) frame_buffer: BufferId,
}

pub(crate) struct EncodedMesh {
    pub(crate) vertices: Vec<u8>,
    pub(crate) tangents: Option<Vec<u8>>,
    pub(crate) indices: Vec<u8>,
}

pub(super) fn create_texture(
    device: &mut dyn ExtensionDevice,
    asset: &TextureAsset,
) -> gpui::Result<TextureResources> {
    let size = Extent2d::new(asset.width(), asset.height())?;
    let texture = device.create_texture(&TextureDescriptor {
        label: Some(format!("gpui-3d texture {}", asset.id().0)),
        size,
        mip_level_count: asset.mip_level_count(),
        format: match asset.color_space() {
            TextureColorSpace::Srgb => Format::Rgba8UnormSrgb,
            TextureColorSpace::Linear => Format::Rgba8Unorm,
        },
        usage: TextureUsage::SAMPLED | TextureUsage::COPY_DST,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
    })?;
    if let Err(error) = write_texture_mips(device, texture, asset) {
        device.destroy_texture(texture)?;
        return Err(error);
    }
    let view = match device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!("gpui-3d texture view {}", asset.id().0)),
        texture,
        base_mip_level: 0,
        mip_level_count: asset.mip_level_count(),
        format: match asset.color_space() {
            TextureColorSpace::Srgb => Format::Rgba8UnormSrgb,
            TextureColorSpace::Linear => Format::Rgba8Unorm,
        },
    }) {
        Ok(view) => view,
        Err(error) => {
            device.destroy_texture(texture)?;
            return Err(error.into());
        }
    };
    Ok(TextureResources { texture, view })
}

fn write_texture_mips(
    device: &mut dyn ExtensionDevice,
    texture: TextureId,
    asset: &TextureAsset,
) -> gpui::Result<()> {
    let mut writes = Vec::with_capacity(asset.mip_level_count() as usize);
    for mip_level in 0..asset.mip_level_count() {
        let (width, height) = asset
            .mip_extent(mip_level)
            .ok_or_else(|| anyhow!("GPUI 3D texture mip level is missing"))?;
        let size = Extent2d::new(width, height)?;
        let bytes_per_row = width
            .checked_mul(4)
            .ok_or_else(|| anyhow!("GPUI 3D texture row size exceeds u32"))?;
        let layout = TextureDataLayout::new(0, bytes_per_row, height)?;
        let data = asset
            .mip_pixels(mip_level)
            .ok_or_else(|| anyhow!("GPUI 3D texture mip pixels are missing"))?;
        writes.push(TextureWrite {
            descriptor: TextureWriteDescriptor {
                texture,
                mip_level,
                layout,
                origin: Origin2d { x: 0, y: 0 },
                size,
            },
            data,
        });
    }
    device.write_texture_batch(&writes)?;
    Ok(())
}

pub(super) fn destroy_texture(
    device: &mut dyn ExtensionDevice,
    texture: TextureResources,
) -> gpui::Result<()> {
    device.destroy_texture_view(texture.view)?;
    device.destroy_texture(texture.texture)?;
    Ok(())
}

pub(super) fn create_mesh(
    device: &mut dyn ExtensionDevice,
    mesh: &Mesh,
    backend: BackendKind,
) -> gpui::Result<MeshResources> {
    let encoded = encode_mesh(mesh)?;
    let EncodedMesh {
        vertices: vertex_data,
        tangents: tangent_data,
        indices: index_data,
    } = encoded;
    // Mesh geometry is written once when the mesh is built and only read afterwards, so it
    // belongs in device-local memory instead of consuming host-visible space. Metal and the
    // stub backends have no staging path for device-local buffers yet, so their geometry stays
    // host visible.
    let mesh_memory_location = match backend {
        BackendKind::Dx11 | BackendKind::Dx12 | BackendKind::Vulkan => MemoryLocation::GpuOnly,
        BackendKind::Metal | BackendKind::OpenGl | BackendKind::WebGl => MemoryLocation::CpuToGpu,
    };
    let vertex_buffer = create_storage_buffer(
        device,
        "gpui-3d vertices",
        vertex_data.len(),
        mesh_memory_location,
    )?;
    if let Err(error) = device.write_buffer(vertex_buffer, 0, &vertex_data) {
        device.destroy_buffer(vertex_buffer)?;
        return Err(error.into());
    }
    let index_buffer = match device.create_buffer(&BufferDescriptor {
        label: Some("gpui-3d indices".into()),
        size: u64::try_from(index_data.len()).context("3D index buffer exceeds address range")?,
        usage: BufferUsage::INDEX | BufferUsage::COPY_DST,
        memory_location: mesh_memory_location,
    }) {
        Ok(buffer) => buffer,
        Err(error) => {
            device.destroy_buffer(vertex_buffer)?;
            return Err(error.into());
        }
    };
    if let Err(error) = device.write_buffer(index_buffer, 0, &index_data) {
        device.destroy_buffer(index_buffer)?;
        device.destroy_buffer(vertex_buffer)?;
        return Err(error.into());
    }
    let (tangent_buffer, tangent_bytes) = if let Some(tangent_data) = tangent_data {
        let tangent_buffer = match create_storage_buffer(
            device,
            "gpui-3d tangents",
            tangent_data.len(),
            mesh_memory_location,
        ) {
            Ok(buffer) => buffer,
            Err(error) => {
                device.destroy_buffer(index_buffer)?;
                device.destroy_buffer(vertex_buffer)?;
                return Err(error);
            }
        };
        if let Err(error) = device.write_buffer(tangent_buffer, 0, &tangent_data) {
            device.destroy_buffer(tangent_buffer)?;
            device.destroy_buffer(index_buffer)?;
            device.destroy_buffer(vertex_buffer)?;
            return Err(error.into());
        }
        (Some(tangent_buffer), tangent_data.len())
    } else {
        (None, 0)
    };
    Ok(MeshResources {
        generation: mesh.generation(),
        vertices: vertex_buffer,
        vertex_bytes: vertex_data.len(),
        tangents: tangent_buffer,
        tangent_bytes,
        indices: index_buffer,
    })
}

pub(crate) fn encode_mesh(mesh: &Mesh) -> gpui::Result<EncodedMesh> {
    let mut vertices = Vec::new();
    let mut tangents = mesh.tangents().map(|_| Vec::new());
    let mut indices = Vec::new();
    let edge_masks = mesh
        .edge_masks()
        .filter(|masks| masks.iter().any(|mask| mask.bits() != 0));

    if let Some(edge_masks) = edge_masks {
        if edge_masks.len() != mesh.indices().len() / 3 {
            return Err(anyhow!("GPUI 3D mesh edge mask count is invalid").into());
        }
        vertices.reserve(mesh.indices().len() * PACKED_VERTEX_STRIDE as usize);
        indices.reserve(mesh.indices().len() * 4);
        for (triangle, (triangle_indices, mask)) in mesh
            .indices()
            .chunks_exact(3)
            .zip(edge_masks.iter().copied())
            .enumerate()
        {
            for (corner, &index) in triangle_indices.iter().enumerate() {
                let vertex = mesh
                    .vertices()
                    .get(index as usize)
                    .ok_or_else(|| anyhow!("GPUI 3D mesh index refers to a missing vertex"))?;
                let barycentrics = [
                    if corner == 0 { 1.0 } else { 0.0 },
                    if corner == 1 { 1.0 } else { 0.0 },
                    if corner == 2 { 1.0 } else { 0.0 },
                ];
                write_vertex(&mut vertices, vertex, barycentrics, mask.bits());
                if let Some(encoded_tangents) = &mut tangents {
                    let tangent = mesh
                        .tangents()
                        .and_then(|tangents| tangents.get(index as usize))
                        .ok_or_else(|| anyhow!("GPUI 3D mesh tangent data is incomplete"))?;
                    write_vec4(encoded_tangents, *tangent);
                }
                let expanded_index = u32::try_from(triangle * 3 + corner)
                    .context("GPUI 3D expanded mesh exceeds u32 draw range")?;
                indices.extend_from_slice(&expanded_index.to_ne_bytes());
            }
        }
    } else {
        vertices.reserve(mesh.vertices().len() * PACKED_VERTEX_STRIDE as usize);
        for (index, vertex) in mesh.vertices().iter().enumerate() {
            write_vertex(
                &mut vertices,
                vertex,
                [1.0, 0.0, 0.0],
                TriangleEdgeMask::NONE.bits(),
            );
            if let Some(encoded_tangents) = &mut tangents {
                let tangent = mesh
                    .tangents()
                    .and_then(|tangents| tangents.get(index))
                    .ok_or_else(|| anyhow!("GPUI 3D mesh tangent data is incomplete"))?;
                write_vec4(encoded_tangents, *tangent);
            }
        }
        indices.reserve(mesh.indices().len() * 4);
        for index in mesh.indices() {
            indices.extend_from_slice(&index.to_ne_bytes());
        }
    }

    Ok(EncodedMesh {
        vertices,
        tangents,
        indices,
    })
}

fn write_vertex(bytes: &mut Vec<u8>, vertex: &Vertex, barycentrics: [f32; 3], edge_mask: u8) {
    write_vec4(
        bytes,
        [
            vertex.position.x,
            vertex.position.y,
            vertex.position.z,
            barycentrics[0],
        ],
    );
    write_vec4(
        bytes,
        [
            vertex.normal.x,
            vertex.normal.y,
            vertex.normal.z,
            barycentrics[1],
        ],
    );
    write_vec4(
        bytes,
        [
            vertex.uv.x,
            vertex.uv.y,
            barycentrics[2],
            f32::from(edge_mask),
        ],
    );
    write_vec4(bytes, vertex.color);
}

pub(super) fn create_draw_resource_set(
    device: &mut dyn ExtensionDevice,
    bindings: DrawBindingResources<'_>,
) -> gpui::Result<BindGroupId> {
    let DrawBindingResources {
        bind_group_layout,
        mesh,
        draw_buffer,
        draw_index,
        instance_buffer,
        instance_count,
        light_buffer,
        light_capacity,
        albedo_view,
        normal_texture_view,
        occlusion_view,
        metallic_roughness_view,
        material_sampler,
        frame_buffer,
    } = bindings;
    let draw_offset = u64::try_from(draw_index * DRAW_SLOT_STRIDE)
        .context("3D draw offset exceeds address range")?;
    let instance_bytes = instance_count
        .max(1)
        .checked_mul(INSTANCE_STRIDE as usize)
        .ok_or_else(|| anyhow!("3D instance buffer size overflow"))?;
    let (tangent_buffer, tangent_offset, tangent_bytes) = if let Some(tangents) = mesh.tangents {
        (
            tangents,
            0,
            u64::try_from(mesh.tangent_bytes.max(16)).context("3D tangent buffer size overflow")?,
        )
    } else {
        (draw_buffer, draw_offset, 16)
    };
    Ok(device.create_bind_group(&BindGroupDescriptor {
        label: Some("gpui-3d mesh draw resource set".into()),
        layout: bind_group_layout,
        entries: vec![
            BindGroupEntry {
                binding: VERTEX_BINDING,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: mesh.vertices,
                    offset: 0,
                    size: u64::try_from(mesh.vertex_bytes)
                        .context("3D vertex buffer size overflow")?,
                    stride: Some(PACKED_VERTEX_STRIDE),
                }),
            },
            BindGroupEntry {
                binding: DRAW_BINDING,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: draw_buffer,
                    offset: draw_offset,
                    size: u64::from(DRAW_PARAMS_STRIDE),
                    stride: Some(DRAW_PARAMS_STRIDE),
                }),
            },
            BindGroupEntry {
                binding: LIGHT_BINDING,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: light_buffer,
                    offset: 0,
                    size: u64::try_from(light_capacity * LIGHT_STRIDE as usize)
                        .context("3D light buffer size overflow")?,
                    stride: Some(LIGHT_STRIDE),
                }),
            },
            BindGroupEntry {
                binding: ALBEDO_BINDING,
                resource: BindingResource::Texture(TextureBinding {
                    texture_view: albedo_view,
                }),
            },
            BindGroupEntry {
                binding: SAMPLER_BINDING,
                resource: BindingResource::Sampler(SamplerBinding {
                    sampler: material_sampler,
                }),
            },
            BindGroupEntry {
                binding: NORMAL_MAP_BINDING,
                resource: BindingResource::Texture(TextureBinding {
                    texture_view: normal_texture_view,
                }),
            },
            BindGroupEntry {
                binding: OCCLUSION_BINDING,
                resource: BindingResource::Texture(TextureBinding {
                    texture_view: occlusion_view,
                }),
            },
            BindGroupEntry {
                binding: METALLIC_ROUGHNESS_BINDING,
                resource: BindingResource::Texture(TextureBinding {
                    texture_view: metallic_roughness_view,
                }),
            },
            BindGroupEntry {
                binding: TANGENT_BINDING,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: tangent_buffer,
                    offset: tangent_offset,
                    size: tangent_bytes,
                    stride: Some(16),
                }),
            },
            BindGroupEntry {
                binding: INSTANCE_BINDING,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: instance_buffer,
                    offset: 0,
                    size: u64::try_from(instance_bytes)
                        .context("3D instance buffer size exceeds address range")?,
                    stride: Some(INSTANCE_STRIDE),
                }),
            },
            BindGroupEntry {
                binding: FRAME_BINDING,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: frame_buffer,
                    offset: 0,
                    size: u64::try_from(FRAME_PARAMS_STRIDE)
                        .context("3D frame buffer size exceeds address range")?,
                    stride: None,
                }),
            },
        ],
    })?)
}

pub(super) fn create_storage_buffer(
    device: &mut dyn ExtensionDevice,
    label: &str,
    bytes: usize,
    memory_location: MemoryLocation,
) -> gpui::Result<BufferId> {
    let size = u64::try_from(bytes.max(1)).context("GPUI 3D buffer size exceeds address range")?;
    Ok(device.create_buffer(&BufferDescriptor {
        label: Some(label.into()),
        size,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location,
    })?)
}

pub(super) fn destroy_mesh(
    device: &mut dyn ExtensionDevice,
    mesh: MeshResources,
) -> gpui::Result<()> {
    device.destroy_buffer(mesh.indices)?;
    if let Some(tangents) = mesh.tangents {
        device.destroy_buffer(tangents)?;
    }
    device.destroy_buffer(mesh.vertices)?;
    Ok(())
}

pub(super) fn grow_capacity(current: usize, required: usize) -> gpui::Result<usize> {
    if required <= current {
        return Ok(current);
    }
    current
        .checked_next_power_of_two()
        .and_then(|capacity| capacity.max(required).checked_next_power_of_two())
        .ok_or_else(|| anyhow!("GPUI 3D scene buffer capacity overflow"))
}

/// Encodes the camera, viewport, and extension-bounds values shared by every draw in one frame.
///
/// These values change whenever the camera or the viewport moves. Keeping them in a single small
/// uniform buffer lets a camera change upload [`FRAME_PARAMS_STRIDE`] bytes instead of re-encoding
/// every draw slot.
pub(super) fn encode_frame_params(
    view_projection: Mat4,
    camera: Camera,
    bounds: Bounds<gpui::ScaledPixels>,
    render_target_size: gfx_core::Extent2d,
    blend_edge_feather_px: f32,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(FRAME_PARAMS_STRIDE);
    write_matrix(&mut bytes, view_projection.columns());
    write_vec4(&mut bytes, [camera.eye.x, camera.eye.y, camera.eye.z, 0.0]);
    write_vec4(
        &mut bytes,
        [
            f64::from(bounds.origin.x) as f32,
            f64::from(bounds.origin.y) as f32,
            f64::from(bounds.size.width) as f32,
            f64::from(bounds.size.height) as f32,
        ],
    );
    write_vec4(
        &mut bytes,
        [
            render_target_size.width() as f32,
            render_target_size.height() as f32,
            blend_edge_feather_px,
            0.0,
        ],
    );
    debug_assert_eq!(bytes.len(), FRAME_PARAMS_STRIDE);
    bytes
}

/// Encodes per-draw material state that is independent of the camera and viewport.
pub(crate) fn encode_draws(scene: &PreparedScene) -> gpui::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(scene.draws.len().max(1) * DRAW_SLOT_STRIDE);
    let light_count =
        u32::try_from(scene.lights.len()).context("GPUI 3D light count exceeds u32")?;
    for draw in &scene.draws {
        encode_draw_slot(draw, light_count, &mut bytes)?;
    }
    if scene.draws.is_empty() {
        bytes.resize(DRAW_SLOT_STRIDE, 0);
    }
    Ok(bytes)
}

/// Encodes one padded draw slot for a draw (or one representative per instanced batch).
///
/// Offscreen batching submits one draw slot per batch instead of one per instance, so the
/// streaming draw buffer grows with distinct material/mesh combinations rather than node count.
pub(crate) fn encode_draw_slot(
    draw: &crate::PreparedDraw,
    light_count: u32,
    bytes: &mut Vec<u8>,
) -> gpui::Result<()> {
    let slot_start = bytes.len();
    write_vec4(bytes, draw.material.base_color);
    write_vec4(bytes, draw.material.emissive);
    write_f32(bytes, draw.material.metallic);
    write_f32(bytes, draw.material.roughness);
    write_f32(bytes, draw.material.alpha_cutoff);
    let normal_mapping_enabled = draw.material.shading_model == ShadingModel::MetallicRoughness
        && draw.material.normal_texture.is_some();
    write_u32(bytes, u32::from(normal_mapping_enabled));
    write_f32(bytes, draw.material.occlusion_strength);
    write_u32(bytes, alpha_mode_value(draw.material.alpha_mode));
    write_u32(bytes, light_count);
    let shading_model = match draw.material.shading_model {
        ShadingModel::MetallicRoughness => SHADING_MODEL_METALLIC_ROUGHNESS,
        ShadingModel::Unlit => SHADING_MODEL_UNLIT,
    };
    write_u32(bytes, shading_model);
    let mut texture_flags = 0;
    if draw.material.albedo_texture.is_some() {
        texture_flags |= TEXTURE_FLAG_ALBEDO;
    }
    if draw.material.occlusion_texture.is_some() {
        texture_flags |= TEXTURE_FLAG_OCCLUSION;
    }
    if draw.mesh.uses_uv_regions() {
        texture_flags |= TEXTURE_FLAG_UV;
    }
    if draw.material.metallic_roughness_texture.is_some() {
        texture_flags |= TEXTURE_FLAG_METALLIC_ROUGHNESS;
    }
    write_u32(bytes, texture_flags);

    let encoded_size = bytes.len() - slot_start;
    if encoded_size > DRAW_PARAMS_STRIDE as usize {
        return Err(anyhow!(
                "GPUI 3D DrawParams encoding is {encoded_size} bytes but WGSL stride is {DRAW_PARAMS_STRIDE}"
            )
            .into());
    }
    let slot_end = slot_start
        .checked_add(DRAW_SLOT_STRIDE)
        .ok_or_else(|| anyhow!("GPUI 3D draw slot size overflow"))?;
    bytes.resize(slot_end, 0);

    Ok(())
}
pub(crate) fn encode_instances(scene: &PreparedScene) -> gpui::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(scene.draws.len().max(1) * INSTANCE_STRIDE as usize);
    encode_instances_into(scene, &mut bytes)?;
    Ok(bytes)
}

/// Appends instance records to a reusable staging buffer.
///
/// The offscreen renderer reuses its staging buffer across frames so a dirty frame does not
/// allocate the full instance table again; the GPUI scene-view path keeps the allocating wrapper.
pub(crate) fn encode_instances_into(
    scene: &PreparedScene,
    bytes: &mut Vec<u8>,
) -> gpui::Result<()> {
    let start_len = bytes.len();
    for draw in &scene.draws {
        let inverse = draw
            .world
            .inverse()
            .ok_or_else(|| anyhow!("GPUI 3D draw has a singular world transform"))?;
        write_matrix(bytes, draw.world.columns());
        write_matrix(bytes, transpose(inverse).columns());
        write_vec4(
            bytes,
            [
                draw.pixel_offset.x,
                draw.pixel_offset.y,
                draw.depth_bias,
                0.0,
            ],
        );
    }
    if scene.draws.is_empty() && bytes.len() == start_len {
        bytes.resize(INSTANCE_STRIDE as usize, 0);
    }
    debug_assert_eq!((bytes.len() - start_len) % INSTANCE_STRIDE as usize, 0);
    Ok(())
}

pub(crate) fn encode_lights(lights: &[PreparedLight]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(lights.len().max(1) * LIGHT_STRIDE as usize);
    encode_lights_into(lights, &mut bytes);
    bytes
}

/// Appends light records to a reusable staging buffer for the offscreen renderer.
pub(crate) fn encode_lights_into(lights: &[PreparedLight], bytes: &mut Vec<u8>) {
    let start_len = bytes.len();
    for light in lights {
        match *light {
            PreparedLight::Ambient { color, intensity } => {
                write_light_header(bytes, LIGHT_KIND_AMBIENT);
                write_vec4(bytes, [0.0, 0.0, 0.0, 0.0]);
                write_vec4(bytes, [0.0, 0.0, 0.0, 0.0]);
                write_vec4(bytes, [color[0], color[1], color[2], intensity]);
                write_spot_cone_cosines(bytes, 0.0, 0.0);
            }
            PreparedLight::Directional {
                direction,
                color,
                intensity,
            } => {
                write_light_header(bytes, LIGHT_KIND_DIRECTIONAL);
                write_vec4(bytes, [0.0, 0.0, 0.0, 0.0]);
                write_vec4(bytes, [direction.x, direction.y, direction.z, 0.0]);
                write_vec4(bytes, [color[0], color[1], color[2], intensity]);
                write_spot_cone_cosines(bytes, 0.0, 0.0);
            }
            PreparedLight::Point {
                position,
                color,
                intensity,
                range,
            } => {
                write_light_header(bytes, LIGHT_KIND_POINT);
                write_vec4(bytes, [position.x, position.y, position.z, range]);
                write_vec4(bytes, [0.0, 0.0, 0.0, 0.0]);
                write_vec4(bytes, [color[0], color[1], color[2], intensity]);
                write_spot_cone_cosines(bytes, 0.0, 0.0);
            }
            PreparedLight::Spot {
                position,
                direction,
                color,
                intensity,
                range,
                inner_angle,
                outer_angle,
            } => {
                write_light_header(bytes, LIGHT_KIND_SPOT);
                write_vec4(bytes, [position.x, position.y, position.z, range]);
                write_vec4(bytes, [direction.x, direction.y, direction.z, 0.0]);
                write_vec4(bytes, [color[0], color[1], color[2], intensity]);
                write_spot_cone_cosines(bytes, outer_angle.cos(), inner_angle.cos());
            }
        }
    }
    if lights.is_empty() && bytes.len() == start_len {
        bytes.resize(LIGHT_STRIDE as usize, 0);
    }
    debug_assert_eq!((bytes.len() - start_len) % LIGHT_STRIDE as usize, 0);
}

fn write_light_header(bytes: &mut Vec<u8>, light_kind: u32) {
    write_u32(bytes, light_kind);
    bytes.resize(bytes.len() + 12, 0);
}

fn write_spot_cone_cosines(bytes: &mut Vec<u8>, outer_cone_cosine: f32, inner_cone_cosine: f32) {
    write_vec2(bytes, [outer_cone_cosine, inner_cone_cosine]);
    bytes.resize(bytes.len() + 8, 0);
}

pub(super) fn alpha_mode_value(mode: AlphaMode) -> u32 {
    match mode {
        AlphaMode::Opaque => ALPHA_MODE_OPAQUE,
        AlphaMode::Mask => ALPHA_MODE_MASK,
        AlphaMode::Blend => ALPHA_MODE_BLEND,
    }
}

pub(super) fn transpose(matrix: crate::Mat4) -> crate::Mat4 {
    let columns = matrix.columns();
    crate::Mat4::from_columns(std::array::from_fn(|column| {
        std::array::from_fn(|row| columns[row][column])
    }))
}

pub(crate) fn write_matrix(bytes: &mut Vec<u8>, matrix: [[f32; 4]; 4]) {
    for column in matrix {
        write_vec4(bytes, column);
    }
}

pub(crate) fn write_vec4(bytes: &mut Vec<u8>, vector: [f32; 4]) {
    for value in vector {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
}

pub(crate) fn write_f32(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_ne_bytes());
}

pub(super) fn write_vec2(bytes: &mut Vec<u8>, vector: [f32; 2]) {
    for value in vector {
        write_f32(bytes, value);
    }
}

pub(super) fn write_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_ne_bytes());
}

#[cfg(test)]
mod tests {
    use super::encode_mesh;
    use crate::{Mesh, TriangleEdgeMask, Vec2, Vec3, Vertex};

    fn vertices() -> Vec<Vertex> {
        [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::new(1.0, 1.0, 0.0)]
            .into_iter()
            .map(|position| Vertex {
                position,
                normal: Vec3::Z,
                uv: Vec2::ZERO,
                color: [1.0; 4],
            })
            .collect()
    }

    fn float(bytes: &[u8], offset: usize) -> f32 {
        f32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn mesh_without_edge_masks_keeps_shared_vertices_and_indices() {
        let mesh = Mesh::new(vertices(), vec![0, 1, 2, 0, 2, 3]).unwrap();
        let encoded = encode_mesh(&mesh).unwrap();

        assert_eq!(encoded.vertices.len(), 4 * 64);
        assert_eq!(encoded.indices.len(), 6 * 4);
        assert!(encoded.tangents.is_none());
        let indices = encoded
            .indices
            .chunks_exact(4)
            .map(|bytes| u32::from_ne_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(indices, [0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn mesh_with_edge_masks_expands_triangle_corners_and_barycentrics() {
        let mesh = Mesh::new(vertices(), vec![0, 2, 1, 0, 2, 3])
            .unwrap()
            .with_edge_masks(vec![
                TriangleEdgeMask::new([true, false, true]),
                TriangleEdgeMask::NONE,
            ])
            .unwrap();
        let encoded = encode_mesh(&mesh).unwrap();

        assert_eq!(encoded.vertices.len(), 6 * 64);
        assert!(encoded.tangents.is_none());
        let indices = encoded
            .indices
            .chunks_exact(4)
            .map(|bytes| u32::from_ne_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(indices, [0, 1, 2, 3, 4, 5]);
        for (corner, expected) in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
            .into_iter()
            .enumerate()
        {
            let base = corner * 64;
            assert_eq!(
                [
                    float(&encoded.vertices, base + 12),
                    float(&encoded.vertices, base + 28),
                    float(&encoded.vertices, base + 40),
                ],
                expected
            );
            assert_eq!(float(&encoded.vertices, base + 44), 0b101 as f32);
        }
        assert_eq!(float(&encoded.vertices, 3 * 64 + 44), 0.0);
    }

    #[test]
    fn mesh_tangents_encode_to_separate_stream() {
        let source = Mesh::new(
            [
                Vertex {
                    position: Vec3::ZERO,
                    normal: Vec3::Z,
                    uv: Vec2::ZERO,
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::X,
                    normal: Vec3::Z,
                    uv: Vec2::new(1.0, 0.0),
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::Y,
                    normal: Vec3::Z,
                    uv: Vec2::new(0.0, 1.0),
                    color: [1.0; 4],
                },
            ],
            [0, 1, 2],
        )
        .unwrap()
        .generate_tangents()
        .unwrap()
        .into_parts()
        .0;
        let encoded = encode_mesh(&source).unwrap();

        assert_eq!(encoded.vertices.len(), source.vertices().len() * 64);
        let encoded_tangents = encoded.tangents.unwrap();
        assert_eq!(encoded_tangents.len(), source.vertices().len() * 16);
        let first_tangent = [
            float(&encoded_tangents, 0),
            float(&encoded_tangents, 4),
            float(&encoded_tangents, 8),
            float(&encoded_tangents, 12),
        ];
        assert!((first_tangent[0] - 1.0).abs() < 1.0e-5);
        assert!(first_tangent[1].abs() < 1.0e-5);
        assert!(first_tangent[2].abs() < 1.0e-5);
        assert_eq!(first_tangent[3], 1.0);
    }

    #[test]
    fn mesh_with_edge_masks_expands_tangents_in_triangle_order() {
        let source = Mesh::new(vertices(), [0, 2, 1])
            .unwrap()
            .with_tangents([
                [1.0, 0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0, 1.0],
                [-1.0, 0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0, 1.0],
            ])
            .unwrap()
            .with_edge_masks([TriangleEdgeMask::ALL])
            .unwrap();
        let encoded = encode_mesh(&source).unwrap();
        let encoded_tangents = encoded.tangents.unwrap();

        assert_eq!(encoded_tangents.len(), 3 * 16);
        for (corner, source_index) in [0, 2, 1].into_iter().enumerate() {
            for component in 0..4 {
                assert_eq!(
                    float(&encoded_tangents, corner * 16 + component * 4),
                    source.tangents().unwrap()[source_index][component]
                );
            }
        }
    }
}
