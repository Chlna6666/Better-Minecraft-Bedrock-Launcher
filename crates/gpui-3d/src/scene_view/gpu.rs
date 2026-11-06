use super::{
    ALBEDO_BINDING, DRAW_BINDING, DRAW_PARAMS_STRIDE, DRAW_SLOT_STRIDE, FRAME_BINDING,
    FRAME_PARAMS_STRIDE, INSTANCE_BINDING, INSTANCE_STRIDE, LIGHT_BINDING, LIGHT_STRIDE,
    METALLIC_ROUGHNESS_BINDING, NORMAL_MAP_BINDING, OCCLUSION_BINDING, PACKED_VERTEX_STRIDE,
    SAMPLER_BINDING, TANGENT_BINDING, VERTEX_BINDING, VISIBLE_INSTANCE_BINDING,
    VOXEL_VERTEX_STRIDE,
};
use crate::{
    AlphaMode, Camera, Mat4, MaterialId, Mesh, NodeId, PreparedDraw, PreparedLight, PreparedScene, ShadingModel, TextureAsset,
    TextureColorSpace, TriangleEdgeMask, Vertex,
};
use anyhow::{Context as _, anyhow};
use gfx_core::{
    BackendKind, BindGroupDescriptor, BindGroupEntry, BindGroupId, BindGroupLayoutId,
    BindingResource, BufferBinding, BufferDescriptor, BufferId, BufferUsage, Extent2d, Format,
    IndexFormat, MemoryLocation, Origin2d, SamplerBinding, SamplerId, TextureBinding,
    TextureDataLayout, TextureDescriptor, TextureDimension, TextureId, TextureUsage,
    TextureViewDescriptor, TextureViewId, TextureWrite, TextureWriteDescriptor,
};
use gpui::Bounds;
use gpui::ExtensionDevice;
use std::collections::{BTreeSet, HashMap};
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
    pub(super) generated: Option<super::generated::GeneratedMesh>,
    pub(super) generation: u64,
    pub(super) vertices: BufferId,
    pub(super) vertex_binding_bytes: usize,
    pub(super) vertex_stride: u32,
    pub(super) vertex_base: u32,
    pub(super) tangents: Option<BufferId>,
    pub(super) tangent_bytes: usize,
    pub(super) indices: BufferId,
    pub(super) index_format: IndexFormat,
    pub(super) index_offset: u64,
    pub(super) procedural_quads: bool,
    pub(super) allocation: Option<ArenaAllocation>,
}

pub(super) struct PendingMeshUpload {
    pub(super) buffer: BufferId,
    pub(super) offset: u64,
    pub(super) bytes: Vec<u8>,
}

const VOXEL_ARENA_PAGE_BYTES: u64 = 8 * 1024 * 1024;

pub(super) struct ArenaAllocation {
    page: usize,
    offset: u64,
    bytes: u64,
}

struct ArenaPage {
    buffer: BufferId,
    capacity: u64,
    used: u64,
    live: usize,
    live_bytes: u64,
    free_ranges: Vec<(u64, u64)>,
    retired_ranges: Vec<(u64, u64)>,
}

impl ArenaPage {
    fn available_bytes(&self) -> u64 {
        self.free_ranges
            .iter()
            .map(|(_, len)| *len)
            .fold(self.capacity - self.used, u64::max)
    }
    fn reserve(&mut self, bytes: u64) -> Option<u64> {
        if let Some(index) = self.free_ranges.iter().position(|(_, len)| *len >= bytes) {
            let (offset, len) = self.free_ranges[index];
            if len == bytes {
                self.free_ranges.swap_remove(index);
            } else {
                self.free_ranges[index] = (offset + bytes, len - bytes);
            }
            self.live += 1;
            self.live_bytes += bytes;
            return Some(offset);
        }
        if self.capacity.checked_sub(self.used)? < bytes {
            return None;
        }
        let offset = self.used;
        self.used += bytes;
        self.live += 1;
        self.live_bytes += bytes;
        Some(offset)
    }

    fn retire(&mut self, offset: u64, bytes: u64) -> gpui::Result<()> {
        let remaining = self
            .live_bytes
            .checked_sub(bytes)
            .ok_or_else(|| anyhow!("voxel arena allocation exceeds live page bytes"))?;
        self.live -= 1;
        self.live_bytes = remaining;
        self.retired_ranges.push((offset, bytes));
        Ok(())
    }

    fn reclaim_retired(&mut self) {
        self.free_ranges.append(&mut self.retired_ranges);
        self.free_ranges.sort_unstable_by_key(|range| range.0);
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(self.free_ranges.len());
        for (offset, len) in self.free_ranges.drain(..) {
            if let Some((start, previous_len)) = merged.last_mut()
                && *start + *previous_len == offset
            {
                *previous_len += len;
            } else {
                merged.push((offset, len));
            }
        }
        self.free_ranges = merged;
    }
}

#[derive(Default)]
pub(super) struct MeshArena {
    // GPU-generated face pages require writable storage. Keep them isolated
    // from the immutable CPU-packed voxel vertex/index pages.
    generated_output: bool,
    generated_limits: gfx_core::BufferBindingLimits,
    pages: Vec<Option<ArenaPage>>,
    available_pages: BTreeSet<(u64, usize)>,
    free_pages: Vec<usize>,
}

impl MeshArena {
    pub(super) fn generated_output(limits: gfx_core::BufferBindingLimits) -> Self {
        Self { generated_output: true, generated_limits: limits, ..Self::default() }
    }

    // Shared page offsets must satisfy both the driver's storage binding
    // offset alignment and the 16-byte GeneratedFace shader address unit.
    // Compute their least common multiple; do not assume power-of-two limits.
    fn generated_face_alignment(limits: gfx_core::BufferBindingLimits) -> gpui::Result<u64> {
        let alignment = limits.storage_offset_alignment;
        if alignment == 0 {
            return Err(anyhow!("storage offset alignment was not reported").into());
        }
        let (mut a, mut b) = (alignment, 16u64);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        alignment.checked_div(a).and_then(|v| v.checked_mul(16))
            .ok_or_else(|| anyhow!("generated face alignment overflow").into())
    }

