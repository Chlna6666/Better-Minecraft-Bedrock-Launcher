use anyhow::Result;

use super::super::*;
use super::buffers::FrameResourceBuffers;

#[derive(Clone, Copy)]
pub(in crate::platform::nova) struct FrameResourceSets {
    pub(in crate::platform::nova) quad_resource_set: ResourceSetId,
    pub(in crate::platform::nova) shadow_resource_set: ResourceSetId,
    pub(in crate::platform::nova) path_rasterization_resource_set: ResourceSetId,
    pub(in crate::platform::nova) underline_resource_set: ResourceSetId,
}

pub(in crate::platform::nova) fn create_path_rasterization_resource_set<D>(
    device: &mut D,
    label: &str,
    layout: ResourceSetLayoutId,
    buffers: &FrameResourceBuffers,
) -> Result<ResourceSetId>
where
    D: BackendResources,
{
    Ok(device.create_resource_set(&ResourceSetDescriptor {
        label: Some(format!("{label} path rasterization resource set")),
        layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: buffers.global_buffer,
                    offset: 0,
                    size: GLOBAL_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 3,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: buffers.path_rasterization_vertex_buffer,
                    offset: 0,
                    size: (buffers.path_rasterization_vertex_capacity
                        * PACKED_PATH_RASTERIZATION_VERTEX_BYTES) as u64,
                    stride: Some(PACKED_PATH_RASTERIZATION_VERTEX_BYTES as u32),
                }),
            },
        ],
    })?)
}

fn animation_value_binding(buffers: &FrameResourceBuffers) -> ResourceBinding {
    ResourceBinding {
        binding: 17,
        resource: BindingResource::Buffer(BufferBinding {
            buffer: buffers.animation_value_buffer,
            offset: 0,
            size: (MAX_ANIMATION_VALUES * PACKED_ANIMATION_VALUE_BYTES) as u64,
            stride: Some(PACKED_ANIMATION_VALUE_BYTES as u32),
        }),
    }
}

pub(in crate::platform::nova) fn create_quad_resource_set<D>(
    device: &mut D,
    label: &str,
    layout: ResourceSetLayoutId,
    buffers: &FrameResourceBuffers,
) -> Result<ResourceSetId>
where
    D: BackendResources,
{
    Ok(device.create_resource_set(&ResourceSetDescriptor {
        label: Some(format!("{label} quad resource set")),
        layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: buffers.global_buffer,
                    offset: 0,
                    size: GLOBAL_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 1,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: buffers.quad_buffer,
                    offset: 0,
                    size: (buffers.quad_capacity * PACKED_QUAD_BYTES) as u64,
                    stride: Some(PACKED_QUAD_BYTES as u32),
                }),
            },
            animation_value_binding(buffers),
        ],
    })?)
}

pub(super) fn create_renderer_resource_sets<D>(
    device: &mut D,
    label: &str,
    layouts: &ResourceLayouts,
    buffers: &FrameResourceBuffers,
) -> Result<FrameResourceSets>
where
    D: BackendResources,
{
    let quad_resource_set = create_quad_resource_set(
        device,
        label,
        layouts.quad_resource_set_layout,
        buffers,
    )?;
    let shadow_resource_set = device.create_resource_set(&ResourceSetDescriptor {
        label: Some(format!("{label} shadow resource set")),
        layout: layouts.shadow_resource_set_layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: buffers.global_buffer,
                    offset: 0,
                    size: GLOBAL_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 2,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: buffers.shadow_buffer,
                    offset: 0,
                    size: (MAX_SHADOWS * PACKED_SHADOW_BYTES) as u64,
                    stride: Some(PACKED_SHADOW_BYTES as u32),
                }),
            },
            animation_value_binding(buffers),
        ],
    })?;
    let path_rasterization_resource_set = create_path_rasterization_resource_set(
        device,
        label,
        layouts.path_rasterization_resource_set_layout,
        buffers,
    )?;
    let underline_resource_set = device.create_resource_set(&ResourceSetDescriptor {
        label: Some(format!("{label} underline resource set")),
        layout: layouts.underline_resource_set_layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: buffers.global_buffer,
                    offset: 0,
                    size: GLOBAL_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 7,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: buffers.underline_buffer,
                    offset: 0,
                    size: (MAX_UNDERLINES * PACKED_UNDERLINE_BYTES) as u64,
                    stride: Some(PACKED_UNDERLINE_BYTES as u32),
                }),
            },
            animation_value_binding(buffers),
        ],
    })?;
    Ok(FrameResourceSets {
        quad_resource_set,
        shadow_resource_set,
        path_rasterization_resource_set,
        underline_resource_set,
    })
}
