use anyhow::Result;

use super::super::*;

#[derive(Clone, Copy)]
pub(in crate::platform::nova) struct NovaFrameResourceBuffers {
    pub(in crate::platform::nova) global_buffer: BufferId,
    pub(in crate::platform::nova) text_raster_buffer: BufferId,
    pub(in crate::platform::nova) quad_buffer: BufferId,
    pub(in crate::platform::nova) shadow_buffer: BufferId,
    pub(in crate::platform::nova) path_rasterization_vertex_buffer: BufferId,
    pub(in crate::platform::nova) path_sprite_buffer: BufferId,
    pub(in crate::platform::nova) mono_sprite_buffer: BufferId,
    pub(in crate::platform::nova) poly_sprite_buffer: BufferId,
    pub(in crate::platform::nova) present_copy_sprite_buffer: BufferId,
    pub(in crate::platform::nova) underline_buffer: BufferId,
    pub(in crate::platform::nova) backdrop_blur_pass_buffer: BufferId,
    pub(in crate::platform::nova) backdrop_blur_buffer: BufferId,
    pub(in crate::platform::nova) animation_binding_buffer: BufferId,
    pub(in crate::platform::nova) animation_value_buffer: BufferId,
    pub(in crate::platform::nova) custom_mesh_3d_parameters_buffer: BufferId,
}

#[derive(Clone, Copy)]
pub(super) struct NovaSharedResourceBuffers {
    pub(super) custom_mesh_3d_vertices_buffer: BufferId,
    pub(super) custom_mesh_3d_indices_buffer: BufferId,
    pub(super) atlas_sampler: SamplerId,
}

pub(super) struct NovaResourceBuffers {
    pub(super) frame_buffers: Vec<NovaFrameResourceBuffers>,
    pub(super) shared: NovaSharedResourceBuffers,
}

pub(super) fn create_resource_buffers<D>(device: &mut D, label: &str) -> Result<NovaResourceBuffers>
where
    D: BackendResources,
{
    let mut frame_buffers = Vec::with_capacity(MAX_IN_FLIGHT_SUBMISSIONS);
    for index in 0..MAX_IN_FLIGHT_SUBMISSIONS {
        frame_buffers.push(create_frame_resource_buffers(
            device,
            &format!("{label} frame {index}"),
        )?);
    }

    let custom_mesh_3d_vertices_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} custom GPU mesh 3D vertices")),
        size: (MAX_CUSTOM_MESH_3D_VERTICES * PACKED_CUSTOM_MESH_3D_VERTEX_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let custom_mesh_3d_indices_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} custom GPU mesh 3D indices")),
        size: (MAX_CUSTOM_MESH_3D_INDICES * PACKED_CUSTOM_MESH_3D_INDEX_BYTES) as u64,
        usage: BufferUsage::INDEX | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let atlas_sampler = device.create_sampler(&SamplerDescriptor {
        label: Some(format!("{label} glyph atlas sampler")),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
    })?;

    Ok(NovaResourceBuffers {
        frame_buffers,
        shared: NovaSharedResourceBuffers {
            custom_mesh_3d_vertices_buffer,
            custom_mesh_3d_indices_buffer,
            atlas_sampler,
        },
    })
}

fn create_frame_resource_buffers<D>(device: &mut D, label: &str) -> Result<NovaFrameResourceBuffers>
where
    D: BackendResources,
{
    let global_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} globals")),
        size: GLOBAL_UPLOAD_BYTES as u64,
        usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let text_raster_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} text raster parameters")),
        size: TEXT_RASTER_UPLOAD_BYTES as u64,
        usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let quad_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} quads")),
        size: (MAX_QUADS * PACKED_QUAD_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let shadow_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} shadows")),
        size: (MAX_SHADOWS * PACKED_SHADOW_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let path_rasterization_vertex_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} path rasterization vertices")),
        size: (MAX_PATH_VERTICES * PACKED_PATH_RASTERIZATION_VERTEX_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let path_sprite_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} path sprites")),
        size: (MAX_PATH_SPRITES * PACKED_PATH_SPRITE_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let mono_sprite_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} mono sprites")),
        size: (MAX_MONO_SPRITES * PACKED_MONO_SPRITE_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let poly_sprite_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} poly sprites")),
        size: (MAX_POLY_SPRITES * PACKED_POLY_SPRITE_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let present_copy_sprite_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} present copy sprite")),
        size: PACKED_POLY_SPRITE_BYTES as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let underline_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} underlines")),
        size: (MAX_UNDERLINES * PACKED_UNDERLINE_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let backdrop_blur_pass_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} backdrop blur pass")),
        size: BACKDROP_BLUR_PASS_BYTES as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let backdrop_blur_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} backdrop blurs")),
        size: (MAX_BACKDROP_BLURS * PACKED_BACKDROP_BLUR_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let animation_binding_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} animation bindings")),
        size: (MAX_ANIMATION_BINDINGS * PACKED_ANIMATION_BINDING_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let animation_value_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} animation values")),
        size: (MAX_ANIMATION_VALUES * PACKED_ANIMATION_VALUE_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let custom_mesh_3d_parameters_buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} custom GPU mesh 3D params")),
        size: (MAX_CUSTOM_MESH_3D_DRAWS * PACKED_CUSTOM_MESH_3D_PARAMETERS_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;

    Ok(NovaFrameResourceBuffers {
        global_buffer,
        text_raster_buffer,
        quad_buffer,
        shadow_buffer,
        path_rasterization_vertex_buffer,
        path_sprite_buffer,
        mono_sprite_buffer,
        poly_sprite_buffer,
        present_copy_sprite_buffer,
        underline_buffer,
        backdrop_blur_pass_buffer,
        backdrop_blur_buffer,
        animation_binding_buffer,
        animation_value_buffer,
        custom_mesh_3d_parameters_buffer,
    })
}