    pub(super) fn allocate(
        &mut self,
        device: &mut dyn ExtensionDevice,
        backend: BackendKind,
        bytes: u64,
    ) -> gpui::Result<(ArenaAllocation, BufferId, u64, u64)> {
        // Match the actual device's storage-buffer binding offset alignment,
        // while also maintaining the shader's 16-byte GeneratedFace addressing.
        // No backend name or assumed 256-byte limit is involved in this plan.
        let stride = if self.generated_output {
            Self::generated_face_alignment(self.generated_limits)?
        } else {
            u64::from(VOXEL_VERTEX_STRIDE)
        };
        let aligned = bytes.max(1)
            .checked_add(stride - 1)
            .ok_or_else(|| anyhow!("voxel mesh exceeds arena address range"))?
            / stride * stride;
        if self.generated_output {
            self.generated_limits.validate_range(0, aligned, false)?;
        }
        if let Some(reserved) = self.reserve_page(aligned)? {
            return Ok(reserved);
        }
        let capacity = VOXEL_ARENA_PAGE_BYTES.max(
            aligned
                .checked_next_power_of_two()
                .ok_or_else(|| anyhow!("voxel arena page size overflow"))?,
        );
        // The 8 MiB page is an allocation-policy target, not a GPU limit.
        // Cap generated pages to the device's reported maximum storage range.
        let capacity = if self.generated_output {
            capacity.min(self.generated_limits.max_storage_binding_bytes)
        } else {
            capacity
        };
        if capacity < aligned {
            return Err(anyhow!("generated face exceeds device storage range").into());
        }
        let memory_location = buffer_memory_location(backend);
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(if self.generated_output {
                "gpui-3d generated face pool".into()
            } else {
                "gpui-3d voxel mesh arena".into()
            }),
            size: capacity,
            usage: if self.generated_output {
                BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE | BufferUsage::COPY_DST
            } else {
                BufferUsage::STORAGE | BufferUsage::INDEX | BufferUsage::COPY_DST
            },
            memory_location,
        })?;
        let page = ArenaPage {
            buffer,
            capacity,
            used: aligned,
            live: 1,
            live_bytes: aligned,
            free_ranges: Vec::new(),
            retired_ranges: Vec::new(),
        };
        let available = page.available_bytes();
        let index = if let Some(index) = self.free_pages.pop() {
            self.pages[index] = Some(page);
            index
        } else {
            let index = self.pages.len();
            self.pages.push(Some(page));
            index
        };
        if available > 0 {
            self.available_pages.insert((available, index));
        }
        Ok((
            ArenaAllocation {
                page: index,
                offset: 0,
                bytes: aligned,
            },
            buffer,
            0,
            capacity,
        ))
    }

    fn reserve_page(
        &mut self,
        bytes: u64,
    ) -> gpui::Result<Option<(ArenaAllocation, BufferId, u64, u64)>> {
        let Some(&(available, index)) = self.available_pages.range((bytes, 0)..).next() else {
            return Ok(None);
        };
        let page = self.pages[index]
            .as_mut()
            .ok_or_else(|| anyhow!("voxel arena available page is missing"))?;
        let offset = page
            .reserve(bytes)
            .ok_or_else(|| anyhow!("voxel arena available range is too small"))?;
        self.available_pages.remove(&(available, index));
        let remaining = page.available_bytes();
        if remaining > 0 {
            self.available_pages.insert((remaining, index));
        }
        Ok(Some((
            ArenaAllocation {
                page: index,
                offset,
                bytes,
            },
            page.buffer,
            offset,
            page.capacity,
        )))
    }

    pub(super) fn release(
        &mut self,
        device: &mut dyn ExtensionDevice,
        allocation: ArenaAllocation,
    ) -> gpui::Result<()> {
        let Some(page) = self.pages.get_mut(allocation.page).and_then(Option::as_mut) else {
            return Err(anyhow!("voxel arena allocation refers to a missing page").into());
        };
        if page.live == 0 {
            return Err(anyhow!("voxel arena page has no live allocations").into());
        }
        if page.live == 1 {
            if page.live_bytes != allocation.bytes {
                return Err(anyhow!("voxel arena final allocation size mismatch").into());
            }
            device.destroy_buffer(page.buffer)?;
            self.available_pages
                .remove(&(page.available_bytes(), allocation.page));
            self.pages[allocation.page] = None;
            self.free_pages.push(allocation.page);
        } else {
            page.retire(allocation.offset, allocation.bytes)?;
        }
        Ok(())
    }

    /// Called only after the host has confirmed all prior GPU submissions completed.
    pub(super) fn reclaim_after_gpu_idle(&mut self) {
        for (index, page) in self.pages.iter_mut().enumerate() {
            let Some(page) = page else { continue };
            if page.retired_ranges.is_empty() {
                continue;
            }
            self.available_pages
                .remove(&(page.available_bytes(), index));
            page.reclaim_retired();
            let available = page.available_bytes();
            if available > 0 {
                self.available_pages.insert((available, index));
            }
        }
    }

    pub(super) fn has_retired_ranges(&self) -> bool {
        self.pages
            .iter()
            .flatten()
            .any(|page| !page.retired_ranges.is_empty())
    }

    pub(super) fn stats(&self) -> (usize, u64, u64, u64) {
        self.pages
            .iter()
            .flatten()
            .fold((0, 0, 0, 0), |(count, capacity, live, dead), page| {
                (
                    count + 1,
                    capacity + page.capacity,
                    live + page.live_bytes,
                    dead + page.used.saturating_sub(page.live_bytes),
                )
            })
    }
}

pub(super) struct TextureResources {
    pub(super) texture: TextureId,
    pub(super) view: TextureViewId,
}

pub(super) struct DrawBindingResources<'a> {
    pub(super) shadow: Option<BufferId>,
    pub(super) bind_group_layout: BindGroupLayoutId,
    pub(super) mesh: &'a MeshResources,
    pub(super) draw_buffer: BufferId,
    pub(super) draw_capacity: usize,
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
    pub(super) visible_instances: Option<(BufferId, u64)>,
    pub(super) draw_slots: BufferId,
    pub(super) draw_slot_capacity: usize,
}

