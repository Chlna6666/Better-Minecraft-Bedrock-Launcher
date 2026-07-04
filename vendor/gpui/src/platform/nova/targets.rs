use super::*;

#[derive(Clone, Copy)]
pub(super) struct NovaPathMaskTarget {
    pub(super) texture: TextureId,
    pub(super) texture_view: TextureViewId,
    pub(super) resource_set: ResourceSetId,
}

#[derive(Clone, Copy)]
pub(super) struct NovaPathMaskTargetDescriptor {
    pub(super) size: Extent2d,
    pub(super) format: Format,
    pub(super) resource_set_layout: ResourceSetLayoutId,
    pub(super) global_buffer: BufferId,
    pub(super) path_sprite_buffer: BufferId,
    pub(super) sampler: SamplerId,
}

#[derive(Clone, Copy)]
pub(super) struct NovaPresentCacheTarget {
    pub(super) texture: TextureId,
    pub(super) texture_view: TextureViewId,
    pub(super) resource_set: ResourceSetId,
}

#[derive(Clone, Copy)]
pub(super) struct NovaPresentCacheTargetDescriptor {
    pub(super) size: Extent2d,
    pub(super) format: Format,
    pub(super) resource_set_layout: ResourceSetLayoutId,
    pub(super) global_buffer: BufferId,
    pub(super) sprite_buffer: BufferId,
    pub(super) sampler: SamplerId,
}

#[derive(Clone)]
pub(super) struct NovaBackdropBlurTargets {
    pub(super) downsample: u8,
    pub(super) source: NovaTextureTarget,
    pub(super) levels: Vec<NovaBackdropBlurLevelTarget>,
    pub(super) source_pass_resource_set: ResourceSetId,
    pub(super) target_resource_set: ResourceSetId,
}

#[derive(Clone, Copy)]
pub(super) struct NovaBackdropBlurLevelTarget {
    pub(super) texture: TextureId,
    pub(super) texture_view: TextureViewId,
    pub(super) pass_resource_set: ResourceSetId,
}

#[derive(Clone, Copy)]
pub(super) struct NovaBackdropBlurTargetDescriptor {
    pub(super) size: Extent2d,
    pub(super) format: Format,
    pub(super) downsample: u8,
    pub(super) pass_resource_set_layout: ResourceSetLayoutId,
    pub(super) blur_resource_set_layout: ResourceSetLayoutId,
    pub(super) global_buffer: BufferId,
    pub(super) pass_buffer: BufferId,
    pub(super) blur_buffer: BufferId,
    pub(super) sampler: SamplerId,
}

#[derive(Clone, Copy)]
pub(super) struct NovaTextureTarget {
    pub(super) texture: TextureId,
    pub(super) texture_view: TextureViewId,
}

pub(super) fn create_path_mask_target<D>(
    device: &mut D,
    label: &str,
    descriptor: NovaPathMaskTargetDescriptor,
) -> Result<NovaPathMaskTarget>
where
    D: BackendResources + BackendPipelines,
{
    let texture = device.create_texture(&TextureDescriptor {
        label: Some(format!("{label} path mask texture")),
        size: descriptor.size,
        format: descriptor.format,
        usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
    })?;
    let texture_view = device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!("{label} path mask texture view")),
        texture,
        format: descriptor.format,
    })?;
    let resource_set = device.create_resource_set(&ResourceSetDescriptor {
        label: Some(format!("{label} path mask resource set")),
        layout: descriptor.resource_set_layout,
        bindings: path_resource_bindings(
            descriptor.global_buffer,
            texture_view,
            descriptor.sampler,
            descriptor.path_sprite_buffer,
        ),
    })?;
    Ok(NovaPathMaskTarget {
        texture,
        texture_view,
        resource_set,
    })
}

pub(super) fn destroy_path_mask_target<D>(
    device: &mut D,
    target: NovaPathMaskTarget,
    backend_name: &str,
) where
    D: BackendResources + BackendPipelines,
{
    if let Err(error) = device.destroy_resource_set(target.resource_set) {
        log::debug!("failed to destroy {backend_name} old path mask resource set: {error}");
    }
    if let Err(error) = device.destroy_texture_view(target.texture_view) {
        log::debug!("failed to destroy {backend_name} old path mask texture view: {error}");
    }
    if let Err(error) = device.destroy_texture(target.texture) {
        log::debug!("failed to destroy {backend_name} old path mask texture: {error}");
    }
}

pub(super) fn create_present_cache_target<D>(
    device: &mut D,
    label: &str,
    descriptor: NovaPresentCacheTargetDescriptor,
) -> Result<NovaPresentCacheTarget>
where
    D: BackendResources + BackendPipelines,
{
    let target = create_render_texture_target(
        device,
        &format!("{label} retained present cache"),
        descriptor.size,
        descriptor.format,
    )?;
    let resource_set = device.create_resource_set(&ResourceSetDescriptor {
        label: Some(format!("{label} retained present cache resource set")),
        layout: descriptor.resource_set_layout,
        bindings: present_cache_resource_bindings(
            descriptor.global_buffer,
            target.texture_view,
            descriptor.sampler,
            descriptor.sprite_buffer,
        ),
    })?;
    Ok(NovaPresentCacheTarget {
        texture: target.texture,
        texture_view: target.texture_view,
        resource_set,
    })
}

pub(super) fn destroy_present_cache_target<D>(
    device: &mut D,
    target: NovaPresentCacheTarget,
    backend_name: &str,
) where
    D: BackendResources + BackendPipelines,
{
    if let Err(error) = device.destroy_resource_set(target.resource_set) {
        log::debug!("failed to destroy {backend_name} retained present resource set: {error}");
    }
    destroy_render_texture_target(
        device,
        NovaTextureTarget {
            texture: target.texture,
            texture_view: target.texture_view,
        },
        backend_name,
    );
}

