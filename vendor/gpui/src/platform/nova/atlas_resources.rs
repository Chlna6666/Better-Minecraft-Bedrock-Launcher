use super::*;

#[derive(Clone, Copy)]
pub(super) struct NovaGpuAtlasTexture {
    pub(super) size: Size<DevicePixels>,
    pub(super) texture: TextureId,
    pub(super) texture_view: TextureViewId,
    pub(super) mono_resource_set: ResourceSetId,
    pub(super) poly_resource_set: ResourceSetId,
}

fn default_atlas_texture_size() -> Size<DevicePixels> {
    Size {
        width: DevicePixels(NOVA_DEFAULT_ATLAS_SIZE as i32),
        height: DevicePixels(NOVA_DEFAULT_ATLAS_SIZE as i32),
    }
}

pub(super) fn initial_gpu_atlas_textures(
    resources: &NovaRendererResources,
) -> FxHashMap<AtlasTextureId, NovaGpuAtlasTexture> {
    let mut textures = FxHashMap::default();
    textures.insert(
        AtlasTextureId {
            index: 0,
            kind: AtlasTextureKind::Bgra,
        },
        NovaGpuAtlasTexture {
            size: default_atlas_texture_size(),
            texture: resources.atlas_texture,
            texture_view: resources.atlas_texture_view,
            mono_resource_set: resources.mono_sprite_resource_set,
            poly_resource_set: resources.poly_sprite_resource_set,
        },
    );
    textures
}

fn destroy_gpu_atlas_texture<D>(
    device: &mut D,
    texture: NovaGpuAtlasTexture,
    backend_name: &str,
    atlas_id: AtlasTextureId,
) where
    D: BackendResources,
{
    if let Err(error) = device.destroy_resource_set(texture.mono_resource_set) {
        log::debug!(
            "failed to destroy {backend_name} atlas {:?}/{} mono resource set: {error}",
            atlas_id.kind,
            atlas_id.index
        );
    }
    if let Err(error) = device.destroy_resource_set(texture.poly_resource_set) {
        log::debug!(
            "failed to destroy {backend_name} atlas {:?}/{} poly resource set: {error}",
            atlas_id.kind,
            atlas_id.index
        );
    }
    if let Err(error) = device.destroy_texture_view(texture.texture_view) {
        log::debug!(
            "failed to destroy {backend_name} atlas {:?}/{} texture view: {error}",
            atlas_id.kind,
            atlas_id.index
        );
    }
    if let Err(error) = device.destroy_texture(texture.texture) {
        log::debug!(
            "failed to destroy {backend_name} atlas {:?}/{} texture: {error}",
            atlas_id.kind,
            atlas_id.index
        );
    }
}

pub(super) fn sync_gpu_atlas_textures<D>(
    atlas: &NovaAtlas,
    gpu_textures: &mut FxHashMap<AtlasTextureId, NovaGpuAtlasTexture>,
    device: &mut D,
    backend_name: &str,
    descriptor: NovaAtlasResourceDescriptor,
) -> Result<()>
where
    D: BackendResources,
{
    let texture_infos = atlas.texture_infos();
    let live_ids = texture_infos
        .iter()
        .map(|texture| texture.id)
        .collect::<FxHashSet<_>>();
    let stale_ids = gpu_textures
        .keys()
        .copied()
        .filter(|id| !live_ids.contains(id))
        .collect::<Vec<_>>();
    for stale_id in stale_ids {
        if let Some(texture) = gpu_textures.remove(&stale_id) {
            destroy_gpu_atlas_texture(device, texture, backend_name, stale_id);
        }
    }

    for texture_info in texture_infos {
        if gpu_textures
            .get(&texture_info.id)
            .is_some_and(|texture| texture.size == texture_info.size)
        {
            continue;
        }
        if let Some(texture) = gpu_textures.remove(&texture_info.id) {
            destroy_gpu_atlas_texture(device, texture, backend_name, texture_info.id);
        }
        let gpu_texture = create_atlas_texture_resources(
            device,
            backend_name,
            texture_info.id,
            texture_info.size,
            descriptor,
        )?;
        gpu_textures.insert(texture_info.id, gpu_texture);
    }

    Ok(())
}

pub(super) fn create_atlas_texture_resources<D>(
    device: &mut D,
    label: &str,
    atlas_id: AtlasTextureId,
    size: Size<DevicePixels>,
    descriptor: NovaAtlasResourceDescriptor,
) -> Result<NovaGpuAtlasTexture>
where
    D: BackendResources,
{
    let width =
        u32::try_from(size.width.0.max(1)).context("nova atlas texture width does not fit u32")?;
    let height = u32::try_from(size.height.0.max(1))
        .context("nova atlas texture height does not fit u32")?;
    let texture = device.create_texture(&TextureDescriptor {
        label: Some(format!(
            "{label} atlas {:?}/{} texture",
            atlas_id.kind, atlas_id.index
        )),
        size: Extent2d::new(width, height)?,
        format: Format::Bgra8Unorm,
        usage: TextureUsage::COPY_DST | TextureUsage::SAMPLED,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
    })?;
    let texture_view = device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!(
            "{label} atlas {:?}/{} texture view",
            atlas_id.kind, atlas_id.index
        )),
        texture,
        format: Format::Bgra8Unorm,
    })?;
    let mono_resource_set = device.create_resource_set(&ResourceSetDescriptor {
        label: Some(format!(
            "{label} atlas {:?}/{} mono resource set",
            atlas_id.kind, atlas_id.index
        )),
        layout: descriptor.mono_sprite_resource_set_layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: descriptor.global_buffer,
                    offset: 0,
                    size: GLOBAL_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 1,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: descriptor.text_raster_buffer,
                    offset: 0,
                    size: TEXT_RASTER_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 4,
                resource: ResourceBindingResource::Texture(TextureBinding { texture_view }),
            },
            ResourceBinding {
                binding: 5,
                resource: ResourceBindingResource::Sampler(SamplerBinding {
                    sampler: descriptor.sampler,
                }),
            },
            ResourceBinding {
                binding: 8,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: descriptor.mono_sprite_buffer,
                    offset: 0,
                    size: (MAX_MONO_SPRITES * PACKED_MONO_SPRITE_BYTES) as u64,
                    stride: Some(PACKED_MONO_SPRITE_BYTES as u32),
                }),
            },
        ],
    })?;
    let poly_resource_set = device.create_resource_set(&ResourceSetDescriptor {
        label: Some(format!(
            "{label} atlas {:?}/{} poly resource set",
            atlas_id.kind, atlas_id.index
        )),
        layout: descriptor.poly_sprite_resource_set_layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: descriptor.global_buffer,
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
                resource: ResourceBindingResource::Sampler(SamplerBinding {
                    sampler: descriptor.sampler,
                }),
            },
            ResourceBinding {
                binding: 9,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: descriptor.poly_sprite_buffer,
                    offset: 0,
                    size: (MAX_POLY_SPRITES * PACKED_POLY_SPRITE_BYTES) as u64,
                    stride: Some(PACKED_POLY_SPRITE_BYTES as u32),
                }),
            },
        ],
    })?;

    Ok(NovaGpuAtlasTexture {
        size,
        texture,
        texture_view,
        mono_resource_set,
        poly_resource_set,
    })
}