pub(crate) struct EncodedMesh {
    pub(crate) vertices: Vec<u8>,
    pub(crate) tangents: Option<Vec<u8>>,
    pub(crate) indices: Vec<u8>,
    pub(crate) index_format: IndexFormat,
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
    compact_voxel: bool,
    arena: &mut MeshArena,
) -> gpui::Result<(MeshResources, Option<PendingMeshUpload>)> {
    let encoded = if compact_voxel {
        encode_voxel_mesh(mesh)?
    } else {
        encode_mesh(mesh)?
    };
    let EncodedMesh {
        vertices: mut vertex_data,
        tangents: tangent_data,
        indices: index_data,
        index_format,
    } = encoded;
    if compact_voxel {
        let procedural_quads = mesh.compact_voxel_quad_count().is_some();
        let vertex_bytes = vertex_data.len();
        let index_bytes = index_data.len();
        let requested = u64::try_from(
            vertex_bytes
                .checked_add(index_bytes)
                .ok_or_else(|| anyhow!("voxel mesh size overflow"))?,
        )
        .context("voxel mesh exceeds address range")?;
        let (allocation, buffer, offset, capacity) = arena.allocate(device, backend, requested)?;
        let vertex_base = u32::try_from(offset / u64::from(VOXEL_VERTEX_STRIDE));
        let binding_bytes = usize::try_from(capacity);
        let (Ok(vertex_base), Ok(vertex_binding_bytes)) = (vertex_base, binding_bytes) else {
            arena.release(device, allocation)?;
            return Err(anyhow!("voxel arena exceeds shader or host address range").into());
        };
        let index_offset = offset + vertex_bytes as u64;
        vertex_data.extend_from_slice(&index_data);
        return Ok((
            MeshResources {
                generated: None,
                generation: mesh.generation(),
                vertices: buffer,
                vertex_binding_bytes,
                vertex_stride: VOXEL_VERTEX_STRIDE,
                vertex_base,
                tangents: None,
                tangent_bytes: 0,
                indices: buffer,
                index_format,
                index_offset,
                procedural_quads,
                allocation: Some(allocation),
            },
            Some(PendingMeshUpload {
                buffer,
                offset,
                bytes: vertex_data,
            }),
        ));
    }
    // Mesh geometry is written once when the mesh is built and only read afterwards, so it
    // belongs in device-local memory instead of consuming host-visible space. Metal and the
    // stub backends have no staging path for device-local buffers yet, so their geometry stays
    // host visible.
    let buffer_memory_location = buffer_memory_location(backend);
    let vertex_buffer = create_storage_buffer(
        device,
        "gpui-3d vertices",
        vertex_data.len(),
        buffer_memory_location,
    )?;
    if let Err(error) = device.write_buffer(vertex_buffer, 0, &vertex_data) {
        device.destroy_buffer(vertex_buffer)?;
        return Err(error.into());
    }
    let index_buffer = match device.create_buffer(&BufferDescriptor {
        label: Some("gpui-3d indices".into()),
        size: u64::try_from(index_data.len()).context("3D index buffer exceeds address range")?,
        usage: BufferUsage::INDEX | BufferUsage::COPY_DST,
        memory_location: buffer_memory_location,
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
            buffer_memory_location,
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
    Ok((
        MeshResources {
            generated: None,
            generation: mesh.generation(),
            vertices: vertex_buffer,
            vertex_binding_bytes: vertex_data.len(),
            vertex_stride: PACKED_VERTEX_STRIDE,
            vertex_base: 0,
            tangents: tangent_buffer,
            tangent_bytes,
            indices: index_buffer,
            index_format,
            index_offset: 0,
            procedural_quads: false,
            allocation: None,
        },
        None,
    ))
}

pub(super) fn buffer_memory_location(backend: BackendKind) -> MemoryLocation {
    match backend {
        BackendKind::Dx11 | BackendKind::Dx12 | BackendKind::Vulkan => MemoryLocation::GpuOnly,
        BackendKind::Metal | BackendKind::OpenGl | BackendKind::WebGl => MemoryLocation::CpuToGpu,
    }
}

pub(super) fn can_encode_voxel_mesh(mesh: &Mesh) -> bool {
    mesh.edge_masks().is_none() && mesh.has_unorm_colors()
}

fn encode_voxel_mesh(mesh: &Mesh) -> gpui::Result<EncodedMesh> {
    let vertex_count = mesh.vertex_count();
    let use_u16_indices = vertex_count <= u16::MAX as usize + 1;
    let mut vertices = Vec::with_capacity(vertex_count * VOXEL_VERTEX_STRIDE as usize);
    if let Some(packed) = mesh.compact_voxel_vertices() {
        for vertex in packed {
            vertices.extend_from_slice(&vertex[..VOXEL_VERTEX_STRIDE as usize]);
        }
    } else {
        for vertex in mesh.vertices() {
            for position in [vertex.position.x, vertex.position.y, vertex.position.z] {
                vertices.extend_from_slice(&position.to_ne_bytes());
            }
            let color = vertex
                .color
                .iter()
                .enumerate()
                .fold(0u32, |packed, (index, channel)| {
                    packed | ((*channel * 255.0).round() as u32) << (index * 8)
                });
            vertices.extend_from_slice(&color.to_ne_bytes());
            for uv in [vertex.uv.x, vertex.uv.y] {
                vertices.extend_from_slice(&uv.to_ne_bytes());
            }
        }
    }
    let procedural_quads = mesh.compact_voxel_quad_count().is_some();
    let mut indices = Vec::with_capacity(if procedural_quads {
        0
    } else {
        mesh.index_count() * if use_u16_indices { 2 } else { 4 }
    });
    if !procedural_quads {
        for &index in mesh.indices() {
            if index as usize >= vertex_count {
                return Err(anyhow!("GPUI 3D mesh index refers to a missing vertex").into());
            }
            if use_u16_indices {
                indices.extend_from_slice(&(index as u16).to_ne_bytes());
            } else {
                indices.extend_from_slice(&index.to_ne_bytes());
            }
        }
    }
    Ok(EncodedMesh {
        vertices,
        tangents: None,
        indices,
        index_format: if use_u16_indices {
            IndexFormat::Uint16
        } else {
            IndexFormat::Uint32
        },
    })
}

pub(crate) fn encode_mesh(mesh: &Mesh) -> gpui::Result<EncodedMesh> {
    let mut vertices = Vec::new();
    let mut tangents = mesh.tangents().map(|_| Vec::new());
    let mut indices = Vec::new();
    let edge_masks = mesh
        .edge_masks()
        .filter(|masks| masks.iter().any(|mask| mask.bits() != 0));
    let use_u16_indices = if edge_masks.is_some() {
        mesh.indices().len() <= u16::MAX as usize + 1
    } else {
        mesh.vertices().len() <= u16::MAX as usize + 1
    };
    let index_width = if use_u16_indices { 2 } else { 4 };

    if let Some(edge_masks) = edge_masks {
        if edge_masks.len() != mesh.indices().len() / 3 {
            return Err(anyhow!("GPUI 3D mesh edge mask count is invalid").into());
        }
        vertices.reserve(mesh.indices().len() * PACKED_VERTEX_STRIDE as usize);
        indices.reserve(mesh.indices().len() * index_width);
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
                if use_u16_indices {
                    indices.extend_from_slice(&(expanded_index as u16).to_ne_bytes());
                } else {
                    indices.extend_from_slice(&expanded_index.to_ne_bytes());
                }
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
        indices.reserve(mesh.indices().len() * index_width);
        for index in mesh.indices() {
            if use_u16_indices {
                indices.extend_from_slice(&(*index as u16).to_ne_bytes());
            } else {
                indices.extend_from_slice(&index.to_ne_bytes());
            }
        }
    }

    Ok(EncodedMesh {
        vertices,
        tangents,
        indices,
        index_format: if use_u16_indices {
            IndexFormat::Uint16
        } else {
            IndexFormat::Uint32
        },
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
        shadow,
        bind_group_layout,
        mesh,
        draw_buffer,
        draw_capacity,
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
        visible_instances,
        draw_slots,
        draw_slot_capacity,
    } = bindings;
    let draw_bytes = draw_capacity
        .checked_mul(DRAW_PARAMS_STRIDE as usize)
        .ok_or_else(|| anyhow!("3D draw buffer size overflow"))?;
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
        (draw_buffer, 0, 16)
    };
    Ok(device.create_bind_group(&BindGroupDescriptor {
        label: Some("gpui-3d mesh draw resource set".into()),
        layout: bind_group_layout,
        entries: vec![
            BindGroupEntry {
                binding: 16,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: draw_slots,
                    offset: 0,
                    size: u64::try_from(draw_slot_capacity.max(1) * 4)
                        .context("3D draw slot mapping buffer exceeds address range")?,
                    stride: Some(4),
                }),
            },
            BindGroupEntry {
                binding: 14,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: mesh.generated.as_ref().map_or(draw_buffer, |m| m.templates),
                    offset: mesh.generated.as_ref().map_or(0, |m| m.template_offset),
                    size: mesh.generated.as_ref().map_or(16, |m| m.template_bytes),
                    stride: None,
                }),
            },
            BindGroupEntry {
                binding: 13,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: if mesh.generated.is_some() {
                        mesh.vertices
                    } else {
                        draw_buffer
                    },
                    offset: 0,
                    size: if mesh.generated.is_some() {
                        mesh.vertex_binding_bytes as u64
                    } else {
                        16
                    },
                    stride: None,
                }),
            },
            BindGroupEntry {
                binding: 12,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: mesh
                        .generated
                        .as_ref()
                        .map_or(draw_buffer, |mesh| mesh.templates),
                    offset: mesh.generated.as_ref().map_or(0, |mesh| mesh.template_offset),
                    size: mesh
                        .generated
                        .as_ref()
                        .map_or(16, |mesh| mesh.template_bytes),
                    stride: None,
                }),
            },
            BindGroupEntry {
                binding: VISIBLE_INSTANCE_BINDING,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: visible_instances.map_or(draw_buffer, |(buffer, _)| buffer),
                    offset: 0,
                    size: visible_instances.map_or(4, |(_, size)| size),
                    stride: Some(4),
                }),
            },
            BindGroupEntry {
                binding: VERTEX_BINDING,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: mesh.vertices,
                    offset: 0,
                    size: u64::try_from(mesh.vertex_binding_bytes)
                        .context("3D vertex buffer size overflow")?,
                    stride: Some(mesh.vertex_stride),
                }),
            },
            BindGroupEntry {
                binding: DRAW_BINDING,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: draw_buffer,
                    offset: 0,
                    size: u64::try_from(draw_bytes)
                        .context("3D draw buffer size exceeds address range")?,
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
            BindGroupEntry {
                binding: 15,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: shadow.unwrap_or(draw_buffer),
                    offset: 0,
                    size: if shadow.is_some() {
                        512 * 512 * 4
                    } else {
                        DRAW_PARAMS_STRIDE as u64
                    },
                    stride: Some(4),
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
    arena: &mut MeshArena,
    generated_arena: &mut MeshArena,
) -> gpui::Result<()> {
    if let Some(allocation) = mesh.allocation {
        if let Some(generated) = &mesh.generated {
            // Mesh output and template source each have an independently
            // retired slice of the device-level generated page pool. Never
            // destroy the page while other Sections still reference it.
            if let Some(template_allocation) = generated.template_allocation {
                device.destroy_buffer(generated.arguments)?;
                generated_arena.release(device, template_allocation)?;
            } else {
                device.destroy_buffer_batch(&[generated.templates, generated.arguments])?;
            }
            return generated_arena.release(device, allocation);
        }
        return arena.release(device, allocation);
    }
    let mut buffers = [mesh.indices, mesh.vertices, mesh.indices, mesh.indices];
    let mut count = 2;
    for buffer in [
        mesh.tangents,
        mesh.generated.as_ref().map(|generated| generated.templates),
    ]
    .into_iter()
    .flatten()
    {
        buffers[count] = buffer;
        count += 1;
    }
    device.destroy_buffer_batch(&buffers[..count])?;
    Ok(())
}

/// Reserve one future growth band on DX11 after entering a large streamed
/// voxel scene. At 32K and 64K draws, rebuilding every native SRV bind group
/// caused 357ms and 644ms stalls in captured traces. A bounded 128K physical
/// capacity moves that unavoidable rebind to the 16K threshold instead.
///
/// This is deliberately not applied to small scenes or other backends, and
/// never grows past 128K unless the actual draw count requires it.
pub(super) fn dx11_streaming_headroom(backend: BackendKind, draws: usize) -> usize {
    if backend == BackendKind::Dx11 && draws > 16_384 && draws <= 131_072 {
        131_072
    } else {
        draws
    }
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
    // Default fog is disabled. The frame owner fills the trailing fog uniforms.
    bytes.resize(FRAME_PARAMS_STRIDE, 0);
    debug_assert_eq!(bytes.len(), FRAME_PARAMS_STRIDE);
    bytes
}

/// Encodes per-draw material state that is independent of the camera and viewport.
#[cfg(test)]
pub(crate) fn encode_draws(scene: &PreparedScene) -> gpui::Result<Vec<u8>> {
    encode_draw_records(&scene.draws, scene.lights.len())
}

/// Retained immutable draw parameter records. A large streamed world can render
/// thousands of draws sharing the same material; their 80-byte GPU parameters
/// need not be rebuilt from the material's fields for every draw on every delta.
/// Exact identity of an encoded GPU DrawParams record.
///
/// MaterialId is stable across Material::clone, including cloned variants with
/// changed fields. A per-snapshot generation therefore distinguishes immutable
/// Arc<Material> snapshots while retaining the previous fast shared-material
/// record cache. UV support and the number of lights also affect GPU bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DrawParamKey {
    material: MaterialId,
    snapshot_generation: u64,
    uv_regions: bool,
    light_count: u32,
}

