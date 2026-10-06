use super::*;

pub(super) struct AtlasResourceDescriptor {
    pub(super) mono_sprite_resource_set_layout: ResourceSetLayoutId,
    pub(super) poly_sprite_resource_set_layout: ResourceSetLayoutId,
    pub(super) frame_buffers: Vec<FrameResourceBuffers>,
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
            resource: BindingResource::Buffer(BufferBinding {
                buffer: global_buffer,
                offset: 0,
                size: GLOBAL_UPLOAD_BYTES as u64,
                stride: None,
            }),
        },
        ResourceBinding {
            binding: 4,
            resource: BindingResource::Texture(TextureBinding {
                texture_view: path_texture_view,
            }),
        },
        ResourceBinding {
            binding: 5,
            resource: BindingResource::Sampler(SamplerBinding { sampler }),
        },
        ResourceBinding {
            binding: 6,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: path_sprite_buffer,
                offset: 0,
                size: (MAX_PATH_SPRITES * PACKED_PATH_SPRITE_BYTES) as u64,
                stride: Some(PACKED_PATH_SPRITE_BYTES as u32),
            }),
        },
    ]
}

pub(super) fn backdrop_blur_pass_resource_bindings(
    source_texture_view: TextureViewId,
    sampler: SamplerId,
    pass_buffer: BufferId,
    animation_buffer: BufferId,
) -> Vec<ResourceBinding> {
    vec![
        ResourceBinding {
            binding: 4,
            resource: BindingResource::Texture(TextureBinding {
                texture_view: source_texture_view,
            }),
        },
        ResourceBinding {
            binding: 5,
            resource: BindingResource::Sampler(SamplerBinding { sampler }),
        },
        ResourceBinding {
            binding: 15,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: pass_buffer,
                offset: 0,
                size: (MAX_BACKDROP_BLURS * 2 * BACKDROP_BLUR_PASS_BYTES) as u64,
                stride: Some(BACKDROP_BLUR_PASS_BYTES as u32),
            }),
        },
        ResourceBinding {
            binding: 17,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: animation_buffer,
                offset: 0,
                size: (MAX_ANIMATION_VALUES * PACKED_ANIMATION_VALUE_BYTES) as u64,
                stride: Some(PACKED_ANIMATION_VALUE_BYTES as u32),
            }),
        },
    ]
}

pub(super) fn backdrop_blur_resource_bindings(
    global_buffer: BufferId,
    source_texture_view: TextureViewId,
    sampler: SamplerId,
    blur_buffer: BufferId,
    animation_buffer: BufferId,
) -> Vec<ResourceBinding> {
    vec![
        ResourceBinding {
            binding: 0,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: global_buffer,
                offset: 0,
                size: GLOBAL_UPLOAD_BYTES as u64,
                stride: None,
            }),
        },
        ResourceBinding {
            binding: 4,
            resource: BindingResource::Texture(TextureBinding {
                texture_view: source_texture_view,
            }),
        },
        ResourceBinding {
            binding: 5,
            resource: BindingResource::Sampler(SamplerBinding { sampler }),
        },
        ResourceBinding {
            binding: 16,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: blur_buffer,
                offset: 0,
                size: (MAX_BACKDROP_BLURS * PACKED_BACKDROP_BLUR_BYTES) as u64,
                stride: Some(PACKED_BACKDROP_BLUR_BYTES as u32),
            }),
        },
        ResourceBinding {
            binding: 17,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: animation_buffer,
                offset: 0,
                size: (MAX_ANIMATION_VALUES * PACKED_ANIMATION_VALUE_BYTES) as u64,
                stride: Some(PACKED_ANIMATION_VALUE_BYTES as u32),
            }),
        },
    ]
}
