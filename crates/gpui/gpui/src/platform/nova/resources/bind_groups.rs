use anyhow::Result;

use super::super::*;
use super::buffers::FrameResourceBuffers;

#[derive(Clone, Copy)]
pub(in crate::platform::nova) struct FrameBindGroups {
    pub(in crate::platform::nova) quad_bind_group: BindGroupId,
    pub(in crate::platform::nova) shadow_bind_group: BindGroupId,
    pub(in crate::platform::nova) path_rasterization_bind_group: BindGroupId,
    pub(in crate::platform::nova) underline_bind_group: BindGroupId,
}

pub(in crate::platform::nova) fn create_path_rasterization_bind_group<D>(
    device: &mut D,
    label: &str,
    layout: BindGroupLayoutId,
    buffers: &FrameResourceBuffers,
) -> Result<BindGroupId>
where
    D: ResourceDevice,
{
    Ok(device.create_bind_group(&BindGroupDescriptor {
        label: Some(format!("{label} path rasterization bind group")),
        layout,
        entries: path_rasterization_resource_bindings(buffers),
    })?)
}

pub(in crate::platform::nova) fn path_rasterization_resource_bindings(
    buffers: &FrameResourceBuffers,
) -> Vec<BindGroupEntry> {
    vec![
        BindGroupEntry {
            binding: 0,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: buffers.global_buffer,
                offset: 0,
                size: GLOBAL_UPLOAD_BYTES as u64,
                stride: None,
            }),
        },
        BindGroupEntry {
            binding: 3,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: buffers.path_rasterization_vertex_buffer,
                offset: 0,
                size: (buffers.path_rasterization_vertex_capacity
                    * PACKED_PATH_RASTERIZATION_VERTEX_BYTES) as u64,
                stride: Some(PACKED_PATH_RASTERIZATION_VERTEX_BYTES as u32),
            }),
        },
    ]
}

fn animation_value_binding(buffers: &FrameResourceBuffers) -> BindGroupEntry {
    BindGroupEntry {
        binding: 17,
        resource: BindingResource::Buffer(BufferBinding {
            buffer: buffers.animation_value_buffer,
            offset: 0,
            size: (buffers.animation_value_capacity * PACKED_ANIMATION_VALUE_BYTES) as u64,
            stride: Some(PACKED_ANIMATION_VALUE_BYTES as u32),
        }),
    }
}

pub(in crate::platform::nova) fn create_quad_bind_group<D>(
    device: &mut D,
    label: &str,
    layout: BindGroupLayoutId,
    buffers: &FrameResourceBuffers,
) -> Result<BindGroupId>
where
    D: ResourceDevice,
{
    Ok(device.create_bind_group(&BindGroupDescriptor {
        label: Some(format!("{label} quad bind group")),
        layout,
        entries: quad_resource_bindings(buffers),
    })?)
}

pub(super) fn create_renderer_bind_groups<D>(
    device: &mut D,
    label: &str,
    layouts: &ResourceLayouts,
    buffers: &FrameResourceBuffers,
) -> Result<FrameBindGroups>
where
    D: ResourceDevice,
{
    let quad_bind_group =
        create_quad_bind_group(device, label, layouts.quad_bind_group_layout, buffers)?;
    let shadow_bind_group = device.create_bind_group(&BindGroupDescriptor {
        label: Some(format!("{label} shadow bind group")),
        layout: layouts.shadow_bind_group_layout,
        entries: shadow_resource_bindings(buffers),
    })?;
    let path_rasterization_bind_group = create_path_rasterization_bind_group(
        device,
        label,
        layouts.path_rasterization_bind_group_layout,
        buffers,
    )?;
    let underline_bind_group = device.create_bind_group(&BindGroupDescriptor {
        label: Some(format!("{label} underline bind group")),
        layout: layouts.underline_bind_group_layout,
        entries: underline_resource_bindings(buffers),
    })?;
    Ok(FrameBindGroups {
        quad_bind_group,
        shadow_bind_group,
        path_rasterization_bind_group,
        underline_bind_group,
    })
}

pub(in crate::platform::nova) fn quad_resource_bindings(
    buffers: &FrameResourceBuffers,
) -> Vec<BindGroupEntry> {
    vec![
        BindGroupEntry {
            binding: 0,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: buffers.global_buffer,
                offset: 0,
                size: GLOBAL_UPLOAD_BYTES as u64,
                stride: None,
            }),
        },
        BindGroupEntry {
            binding: 1,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: buffers.quad_buffer,
                offset: 0,
                size: (buffers.quad_capacity * PACKED_QUAD_BYTES) as u64,
                stride: Some(PACKED_QUAD_BYTES as u32),
            }),
        },
        animation_value_binding(buffers),
    ]
}

pub(in crate::platform::nova) fn shadow_resource_bindings(
    buffers: &FrameResourceBuffers,
) -> Vec<BindGroupEntry> {
    vec![
        BindGroupEntry {
            binding: 0,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: buffers.global_buffer,
                offset: 0,
                size: GLOBAL_UPLOAD_BYTES as u64,
                stride: None,
            }),
        },
        BindGroupEntry {
            binding: 2,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: buffers.shadow_buffer,
                offset: 0,
                size: (buffers.shadow_capacity * PACKED_SHADOW_BYTES) as u64,
                stride: Some(PACKED_SHADOW_BYTES as u32),
            }),
        },
        animation_value_binding(buffers),
    ]
}

pub(in crate::platform::nova) fn underline_resource_bindings(
    buffers: &FrameResourceBuffers,
) -> Vec<BindGroupEntry> {
    vec![
        BindGroupEntry {
            binding: 0,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: buffers.global_buffer,
                offset: 0,
                size: GLOBAL_UPLOAD_BYTES as u64,
                stride: None,
            }),
        },
        BindGroupEntry {
            binding: 7,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: buffers.underline_buffer,
                offset: 0,
                size: (MAX_UNDERLINES * PACKED_UNDERLINE_BYTES) as u64,
                stride: Some(PACKED_UNDERLINE_BYTES as u32),
            }),
        },
        animation_value_binding(buffers),
    ]
}