struct MaterialRecord {
    // Keep the underlying snapshot resident: Arc::make_mut will clone instead
    // of mutating it behind a committed GPU record's identity.
    material: std::sync::Arc<crate::Material>,
    bytes: [u8; DRAW_PARAMS_STRIDE as usize],
    generation: u64,
}

/// Retain material snapshots for the live draw working set. A fixed 8K or
/// 64K ceiling makes a 100K+ section scene re-encode the uncached tail on
/// every publication. Bound historical snapshots independently of scene size.
fn material_record_cache_limit(draws: usize) -> usize {
    draws.saturating_mul(2).clamp(8_192, 262_144)
}

#[derive(Default)]
pub(super) struct DrawRecordCache {
    light_count: Option<u32>,
    // MaterialId survives Arc::make_mut. Different immutable snapshots with
    // the same logical ID must not evict one another on every draw; retained
    // Arc references keep the snapshot pointer identity stable while cached.
    records: HashMap<(MaterialId, bool, usize), MaterialRecord>,
    next_generation: u64,
}

impl DrawRecordCache {
    /// Directly resolve logical draws into persistent physical GPU DrawParams
    /// slots. The transient logical 80-byte-per-draw table is never constructed.
    ///
    /// An empty `output` with matching `next_keys` means the committed CPU
    /// snapshot is reusable after successful GPU upload/binding preparation.
    /// The retained material snapshot cache owns immutable Arc<Material> values:
    /// mutating a material via Arc::make_mut necessarily changes its identity.
    pub(super) fn encode_stable(
        &mut self,
        draws: &[PreparedDraw],
        lights: usize,
        mapping: &[u32],
        high_water: usize,
        previous_keys: &[DrawParamKey],
        previous_bytes: &[u8],
        next_keys: &mut Vec<DrawParamKey>,
        output: &mut Vec<u8>,
        scratch: &mut Vec<u8>,
    ) -> gpui::Result<usize> {
        const STRIDE: usize = DRAW_PARAMS_STRIDE as usize;
        let light_count = u32::try_from(lights).context("GPUI 3D light count exceeds u32")?;
        if self.light_count != Some(light_count) {
            self.records.clear();
            self.light_count = Some(light_count);
        }
        if draws.len() != mapping.len() {
            return Err(anyhow!("GPUI 3D physical draw mapping length mismatch").into());
        }
        let byte_len = high_water.max(1)
            .checked_mul(STRIDE)
            .ok_or_else(|| anyhow!("GPUI 3D physical DrawParams table overflows usize"))?;
        next_keys.clear();
        next_keys.extend_from_slice(&previous_keys[..previous_keys.len().min(high_water)]);
        // Size the cache for the live (possibly 100K+) draw working set so
        // ordinary Section updates do not repeatedly re-encode uncached
        // material snapshots. Prune snapshots no longer held by the scene.
        let cache_limit = material_record_cache_limit(draws.len());
        if self.records.len() >= cache_limit {
            self.records.retain(|_, record| std::sync::Arc::strong_count(&record.material) > 1);
        }
        let mut dirty = Vec::<(usize, [u8; STRIDE])>::new();
        for (draw, &slot) in draws.iter().zip(mapping) {
            let slot = slot as usize;
            if slot >= high_water {
                return Err(anyhow!("GPUI 3D physical draw slot outside table").into());
            }
            let material = draw.material.id();
            let uv_regions = draw.mesh.uses_uv_regions();
            let cache_key = (
                material,
                uv_regions,
                std::sync::Arc::as_ptr(&draw.material) as usize,
            );
            let start = slot * STRIDE;
            let (key, record) = if let Some(cached) = self.records.get(&cache_key)
                && std::sync::Arc::ptr_eq(&cached.material, &draw.material)
            {
                (
                    DrawParamKey {
                        material,
                        snapshot_generation: cached.generation,
                        uv_regions,
                        light_count,
                    },
                    cached.bytes,
                )
            } else {
                scratch.clear();
                encode_draw_record(draw, light_count, scratch, STRIDE)?;
                let record: [u8; STRIDE] = scratch[..STRIDE]
                    .try_into()
                    .expect("DrawParams encoder must write one complete record");
                // Even when the snapshot cache is full, identical 80-byte
                // content at the same stable physical slot is NOT dirty.
                // Reuse its previously committed key to avoid publishing an
                // artificial generation change and re-uploading the GPU table.
                let same_resident_record = previous_keys.get(slot)
                    .filter(|prev| {
                        prev.material == material
                            && prev.uv_regions == uv_regions
                            && prev.light_count == light_count
                    })
                    .filter(|_| {
                        previous_bytes.get(start..start + STRIDE) == Some(record.as_slice())
                    });
                let generation = if let Some(key) = same_resident_record {
                    key.snapshot_generation
                } else {
                    self.next_generation = self.next_generation.wrapping_add(1);
                    self.next_generation
                };
                if self.records.len() < cache_limit || self.records.contains_key(&cache_key) {
                    self.records.insert(cache_key, MaterialRecord {
                        material: draw.material.clone(),
                        bytes: record,
                        generation,
                    });
                }
                (
                    DrawParamKey {
                        material,
                        snapshot_generation: generation,
                        uv_regions,
                        light_count,
                    },
                    record,
                )
            };
            if previous_keys.get(slot) != Some(&key)
                || previous_bytes.get(start..start + STRIDE).is_none()
            {
                dirty.push((slot, record));
            }
            if slot >= next_keys.len() {
                next_keys.resize(slot + 1, key);
            }
            next_keys[slot] = key;
        }

        output.clear();
        if dirty.is_empty()
            && next_keys == previous_keys
            && previous_bytes.len() == byte_len
        {
            return Ok(0);
        }
        // Material changes are infrequent: copy the old physical table only if
        // actual records changed or the table's occupied extent changed.
        output.extend_from_slice(previous_bytes);
        output.resize(byte_len, 0);
        for (slot, record) in &dirty {
            let start = slot * STRIDE;
            output[start..start + STRIDE].copy_from_slice(record);
        }
        Ok(dirty.len())
    }