pub(super) fn create_backdrop_blur_target_chain<D>(
    device: &mut D,
    label: &str,
    descriptor: NovaBackdropBlurTargetDescriptor,
) -> Result<NovaBackdropBlurTargets>
where
    D: BackendResources + BackendPipelines,
{
    let source = create_render_texture_target(
        device,
        &format!("{label} backdrop blur source"),
        descriptor.size,
        descriptor.format,
    )?;
    let source_pass_resource_set = device.create_resource_set(&ResourceSetDescriptor {
        label: Some(format!("{label} backdrop blur source pass resource set")),
        layout: descriptor.pass_resource_set_layout,
        bindings: backdrop_blur_pass_resource_bindings(
            source.texture_view,
            descriptor.sampler,
            descriptor.pass_buffer,
        ),
    })?;
    let downsample = descriptor.downsample.max(1);
    let mut levels = Vec::with_capacity(usize::from(MAX_BACKDROP_BLUR_LEVELS));
    for level in 0..MAX_BACKDROP_BLUR_LEVELS {
        let factor = u32::from(downsample).saturating_mul(1_u32 << u32::from(level));
        let target_size = Extent2d::new(
            (descriptor.size.width() / factor).max(1),
            (descriptor.size.height() / factor).max(1),
        )?;
        let target = create_render_texture_target(
            device,
            &format!("{label} backdrop blur target level {level}"),
            target_size,
            descriptor.format,
        )?;
        let pass_resource_set = device.create_resource_set(&ResourceSetDescriptor {
            label: Some(format!(
                "{label} backdrop blur target level {level} pass resource set"
            )),
            layout: descriptor.pass_resource_set_layout,
            bindings: backdrop_blur_pass_resource_bindings(
                target.texture_view,
                descriptor.sampler,
                descriptor.pass_buffer,
            ),
        })?;
        levels.push(NovaBackdropBlurLevelTarget {
            texture: target.texture,
            texture_view: target.texture_view,
            pass_resource_set,
        });
    }
    let target_resource_set = device.create_resource_set(&ResourceSetDescriptor {
        label: Some(format!("{label} backdrop blur target resource set")),
        layout: descriptor.blur_resource_set_layout,
        bindings: backdrop_blur_resource_bindings(
            descriptor.global_buffer,
            levels
                .first()
                .map_or(source.texture_view, |level| level.texture_view),
            descriptor.sampler,
            descriptor.blur_buffer,
        ),
    })?;
    Ok(NovaBackdropBlurTargets {
        downsample,
        source,
        levels,
        source_pass_resource_set,
        target_resource_set,
    })
}

pub(super) fn destroy_backdrop_blur_target_chain<D>(
    device: &mut D,
    targets: NovaBackdropBlurTargets,
    backend_name: &str,
) where
    D: BackendResources + BackendPipelines,
{
    if let Err(error) = device.destroy_resource_set(targets.source_pass_resource_set) {
        log::debug!("failed to destroy {backend_name} backdrop blur source resource set: {error}");
    }
    if let Err(error) = device.destroy_resource_set(targets.target_resource_set) {
        log::debug!("failed to destroy {backend_name} backdrop blur target resource set: {error}");
    }
    for target in targets.levels {
        if let Err(error) = device.destroy_resource_set(target.pass_resource_set) {
            log::debug!(
                "failed to destroy {backend_name} backdrop blur level resource set: {error}"
            );
        }
        destroy_render_texture_target(
            device,
            NovaTextureTarget {
                texture: target.texture,
                texture_view: target.texture_view,
            },
            backend_name,
        );
    }
    destroy_render_texture_target(device, targets.source, backend_name);
}

pub(super) fn create_depth_target<D>(
    device: &mut D,
    label: &str,
    size: Extent2d,
) -> Result<(TextureId, TextureViewId)>
where
    D: BackendResources,
{
    let texture = create_depth_texture(device, label, size)?;
    let texture_view = device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!("{label} depth texture view")),
        texture,
        format: Format::Depth32Float,
    })?;
    Ok((texture, texture_view))
}

pub(super) fn destroy_depth_target<D>(
    device: &mut D,
    texture: TextureId,
    texture_view: TextureViewId,
    backend_name: &str,
) where
    D: BackendResources,
{
    if let Err(error) = device.destroy_texture_view(texture_view) {
        log::debug!("failed to destroy {backend_name} depth texture view: {error}");
    }
    if let Err(error) = device.destroy_texture(texture) {
        log::debug!("failed to destroy {backend_name} depth texture: {error}");
    }
}

fn create_render_texture_target<D>(
    device: &mut D,
    label: &str,
    size: Extent2d,
    format: Format,
) -> Result<NovaTextureTarget>
where
    D: BackendResources + BackendPipelines,
{
    let texture = device.create_texture(&TextureDescriptor {
        label: Some(format!("{label} texture")),
        size,
        format,
        usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
    })?;
    let texture_view = device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!("{label} texture view")),
        texture,
        format,
    })?;
    Ok(NovaTextureTarget {
        texture,
        texture_view,
    })
}

fn destroy_render_texture_target<D>(device: &mut D, target: NovaTextureTarget, backend_name: &str)
where
    D: BackendResources + BackendPipelines,
{
    if let Err(error) = device.destroy_texture_view(target.texture_view) {
        log::debug!("failed to destroy {backend_name} texture target view: {error}");
    }
    if let Err(error) = device.destroy_texture(target.texture) {
        log::debug!("failed to destroy {backend_name} texture target: {error}");
    }
}
