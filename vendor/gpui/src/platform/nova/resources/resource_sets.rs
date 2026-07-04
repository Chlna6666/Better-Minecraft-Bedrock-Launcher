use anyhow::Result;

use super::super::*;
use super::buffers::NovaResourceBuffers;

pub(super) struct NovaRendererResourceSets {
    pub(super) quad_resource_set: ResourceSetId,
    pub(super) shadow_resource_set: ResourceSetId,
    pub(super) path_rasterization_resource_set: ResourceSetId,
    pub(super) underline_resource_set: ResourceSetId,
    pub(super) custom_mesh_3d_resource_set: ResourceSetId,
}

pub(super) fn create_renderer_resource_sets<D>(
    device: &mut D,
    label: &str,
    layouts: &NovaResourceLayouts,
    buffers: &NovaResourceBuffers,
) -> Result<NovaRendererResourceSets>
where
    D: BackendResources,
{
    let quad_resource_set = device.create_resource_set(&ResourceSetDescriptor {
        label: Some(format!("{label} quad resource set")),
        layout: layouts.quad_resource_set_layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: buffers.global_buffer,
                    offset: 0,
                    size: GLOBAL_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 1,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: buffers.quad_buffer,
                    offset: 0,
                    size: (MAX_QUADS * PACKED_QUAD_BYTES) as u64,
                    stride: Some(PACKED_QUAD_BYTES as u32),
                }),
            },
        ],
    })?;
    let shadow_resource_set = device.create_resource_set(&ResourceSetDescriptor {
        label: Some(format!("{label} shadow resource set")),
        layout: layouts.shadow_resource_set_layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: buffers.global_buffer,
                    offset: 0,
                    size: GLOBAL_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 2,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: buffers.shadow_buffer,
                    offset: 0,
                    size: (MAX_SHADOWS * PACKED_SHADOW_BYTES) as u64,
                    stride: Some(PACKED_SHADOW_BYTES as u32),
                }),
            },
        ],
    })?;
    let path_rasterization_resource_set = device.create_resource_set(&ResourceSetDescriptor {
        label: Some(format!("{label} path rasterization resource set")),
        layout: layouts.path_rasterization_resource_set_layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: buffers.global_buffer,
                    offset: 0,
                    size: GLOBAL_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 3,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: buffers.path_rasterization_vertex_buffer,
                    offset: 0,
                    size: (MAX_PATH_VERTICES * PACKED_PATH_RASTERIZATION_VERTEX_BYTES) as u64,
                    stride: Some(PACKED_PATH_RASTERIZATION_VERTEX_BYTES as u32),
                }),
            },
        ],
    })?;
    let underline_resource_set = device.create_resource_set(&ResourceSetDescriptor {
        label: Some(format!("{label} underline resource set")),
        layout: layouts.underline_resource_set_layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: buffers.global_buffer,
                    offset: 0,
                    size: GLOBAL_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 7,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: buffers.underline_buffer,
                    offset: 0,
                    size: (MAX_UNDERLINES * PACKED_UNDERLINE_BYTES) as u64,
                    stride: Some(PACKED_UNDERLINE_BYTES as u32),
                }),
            },
        ],
    })?;
    let custom_mesh_3d_resource_set = device.create_resource_set(&ResourceSetDescriptor {
        label: Some(format!("{label} custom GPU mesh 3D resource set")),
        layout: layouts.custom_mesh_3d_resource_set_layout,
        bindings: custom_mesh_3d_resource_bindings(
            buffers.global_buffer,
            buffers.custom_mesh_3d_parameters_buffer,
            buffers.custom_mesh_3d_vertices_buffer,
        ),
    })?;

    Ok(NovaRendererResourceSets {
        quad_resource_set,
        shadow_resource_set,
        path_rasterization_resource_set,
        underline_resource_set,
        custom_mesh_3d_resource_set,
    })
}