    pub(super) fn encode(
        &mut self,
        draws: &[PreparedDraw],
        lights: usize,
        bytes: &mut Vec<u8>,
        keys: &mut Vec<DrawParamKey>,
    ) -> gpui::Result<()> {
        let light_count = u32::try_from(lights).context("GPUI 3D light count exceeds u32")?;
        if self.light_count != Some(light_count) {
            self.records.clear();
            self.light_count = Some(light_count);
        }
        let cache_limit = material_record_cache_limit(draws.len());
        if self.records.len() >= cache_limit {
            self.records.retain(|_, record| std::sync::Arc::strong_count(&record.material) > 1);
        }
        let stride = DRAW_PARAMS_STRIDE as usize;
        bytes.clear();
        bytes.reserve(draws.len().max(1).saturating_mul(stride));
        keys.clear();
        keys.reserve(draws.len());
        for draw in draws {
            let material = draw.material.id();
            let uv_regions = draw.mesh.uses_uv_regions();
            let key = (
                material,
                uv_regions,
                std::sync::Arc::as_ptr(&draw.material) as usize,
            );
            if let Some(record) = self.records.get(&key)
                && std::sync::Arc::ptr_eq(&record.material, &draw.material)
            {
                bytes.extend_from_slice(&record.bytes);
                keys.push(DrawParamKey {
                    material,
                    snapshot_generation: record.generation,
                    uv_regions,
                    light_count,
                });
                continue;
            }
            let start = bytes.len();
            encode_draw_record(draw, light_count, bytes, stride)?;
            self.next_generation = self.next_generation.wrapping_add(1);
            let generation = self.next_generation;
            let record: [u8; DRAW_PARAMS_STRIDE as usize] = bytes[start..start + stride]
                .try_into()
                .expect("draw encoder must write exactly one aligned record");
            // Keep the live material working set; bound historical snapshots
            // even if the scene has accumulated many distinct states.
            if self.records.len() < cache_limit || self.records.contains_key(&key) {
                self.records.insert(key, MaterialRecord {
                    material: draw.material.clone(),
                    bytes: record,
                    generation,
                });
            }
            keys.push(DrawParamKey {
                material,
                snapshot_generation: generation,
                uv_regions,
                light_count,
            });
        }
        if draws.is_empty() {
            bytes.resize(stride, 0);
        }
        Ok(())
    }
}

pub(crate) fn encode_draws_into(scene: &PreparedScene, bytes: &mut Vec<u8>) -> gpui::Result<()> {
    encode_draw_records_into(&scene.draws, scene.lights.len(), bytes)
}

#[cfg(test)]
pub(super) fn encode_draw_records(
    draws: &[crate::PreparedDraw],
    lights: usize,
) -> gpui::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(draws.len().max(1) * DRAW_PARAMS_STRIDE as usize);
    encode_draw_records_into(draws, lights, &mut bytes)?;
    Ok(bytes)
}

fn encode_draw_records_into(
    draws: &[crate::PreparedDraw],
    lights: usize,
    bytes: &mut Vec<u8>,
) -> gpui::Result<()> {
    let stride = DRAW_PARAMS_STRIDE as usize;
    bytes.clear();
    bytes.reserve(draws.len().max(1).saturating_mul(stride));
    let light_count = u32::try_from(lights).context("GPUI 3D light count exceeds u32")?;
    for draw in draws {
        encode_draw_record(draw, light_count, bytes, stride)?;
    }
    if draws.is_empty() {
        bytes.resize(stride, 0);
    }
    Ok(())
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
    encode_draw_record(draw, light_count, bytes, DRAW_SLOT_STRIDE)
}

fn encode_draw_record(
    draw: &crate::PreparedDraw,
    light_count: u32,
    bytes: &mut Vec<u8>,
    stride: usize,
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
    match draw.material.surface {
        crate::SurfaceKind::Standard => {}
        crate::SurfaceKind::Water => texture_flags |= 64,
        crate::SurfaceKind::Glass => texture_flags |= 128,
    }
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
    if draw.material.pbr_texture_layout == crate::PbrTextureLayout::MetallicEmissiveRoughness {
        texture_flags |= 16;
    }
    if draw.material.normal_texture.is_some() {
        texture_flags |= 32;
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
        .checked_add(stride)
        .ok_or_else(|| anyhow!("GPUI 3D draw slot size overflow"))?;
    bytes.resize(slot_end, 0);

    Ok(())
}
#[cfg(test)]
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
    encode_instance_records(&scene.draws, bytes)
}

