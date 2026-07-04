use super::*;

#[derive(Clone, Copy)]
pub(super) struct NovaAtlasResourceDescriptor {
    pub(super) mono_sprite_resource_set_layout: ResourceSetLayoutId,
    pub(super) poly_sprite_resource_set_layout: ResourceSetLayoutId,
    pub(super) global_buffer: BufferId,
    pub(super) text_raster_buffer: BufferId,
    pub(super) mono_sprite_buffer: BufferId,
    pub(super) poly_sprite_buffer: BufferId,
    pub(super) sampler: SamplerId,
}

pub(super) fn path_resource_bindings(
    global_buffer: BufferId,
    path_texture_view: TextureViewId,
    sampler: SamplerId,
    path_sprite_buffer: BufferId,
) -> Vec<ResourceBinding> {
    vec![
        ResourceBinding {
            binding: 0,
            resource: ResourceBindingResource::Buffer(BufferBinding {
                buffer: global_buffer,
                offset: 0,
                size: GLOBAL_UPLOAD_BYTES as u64,
                stride: None,
            }),
        },
        ResourceBinding {
            binding: 4,
            resource: ResourceBindingResource::Texture(TextureBinding {
                texture_view: path_texture_view,
            }),
        },
        ResourceBinding {
            binding: 5,
            resource: ResourceBindingResource::Sampler(SamplerBinding { sampler }),
        },
        ResourceBinding {
            binding: 6,
            resource: ResourceBindingResource::Buffer(BufferBinding {
                buffer: path_sprite_buffer,
                offset: 0,
                size: (MAX_PATH_SPRITES * PACKED_PATH_SPRITE_BYTES) as u64,
                stride: Some(PACKED_PATH_SPRITE_BYTES as u32),
            }),
        },
    ]
}

pub(super) fn present_cache_resource_bindings(
    global_buffer: BufferId,
    texture_view: TextureViewId,
    sampler: SamplerId,
    sprite_buffer: BufferId,
) -> Vec<ResourceBinding> {
    vec![
        ResourceBinding {
            binding: 0,
            resource: ResourceBindingResource::Buffer(BufferBinding {
                buffer: global_buffer,
                offset: 0,
                size: GLOBAL_UPLOAD_BYTES as u64,
                stride: None,
            }),
        },
        ResourceBinding {
            binding: 4,
            resource: ResourceBindingResource::Texture(TextureBinding { texture_view }),
        },
        ResourceBinding {
            binding: 5,
            resource: ResourceBindingResource::Sampler(SamplerBinding { sampler }),
        },
        ResourceBinding {
            binding: 9,
            resource: ResourceBindingResource::Buffer(BufferBinding {
                buffer: sprite_buffer,
                offset: 0,
                size: PACKED_POLY_SPRITE_BYTES as u64,
                stride: Some(PACKED_POLY_SPRITE_BYTES as u32),
            }),
        },
    ]
}

pub(super) fn backdrop_blur_pass_resource_bindings(
    source_texture_view: TextureViewId,
    sampler: SamplerId,
    pass_buffer: BufferId,
) -> Vec<ResourceBinding> {
    vec![
        ResourceBinding {
            binding: 4,
            resource: ResourceBindingResource::Texture(TextureBinding {
                texture_view: source_texture_view,
            }),
        },
        ResourceBinding {
            binding: 5,
            resource: ResourceBindingResource::Sampler(SamplerBinding { sampler }),
        },
        ResourceBinding {
            binding: 15,
            resource: ResourceBindingResource::Buffer(BufferBinding {
                buffer: pass_buffer,
                offset: 0,
                size: BACKDROP_BLUR_PASS_BYTES as u64,
                stride: Some(BACKDROP_BLUR_PASS_BYTES as u32),
            }),
        },
    ]
}

pub(super) fn backdrop_blur_resource_bindings(
    global_buffer: BufferId,
    source_texture_view: TextureViewId,
    sampler: SamplerId,
    blur_buffer: BufferId,
) -> Vec<ResourceBinding> {
    vec![
        ResourceBinding {
            binding: 0,
            resource: ResourceBindingResource::Buffer(BufferBinding {
                buffer: global_buffer,
                offset: 0,
                size: GLOBAL_UPLOAD_BYTES as u64,
                stride: None,
            }),
        },
        ResourceBinding {
            binding: 4,
            resource: ResourceBindingResource::Texture(TextureBinding {
                texture_view: source_texture_view,
            }),
        },
        ResourceBinding {
            binding: 5,
            resource: ResourceBindingResource::Sampler(SamplerBinding { sampler }),
        },
        ResourceBinding {
            binding: 16,
            resource: ResourceBindingResource::Buffer(BufferBinding {
                buffer: blur_buffer,
                offset: 0,
                size: (MAX_BACKDROP_BLURS * PACKED_BACKDROP_BLUR_BYTES) as u64,
                stride: Some(PACKED_BACKDROP_BLUR_BYTES as u32),
            }),
        },
    ]
}

pub(super) fn custom_mesh_3d_resource_bindings(
    global_buffer: BufferId,
    parameters_buffer: BufferId,
    vertex_buffer: BufferId,
) -> Vec<ResourceBinding> {
    vec![
        ResourceBinding {
            binding: 0,
            resource: ResourceBindingResource::Buffer(BufferBinding {
                buffer: global_buffer,
                offset: 0,
                size: GLOBAL_UPLOAD_BYTES as u64,
                stride: None,
            }),
        },
        ResourceBinding {
            binding: 20,
            resource: ResourceBindingResource::Buffer(BufferBinding {
                buffer: parameters_buffer,
                offset: 0,
                size: (MAX_CUSTOM_MESH_3D_DRAWS * PACKED_CUSTOM_MESH_3D_PARAMETERS_BYTES) as u64,
                stride: Some(PACKED_CUSTOM_MESH_3D_PARAMETERS_BYTES as u32),
            }),
        },
        ResourceBinding {
            binding: 21,
            resource: ResourceBindingResource::Buffer(BufferBinding {
                buffer: vertex_buffer,
                offset: 0,
                size: (MAX_CUSTOM_MESH_3D_VERTICES * PACKED_CUSTOM_MESH_3D_VERTEX_BYTES) as u64,
                stride: Some(PACKED_CUSTOM_MESH_3D_VERTEX_BYTES as u32),
            }),
        },
    ]
}
