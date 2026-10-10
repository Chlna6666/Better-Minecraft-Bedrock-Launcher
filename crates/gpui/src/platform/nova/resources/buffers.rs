use anyhow::Result;

use super::super::*;

mod streams;
pub(in crate::platform::nova) use streams::BufferStream;

#[derive(Clone, Copy)]
pub(in crate::platform::nova) struct FrameResourceBuffers {
    pub(in crate::platform::nova) global_buffer: BufferId,
    pub(in crate::platform::nova) text_raster_buffer: BufferId,
    pub(in crate::platform::nova) quad_buffer: BufferId,
    pub(in crate::platform::nova) quad_capacity: usize,
    pub(in crate::platform::nova) shadow_buffer: BufferId,
    pub(in crate::platform::nova) shadow_capacity: usize,
    pub(in crate::platform::nova) path_rasterization_vertex_buffer: BufferId,
    pub(in crate::platform::nova) path_rasterization_vertex_capacity: usize,
    pub(in crate::platform::nova) path_sprite_buffer: BufferId,
    pub(in crate::platform::nova) path_sprite_capacity: usize,
    pub(in crate::platform::nova) mono_sprite_buffer: BufferId,
    pub(in crate::platform::nova) mono_sprite_capacity: usize,
    pub(in crate::platform::nova) poly_sprite_buffer: BufferId,
    pub(in crate::platform::nova) poly_sprite_capacity: usize,
    pub(in crate::platform::nova) underline_buffer: BufferId,
    pub(in crate::platform::nova) backdrop_blur_pass_buffer: BufferId,
    pub(in crate::platform::nova) backdrop_blur_buffer: BufferId,
    pub(in crate::platform::nova) animation_value_buffer: BufferId,
    pub(in crate::platform::nova) animation_value_capacity: usize,
}

#[derive(Clone, Copy)]
pub(super) struct SharedResourceBuffers {
    pub(super) atlas_sampler: SamplerId,
}

pub(super) struct ResourceBuffers {
    pub(super) frame_buffers: Vec<FrameResourceBuffers>,
    pub(super) shared: SharedResourceBuffers,
}

pub(super) fn create_resource_buffers<D>(device: &mut D, label: &str) -> Result<ResourceBuffers>
where
    D: BackendResources,
{
    let mut frame_buffers = Vec::with_capacity(MAX_IN_FLIGHT_SUBMISSIONS);
    for index in 0..MAX_IN_FLIGHT_SUBMISSIONS {
        frame_buffers.push(create_frame_resource_buffers(
            device,
            &format!("{label} frame {index}"),
            frame_buffers.first().copied(),
        )?);
    }

    let atlas_sampler = device.create_sampler(&SamplerDescriptor {
        label: Some(format!("{label} glyph atlas sampler")),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        mipmap_filter: FilterMode::Nearest,
        anisotropic: false,
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
    })?;

    Ok(ResourceBuffers {
        frame_buffers,
        shared: SharedResourceBuffers { atlas_sampler },
    })
}

fn create_frame_resource_buffers<D>(
    device: &mut D,
    label: &str,
    shared: Option<FrameResourceBuffers>,
) -> Result<FrameResourceBuffers>
where
    D: BackendResources,
{
    let global_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} globals")),
        size: GLOBAL_UPLOAD_BYTES as u64,
        usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let text_raster_buffer = match shared {
        Some(buffers) => buffers.text_raster_buffer,
        None => device.create_buffer(&BufferDescriptor {
            label: Some(format!("{label} text raster parameters")),
            size: TEXT_RASTER_UPLOAD_BYTES as u64,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        })?,
    };
    let quad_buffer = match shared {
        Some(buffers) => buffers.quad_buffer,
        None => device.create_buffer(&BufferDescriptor {
            label: Some(format!("{label} quads")),
            size: (INITIAL_QUADS * PACKED_QUAD_BYTES) as u64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        })?,
    };
    let shadow_buffer = match shared {
        Some(buffers) => buffers.shadow_buffer,
        None => device.create_buffer(&BufferDescriptor {
            label: Some(format!("{label} shadows")),
            size: (INITIAL_SHADOWS * PACKED_SHADOW_BYTES) as u64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        })?,
    };
    let path_rasterization_vertex_buffer = match shared {
        Some(buffers) => buffers.path_rasterization_vertex_buffer,
        None => device.create_buffer(&BufferDescriptor {
            label: Some(format!("{label} path rasterization vertices")),
            size: (INITIAL_PATH_VERTICES * PACKED_PATH_RASTERIZATION_VERTEX_BYTES) as u64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        })?,
    };
    let path_sprite_buffer = match shared {
        Some(buffers) => buffers.path_sprite_buffer,
        None => device.create_buffer(&BufferDescriptor {
            label: Some(format!("{label} path sprites")),
            size: (INITIAL_PATH_SPRITES * PACKED_PATH_SPRITE_BYTES) as u64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        })?,
    };
    let mono_sprite_buffer = match shared {
        Some(buffers) => buffers.mono_sprite_buffer,
        None => device.create_buffer(&BufferDescriptor {
            label: Some(format!("{label} mono sprites")),
            size: (INITIAL_MONO_SPRITES * PACKED_MONO_SPRITE_BYTES) as u64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        })?,
    };
    let poly_sprite_buffer = match shared {
        Some(buffers) => buffers.poly_sprite_buffer,
        None => device.create_buffer(&BufferDescriptor {
            label: Some(format!("{label} poly sprites")),
            size: (INITIAL_POLY_SPRITES * PACKED_POLY_SPRITE_BYTES) as u64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        })?,
    };
    let underline_buffer = match shared {
        Some(buffers) => buffers.underline_buffer,
        None => device.create_buffer(&BufferDescriptor {
            label: Some(format!("{label} underlines")),
            size: (MAX_UNDERLINES * PACKED_UNDERLINE_BYTES) as u64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        })?,
    };
    let backdrop_blur_pass_buffer = match shared {
        Some(buffers) => buffers.backdrop_blur_pass_buffer,
        None => device.create_buffer(&BufferDescriptor {
            label: Some(format!("{label} backdrop blur passes")),
            size: (MAX_BACKDROP_BLURS * 2 * BACKDROP_BLUR_PASS_BYTES) as u64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        })?,
    };
    let backdrop_blur_buffer = match shared {
        Some(buffers) => buffers.backdrop_blur_buffer,
        None => device.create_buffer(&BufferDescriptor {
            label: Some(format!("{label} backdrop blurs")),
            size: (MAX_BACKDROP_BLURS * PACKED_BACKDROP_BLUR_BYTES) as u64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        })?,
    };
    let animation_value_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} animation values")),
        size: (INITIAL_ANIMATION_VALUES * PACKED_ANIMATION_VALUE_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    Ok(FrameResourceBuffers {
        global_buffer,
        text_raster_buffer,
        quad_buffer,
        quad_capacity: INITIAL_QUADS,
        shadow_buffer,
        shadow_capacity: INITIAL_SHADOWS,
        path_rasterization_vertex_buffer,
        path_rasterization_vertex_capacity: INITIAL_PATH_VERTICES,
        path_sprite_buffer,
        path_sprite_capacity: INITIAL_PATH_SPRITES,
        mono_sprite_buffer,
        mono_sprite_capacity: INITIAL_MONO_SPRITES,
        poly_sprite_buffer,
        poly_sprite_capacity: INITIAL_POLY_SPRITES,
        underline_buffer,
        backdrop_blur_pass_buffer,
        backdrop_blur_buffer,
        animation_value_buffer,
        animation_value_capacity: INITIAL_ANIMATION_VALUES,
    })
}