pub(super) fn encode_instance_records(
    draws: &[crate::PreparedDraw],
    bytes: &mut Vec<u8>,
) -> gpui::Result<()> {
    let start_len = bytes.len();
    // A streamed section commonly emits many material parts from one node.
    // Matrix inversion is independent of the material part, so compute it only
    // once per node/world transform for each table encode.
    let mut inverses = HashMap::<NodeId, (Mat4, Mat4)>::new();
    for draw in draws {
        let inverse = match inverses.get(&draw.node) {
            Some((world, inverse)) if *world == draw.world => *inverse,
            _ => {
                let inverse = draw
                    .world
                    .inverse()
                    .ok_or_else(|| anyhow!("GPUI 3D draw has a singular world transform"))?;
                inverses.insert(draw.node, (draw.world, inverse));
                inverse
            }
        };
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
    if draws.is_empty() && bytes.len() == start_len {
        bytes.resize(INSTANCE_STRIDE as usize, 0);
    }
    debug_assert_eq!((bytes.len() - start_len) % INSTANCE_STRIDE as usize, 0);
    Ok(())
}

#[cfg(test)]
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
            PreparedLight::Spherical {
                position,
                color,
                intensity,
                range,
                radius,
                diffuse_only,
            } => {
                write_light_header(bytes, if diffuse_only { 4 } else { 5 });
                write_vec4(bytes, [position.x, position.y, position.z, range]);
                write_vec4(bytes, [0.; 4]);
                write_vec4(bytes, [color[0], color[1], color[2], intensity]);
                // Point/spherical records reuse the spot-only cone slot for source radius.
                write_spot_cone_cosines(bytes, radius, 0.);
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
    use super::{
        ArenaPage, MeshArena, can_encode_voxel_mesh, encode_draws, encode_draws_into, encode_mesh,
        encode_voxel_mesh,
    };
    use crate::{Camera, Mesh, Node, PreparedScene, Scene, TriangleEdgeMask, Vec2, Vec3, Vertex};

    #[test]
    fn generated_pool_alignment_tracks_reported_device_limits() {
        let mut limits = gfx_core::BufferBindingLimits {
            storage_offset_alignment: 4,
            max_storage_binding_bytes: 16 * 1024 * 1024,
            uniform_offset_alignment: 256,
            max_uniform_binding_bytes: 64 * 1024,
        };
        let align = super::MeshArena::generated_face_alignment;
        assert_eq!(align(limits).unwrap(), 16);
        limits.storage_offset_alignment = 64;
        assert_eq!(align(limits).unwrap(), 64);
        limits.storage_offset_alignment = 256;
        assert_eq!(align(limits).unwrap(), 256);
        limits.storage_offset_alignment = 384;
        assert_eq!(align(limits).unwrap(), 384);
        limits.storage_offset_alignment = 0;
        assert!(align(limits).is_err());
        limits.storage_offset_alignment = u64::MAX;
        assert!(align(limits).is_err());
    }

    #[test]
    fn dx11_streaming_buffers_avoid_rebinding_again_at_32k_and_64k() {
        assert_eq!(super::dx11_streaming_headroom(gfx_core::BackendKind::Dx11, 8192), 8192);
        assert_eq!(super::dx11_streaming_headroom(gfx_core::BackendKind::Dx11, 16384), 16384);
        assert_eq!(super::dx11_streaming_headroom(gfx_core::BackendKind::Dx11, 16385), 131072);
        assert_eq!(super::dx11_streaming_headroom(gfx_core::BackendKind::Dx11, 65993), 131072);
        assert_eq!(super::dx11_streaming_headroom(gfx_core::BackendKind::Dx11, 131073), 131073);
        assert_eq!(super::dx11_streaming_headroom(gfx_core::BackendKind::Vulkan, 65993), 65993);
        assert_eq!(super::dx11_streaming_headroom(gfx_core::BackendKind::Dx12, 65993), 65993);
    }

    #[test]
    fn material_cache_capacity_tracks_large_scenes_without_unbounded_growth() {
        assert_eq!(super::material_record_cache_limit(0), 8192);
        assert_eq!(super::material_record_cache_limit(8192), 16384);
        assert_eq!(super::material_record_cache_limit(103305), 206610);
        assert_eq!(super::material_record_cache_limit(1_000_000), 262144);
    }

    #[test]
    fn cached_draw_records_match_full_encoder_across_light_counts() {
        let mut scene = Scene::new();
        scene.insert(None, Node::new().with_mesh(std::sync::Arc::new(Mesh::cube()))).unwrap();
        let prepared = PreparedScene::new(
            &scene,
            Camera::perspective(Vec3::new(0., 0., 5.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.),
            1.,
        ).unwrap();
        let repeated = [prepared.draws[0].clone(), prepared.draws[0].clone()];
        let mut cache = super::DrawRecordCache::default();
        let mut bytes = Vec::new();
        let mut keys = Vec::new();
        for light_count in [0, 0, 2, 2, 0] {
            cache.encode(&repeated, light_count, &mut bytes, &mut keys).unwrap();
            assert_eq!(bytes, super::encode_draw_records(&repeated, light_count).unwrap());
            assert_eq!(keys.len(), repeated.len());
        }
        cache.encode(&[], 0, &mut bytes, &mut keys).unwrap();
        assert_eq!(bytes, vec![0; super::DRAW_PARAMS_STRIDE as usize]);
    }

    #[test]
    fn draw_cache_invalidates_cloned_material_with_same_id() {
        let mut scene = Scene::new();
        scene.insert(None, Node::new().with_mesh(std::sync::Arc::new(Mesh::cube()))).unwrap();
        let prepared = PreparedScene::new(
            &scene,
            Camera::perspective(Vec3::new(0., 0., 5.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.),
            1.,
        ).unwrap();
        let initial = prepared.draws[0].clone();
        let mut modified = initial.clone();
        let mut changed_material = (*initial.material).clone();
        changed_material.base_color = [0.1, 0.2, 0.3, 0.4];
        modified.material = std::sync::Arc::new(changed_material);
        assert_eq!(modified.material.id(), initial.material.id());
        let mut cache = super::DrawRecordCache::default();
        let mut bytes = Vec::new();
        let mut keys = Vec::new();
        cache.encode(&[initial.clone()], 0, &mut bytes, &mut keys).unwrap();
        let old_key = keys[0];
        let old_bytes = bytes.clone();
        cache.encode(&[modified.clone()], 0, &mut bytes, &mut keys).unwrap();
        assert_ne!(old_key, keys[0]);
        assert_ne!(old_bytes, bytes);
        assert_eq!(bytes, super::encode_draw_records(&[modified.clone()], 0).unwrap());

        let new_key = keys[0];
        cache.encode(&[modified.clone(), modified.clone()], 0, &mut bytes, &mut keys).unwrap();
        assert_eq!(keys, vec![new_key; 2]);
        assert_eq!(bytes, super::encode_draw_records(&[modified.clone(), modified], 0).unwrap());
        cache.encode(&[initial.clone()], 0, &mut bytes, &mut keys).unwrap();
        assert_ne!(keys[0], old_key, "revisited snapshot must not alias an overwritten cache generation");
        assert_eq!(bytes, super::encode_draw_records(&[initial], 0).unwrap());
    }

    #[test]
    fn full_material_cache_preserves_unchanged_resident_gpu_records() {
        use std::sync::Arc;
        let mut scene = Scene::new();
        scene.insert(None, Node::new().with_mesh(Arc::new(Mesh::cube()))).unwrap();
        let prepared = PreparedScene::new(
            &scene,
            Camera::perspective(Vec3::new(0., 0., 5.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.),
            1.,
        ).unwrap();
        let draw = prepared.draws[0].clone();
        let mut cache = super::DrawRecordCache::default();
        cache.light_count = Some(0);
        // Keep all simulated Section material snapshots alive: they must not
        // be evicted as stale, so the active cache remains at its 8192 limit.
        let mut live = Vec::with_capacity(8_192);
        for _ in 0..8_192 {
            let material = Arc::new(crate::Material::new());
            cache.records.insert(
                (material.id(), false, Arc::as_ptr(&material) as usize),
                super::MaterialRecord {
                    material: Arc::clone(&material),
                    bytes: [0; super::DRAW_PARAMS_STRIDE as usize],
                    generation: 0,
                },
            );
            live.push(material);
        }
        assert_eq!(cache.records.len(), 8_192);
        let mapping = [0u32];
        let mut next_keys = Vec::new();
        let mut next_bytes = Vec::new();
        let mut scratch = Vec::new();
        assert_eq!(
            cache.encode_stable(
                std::slice::from_ref(&draw), 0, &mapping, 1,
                &[], &[], &mut next_keys, &mut next_bytes, &mut scratch,
            ).unwrap(), 1
        );
        let old_keys = next_keys.clone();
        let old_bytes = next_bytes.clone();
        assert_eq!(cache.records.len(), 8_192);
        assert_eq!(
            cache.encode_stable(
                std::slice::from_ref(&draw), 0, &mapping, 1,
                &old_keys, &old_bytes, &mut next_keys, &mut next_bytes, &mut scratch,
            ).unwrap(), 0,
            "a full material cache must not redirty identical resident draw data"
        );
        assert_eq!(next_keys, old_keys);
        assert!(next_bytes.is_empty(), "GPU resident buffer must be reused");
        let mut changed = draw;
        let mut cloned = (*changed.material).clone();
        cloned.base_color = [0.5, 0.2, 0.1, 1.0];
        changed.material = Arc::new(cloned);
        assert_eq!(
            cache.encode_stable(
                &[changed.clone()], 0, &mapping, 1,
                &old_keys, &old_bytes, &mut next_keys, &mut next_bytes, &mut scratch,
            ).unwrap(), 1
        );
        assert_ne!(next_keys, old_keys);
        assert_eq!(next_bytes, super::encode_draw_records(&[changed], 0).unwrap());
        assert_eq!(live.len(), 8_192);
    }

    #[test]
    fn draw_cache_keys_track_light_count_even_for_identical_material_snapshots() {
        let mut scene = Scene::new();
        scene.insert(None, Node::new().with_mesh(std::sync::Arc::new(Mesh::cube()))).unwrap();
        let prepared = PreparedScene::new(
            &scene,
            Camera::perspective(Vec3::new(0., 0., 5.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.),
            1.,
        ).unwrap();
        let draws = [prepared.draws[0].clone()];
        let mut cache = super::DrawRecordCache::default();
        let mut bytes = Vec::new();
        let mut keys = Vec::new();
        cache.encode(&draws, 0, &mut bytes, &mut keys).unwrap();
        let first = keys[0];
        cache.encode(&draws, 0, &mut bytes, &mut keys).unwrap();
        assert_eq!(keys[0], first);
        cache.encode(&draws, 2, &mut bytes, &mut keys).unwrap();
        assert_ne!(keys[0], first);
        assert_eq!(bytes, super::encode_draw_records(&draws, 2).unwrap());
    }

    #[test]
    fn repeated_node_instance_transform_matches_individual_encoding() {
        let mut scene = Scene::new();
        scene.insert(None, Node::new().with_mesh(std::sync::Arc::new(Mesh::cube()))).unwrap();
        let prepared = PreparedScene::new(
            &scene,
            Camera::perspective(Vec3::new(0., 0., 5.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.),
            1.,
        ).unwrap();
        let mut repeated = vec![prepared.draws[0].clone(); 4];
        let mut shifted = repeated[0].clone();
        shifted.world = crate::Transform {
            translation: Vec3::new(8.0, 1.0, -3.0),
            ..crate::Transform::IDENTITY
        }.matrix();
        repeated.push(shifted);
        let mut expected = Vec::new();
        for draw in &repeated {
            let inverse = draw.world.inverse().unwrap();
            super::write_matrix(&mut expected, draw.world.columns());
            super::write_matrix(&mut expected, super::transpose(inverse).columns());
            super::write_vec4(&mut expected, [draw.pixel_offset.x, draw.pixel_offset.y, draw.depth_bias, 0.0]);
        }
        let mut actual = Vec::new();
        super::encode_instance_records(&repeated, &mut actual).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn draw_encoding_reuses_staging_capacity() {
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new().with_mesh(std::sync::Arc::new(Mesh::cube())),
            )
            .unwrap();
        let prepared = PreparedScene::new(
            &scene,
            Camera::perspective(Vec3::new(0., 0., 5.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.),
            1.,
        )
        .unwrap();
        let expected = encode_draws(&prepared).unwrap();
        let mut bytes = Vec::new();
        encode_draws_into(&prepared, &mut bytes).unwrap();
        assert_eq!(bytes, expected);
        let pointer = bytes.as_ptr();
        encode_draws_into(&prepared, &mut bytes).unwrap();
        assert_eq!(bytes.as_ptr(), pointer);
        assert_eq!(bytes, expected);
    }

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
    fn arena_page_reserves_disjoint_ranges_without_reusing_live_space() {
        let mut page = ArenaPage {
            buffer: gfx_core::BufferId::from_parts(1, 1),
            capacity: 96,
            used: 0,
            live: 0,
            live_bytes: 0,
            free_ranges: Vec::new(),
            retired_ranges: Vec::new(),
        };
        assert_eq!(page.reserve(32), Some(0));
        assert_eq!(page.reserve(32), Some(32));
        assert_eq!(page.reserve(64), None);
        assert_eq!(page.reserve(32), Some(64));
        assert_eq!(page.live, 3);
        assert_eq!(page.live_bytes, 96);
    }

    #[test]
    fn arena_stats_distinguish_live_and_retired_ranges() {
        let arena = MeshArena {
            pages: vec![
                Some(ArenaPage {
                    buffer: gfx_core::BufferId::from_parts(1, 1),
                    capacity: 128,
                    used: 96,
                    live: 2,
                    live_bytes: 64,
                    free_ranges: Vec::new(),
                    retired_ranges: vec![(32, 32)],
                }),
                None,
            ],
            ..Default::default()
        };
        assert_eq!(arena.stats(), (1, 128, 64, 32));
    }

    #[test]
    fn arena_index_skips_full_pages_and_waits_for_retirement() {
        let mut arena = MeshArena::default();
        for index in 0..4096 {
            arena.pages.push(Some(ArenaPage {
                buffer: gfx_core::BufferId::from_parts(index as u32 + 1, 1),
                capacity: 72,
                used: 72,
                live: 2,
                live_bytes: 48,
                free_ranges: Vec::new(),
                retired_ranges: vec![(0, 24)],
            }));
        }
        assert!(arena.reserve_page(24).unwrap().is_none());
        arena.reclaim_after_gpu_idle();
        assert_eq!(arena.available_pages.len(), 4096);
        for index in 0..4096 {
            let (allocation, _, offset, _) = arena.reserve_page(24).unwrap().unwrap();
            assert_eq!((allocation.page, offset), (index, 0));
        }
        assert!(arena.available_pages.is_empty());
        assert!(arena.reserve_page(24).unwrap().is_none());
    }

    #[test]
    fn arena_reuses_retired_ranges_only_after_gpu_drain() {
        let mut page = ArenaPage {
            buffer: gfx_core::BufferId::from_parts(1, 1),
            capacity: 96,
            used: 0,
            live: 0,
            live_bytes: 0,
            free_ranges: Vec::new(),
            retired_ranges: Vec::new(),
        };
        assert_eq!(page.reserve(32), Some(0));
        assert_eq!(page.reserve(32), Some(32));
        assert_eq!(page.reserve(32), Some(64));
        page.retire(0, 32).unwrap();
        page.retire(32, 32).unwrap();
        assert_eq!(page.reserve(48), None);
        page.reclaim_retired();
        assert_eq!(page.reserve(48), Some(0));
        assert_eq!(page.reserve(16), Some(48));
        assert_eq!(page.live_bytes, 96);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires a Windows D3D11 hardware adapter"]
    fn native_arena_reuses_a_range_only_after_queue_completion() {
        use gfx_core::{
            CommandDevice, CommandEncoderDescriptor, DeviceDescriptor, SubmissionDevice,
        };
        let mut device = gfx_dx11::Dx11Device::new(&DeviceDescriptor::default()).unwrap();
        let mut arena = MeshArena::default();
        let (first, buffer, first_offset, _) = arena
            .allocate(&mut device, gfx_core::BackendKind::Dx11, 24)
            .unwrap();
        let (second, _, _, _) = arena
            .allocate(&mut device, gfx_core::BackendKind::Dx11, 24)
            .unwrap();
        let encoder = device
            .create_command_encoder(&CommandEncoderDescriptor { label: None })
            .unwrap();
        let submission = device.submit_deferred(encoder).unwrap();
        arena.release(&mut device, first).unwrap();
        let (third, _, before_completion, _) = arena
            .allocate(&mut device, gfx_core::BackendKind::Dx11, 24)
            .unwrap();
        assert_ne!(before_completion, first_offset);
        device.wait_queue_idle().unwrap();
        assert!(matches!(
            device.poll_submission(submission),
            Ok(gfx_core::SubmissionStatus::Complete)
        ));
        arena.reclaim_after_gpu_idle();
        let (fourth, reused_buffer, after_completion, _) = arena
            .allocate(&mut device, gfx_core::BackendKind::Dx11, 24)
            .unwrap();
        assert_eq!((reused_buffer, after_completion), (buffer, first_offset));
        arena.release(&mut device, second).unwrap();
        arena.release(&mut device, third).unwrap();
        arena.release(&mut device, fourth).unwrap();
        assert!(arena.available_pages.is_empty());
        assert_eq!(arena.free_pages, [0]);
        let (next, _, _, _) = arena
            .allocate(&mut device, gfx_core::BackendKind::Dx11, 24)
            .unwrap();
        assert_eq!(next.page, 0);
        assert_eq!(arena.pages.len(), 1);
        arena.release(&mut device, next).unwrap();
        device.destroy_command_encoder(encoder).unwrap();
    }

    #[test]
    fn compact_voxel_encoding_uses_24_byte_stride_and_preserves_indices() {
        let mut source = vertices();
        source[0].uv = Vec2::new(0.25, 0.75);
        source[0].color = [0.0, 0.5, 1.0, 1.0];
        let mesh = Mesh::new(source, [0, 1, 2]).unwrap();
        assert!(can_encode_voxel_mesh(&mesh));
        let encoded = encode_voxel_mesh(&mesh).unwrap();

        assert_eq!(encoded.vertices.len(), mesh.vertices().len() * 24);
        assert_eq!(encoded.index_format, gfx_core::IndexFormat::Uint16);
        assert_eq!(
            encoded.indices,
            [0u16, 1, 2]
                .iter()
                .flat_map(|index| index.to_ne_bytes())
                .collect::<Vec<_>>()
        );
        assert_eq!(float(&encoded.vertices, 0), 0.0);
        assert_eq!(float(&encoded.vertices, 16), 0.25);
        assert_eq!(float(&encoded.vertices, 20), 0.75);
        assert_eq!(
            u32::from_ne_bytes(encoded.vertices[12..16].try_into().unwrap()),
            0xffff8000
        );
    }

    #[test]
    fn compact_cpu_quads_upload_vertices_without_an_index_buffer() {
        let source = Mesh::with_parts_without_raycast(
            vertices(),
            [0, 1, 2, 0, 2, 3],
            [crate::MeshPart::new(0, 6, 0)],
        )
        .unwrap();
        let expected = encode_voxel_mesh(&source).unwrap();
        let compact = source.compact_voxel();
        let encoded = encode_voxel_mesh(&compact).unwrap();

        assert!(encoded.indices.is_empty());
        assert_eq!(compact.index_count(), expected.indices.len() / 2);
        for (actual, expected) in encoded
            .vertices
            .chunks_exact(24)
            .zip(expected.vertices.chunks_exact(24))
        {
            assert_eq!(actual, expected);
        }
        assert_eq!(
            compact.geometry_storage_bytes(),
            compact.vertex_count() * 27
        );
    }

    #[test]
    fn compact_voxel_encoding_excludes_out_of_range_colors_and_edge_masks() {
        let mut source = vertices();
        source[0].color[0] = 1.5;
        let mesh = Mesh::new(source, [0, 1, 2]).unwrap();
        assert!(!can_encode_voxel_mesh(&mesh));
        let masked = Mesh::new(vertices(), [0, 1, 2])
            .unwrap()
            .with_edge_masks([TriangleEdgeMask::ALL])
            .unwrap();
        assert!(!can_encode_voxel_mesh(&masked));
    }

    #[test]
    fn mesh_without_edge_masks_keeps_shared_vertices_and_indices() {
        let mesh = Mesh::new(vertices(), vec![0, 1, 2, 0, 2, 3]).unwrap();
        let encoded = encode_mesh(&mesh).unwrap();

        assert_eq!(encoded.vertices.len(), 4 * 64);
        assert_eq!(encoded.index_format, gfx_core::IndexFormat::Uint16);
        assert_eq!(encoded.indices.len(), 6 * 2);
        assert!(encoded.tangents.is_none());
        let indices = encoded
            .indices
            .chunks_exact(2)
            .map(|bytes| u16::from_ne_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(indices, [0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn mesh_with_more_than_u16_vertices_keeps_u32_indices() {
        let mesh = Mesh::new(
            vec![vertices()[0]; u16::MAX as usize + 2],
            vec![u16::MAX as u32 - 1, u16::MAX as u32, u16::MAX as u32 + 1],
        )
        .unwrap();
        let encoded = encode_mesh(&mesh).unwrap();
        assert_eq!(encoded.index_format, gfx_core::IndexFormat::Uint32);
        assert_eq!(encoded.indices.len(), 3 * 4);
        let indices = encoded
            .indices
            .chunks_exact(4)
            .map(|bytes| u32::from_ne_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(indices, [65534, 65535, 65536]);
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
        assert_eq!(encoded.index_format, gfx_core::IndexFormat::Uint16);
        let indices = encoded
            .indices
            .chunks_exact(2)
            .map(|bytes| u16::from_ne_bytes(bytes.try_into().unwrap()))
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

#[cfg(test)]
mod voxel_clip_fast_path_tests {
    // CPU differential of the algebra used by voxel.wgsl. The native headless
    // renderer tests additionally compare the production shader's actual pixels.
    fn old_projection(
        clip: [f32; 4], origin: [f32; 2], bounds: [f32; 2],
        target: [f32; 2], offset: [f32; 2], depth_bias: f32,
    ) -> [f32; 4] {
        let w = clip[3];
        let safe_w = if w >= 0.0 { w.max(0.0001) } else { w.min(-0.0001) };
        let mut result = [0.0; 4];
        for axis in 0..2 {
            let normalized = (origin[axis]
                + (clip[axis] / safe_w * if axis == 0 { 0.5 } else { -0.5 } + 0.5)
                    * bounds[axis] + offset[axis]) / target[axis].max(1.0);
            result[axis] = (normalized * if axis == 0 { 2.0 } else { -2.0 }
                + if axis == 0 { -1.0 } else { 1.0 }) * w;
        }
        result[2] = (clip[2] / safe_w - depth_bias) * w;
        result[3] = w;
        result
    }

    fn new_projection(
        clip: [f32; 4], origin: [f32; 2], bounds: [f32; 2],
        target: [f32; 2], offset: [f32; 2], depth_bias: f32,
    ) -> [f32; 4] {
        if clip[3].abs() < 0.0001 {
            return old_projection(clip, origin, bounds, target, offset, depth_bias);
        }
        let mut result = [0.0; 4];
        for axis in 0..2 {
            let scale = bounds[axis] / target[axis].max(1.0);
            let center = origin[axis] + bounds[axis] * 0.5 + offset[axis];
            let bias = center / target[axis].max(1.0)
                * if axis == 0 { 2.0 } else { -2.0 }
                + if axis == 0 { -1.0 } else { 1.0 };
            result[axis] = clip[axis] * scale + bias * clip[3];
        }
        result[2] = clip[2] - depth_bias * clip[3];
        result[3] = clip[3];
        result
    }

    #[test]
    fn clip_space_viewport_fold_matches_original_for_camera_and_pixel_offsets() {
        for clip in [
            [0.2, -0.4, 0.5, 1.0],
            [-4.0, 2.0, 11.0, 20.0],
            [1.2, -0.2, -1.0, -3.0],
            [0.5, -0.3, 0.2, 0.000001],
            [0.5, -0.3, 0.2, -0.000001],
            [0.0, 0.0, 0.0, 0.0],
        ] {
            for (origin, bounds, target, offset, bias) in [
                ([0.0, 0.0], [1920.0, 1080.0], [1920.0, 1080.0], [0.0, 0.0], 0.0),
                ([37.0, 28.0], [500.0, 320.0], [1920.0, 1080.0], [0.3, -2.0], 0.002),
                ([0.0, 80.0], [180.0, 240.0], [1.0, 800.0], [15.0, -1.0], -0.15),
            ] {
                let original = old_projection(clip, origin, bounds, target, offset, bias);
                let optimized = new_projection(clip, origin, bounds, target, offset, bias);
                for axis in 0..4 {
                    let tolerance = 0.0002 * original[axis].abs().max(1.0);
                    assert!((original[axis] - optimized[axis]).abs() <= tolerance,
                        "clip={clip:?} axis={axis} old={} new={}",
                        original[axis], optimized[axis]);
                }
            }
        }
    }
}
