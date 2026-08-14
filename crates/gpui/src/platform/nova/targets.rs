use super::*;

#[derive(Clone)]
pub(super) struct NovaPathMaskTarget {
    pub(super) texture: TextureId,
    pub(super) texture_view: TextureViewId,
    pub(super) resource_sets: Vec<ResourceSetId>,
}

pub(super) struct NovaPathMaskTargetDescriptor {
    pub(super) size: Extent2d,
    pub(super) format: Format,
    pub(super) resource_set_layout: ResourceSetLayoutId,
    pub(super) frame_buffers: Vec<NovaFrameResourceBuffers>,
    pub(super) sampler: SamplerId,
}

#[derive(Clone)]
pub(super) struct NovaBackdropBlurTargets {
    pub(super) downsample: NovaBackdropBlurConfigSet,
    pub(super) source: NovaTextureTarget,
    pub(super) source_pass_resource_sets: Vec<ResourceSetId>,
    pub(super) variants: Vec<NovaBackdropBlurVariantTargets>,
}

#[derive(Clone)]
pub(super) struct NovaBackdropBlurVariantTargets {
    pub(super) config: NovaBackdropBlurConfig,
    pub(super) levels: Vec<NovaBackdropBlurLevelTarget>,
    pub(super) target_resource_sets: Vec<ResourceSetId>,
}

#[derive(Clone)]
pub(super) struct NovaBackdropBlurLevelTarget {
    pub(super) texture: TextureId,
    pub(super) texture_view: TextureViewId,
    pub(super) pass_resource_sets: Vec<ResourceSetId>,
}

pub(super) struct NovaBackdropBlurTargetDescriptor {
    pub(super) size: Extent2d,
    pub(super) format: Format,
    pub(super) downsample: NovaBackdropBlurConfigSet,
    pub(super) pass_resource_set_layout: ResourceSetLayoutId,
    pub(super) blur_resource_set_layout: ResourceSetLayoutId,
    pub(super) frame_buffers: Vec<NovaFrameResourceBuffers>,
    pub(super) sampler: SamplerId,
}

#[derive(Clone, Copy)]
pub(super) struct NovaTextureTarget {
    pub(super) texture: TextureId,
    pub(super) texture_view: TextureViewId,
}

impl NovaBackdropBlurTargets {
    pub(super) fn resource_set_for_config(
        &self,
        config: NovaBackdropBlurConfig,
        frame_resource_index: usize,
    ) -> Option<ResourceSetId> {
        self.variants
            .iter()
            .find(|variant| variant.config == config)?
            .target_resource_sets
            .get(frame_resource_index)
            .copied()
    }
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
    let mut resource_sets = Vec::with_capacity(descriptor.frame_buffers.len());
    for (index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
        resource_sets.push(device.create_resource_set(&ResourceSetDescriptor {
            label: Some(format!("{label} path mask frame {index} resource set")),
            layout: descriptor.resource_set_layout,
            bindings: path_resource_bindings(
                buffers.global_buffer,
                texture_view,
                descriptor.sampler,
                buffers.path_sprite_buffer,
            ),
        })?);
    }
    Ok(NovaPathMaskTarget {
        texture,
        texture_view,
        resource_sets,
    })
}

pub(super) fn destroy_path_mask_target<D>(
    device: &mut D,
    target: NovaPathMaskTarget,
    backend_name: &str,
) where
    D: BackendResources + BackendPipelines,
{
    for resource_set in target.resource_sets {
        if let Err(error) = device.destroy_resource_set(resource_set) {
            log::debug!("failed to destroy {backend_name} old path mask resource set: {error}");
        }
    }
    if let Err(error) = device.destroy_texture_view(target.texture_view) {
        log::debug!("failed to destroy {backend_name} old path mask texture view: {error}");
    }
    if let Err(error) = device.destroy_texture(target.texture) {
        log::debug!("failed to destroy {backend_name} old path mask texture: {error}");
    }
}

pub(super) fn create_backdrop_blur_target_chain<D>(
    device: &mut D,
    label: &str,
    descriptor: NovaBackdropBlurTargetDescriptor,
) -> Result<NovaBackdropBlurTargets>
where
    D: BackendResources + BackendPipelines,
{
    let config_set = descriptor.downsample.clone();
    let configs = config_set.configs().to_vec();
    create_backdrop_blur_target_chain_with_configs(device, label, descriptor, config_set, &configs)
}

fn create_backdrop_blur_target_chain_with_configs<D>(
    device: &mut D,
    label: &str,
    descriptor: NovaBackdropBlurTargetDescriptor,
    config_set: NovaBackdropBlurConfigSet,
    configs: &[NovaBackdropBlurConfig],
) -> Result<NovaBackdropBlurTargets>
where
    D: BackendResources + BackendPipelines,
{
    let source = create_render_texture_target(
        device,
        &format!("{label} backdrop blur shared source"),
        descriptor.size,
        descriptor.format,
    )?;
    let mut source_pass_resource_sets = Vec::with_capacity(descriptor.frame_buffers.len());
    for (index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
        source_pass_resource_sets.push(device.create_resource_set(&ResourceSetDescriptor {
            label: Some(format!(
                "{label} backdrop blur shared source frame {index} resource set"
            )),
            layout: descriptor.pass_resource_set_layout,
            bindings: backdrop_blur_pass_resource_bindings(
                source.texture_view,
                descriptor.sampler,
                buffers.backdrop_blur_pass_buffer,
            ),
        })?);
    }

    let mut variants = Vec::with_capacity(configs.len());
    for (variant_index, config) in configs.iter().copied().enumerate() {
        let downsample = u32::from(config.downsample().max(1));
        let level_count = config.levels().clamp(1, usize::from(MAX_BACKDROP_BLUR_LEVELS));
        let mut levels = Vec::with_capacity(level_count);
        for level_index in 0..level_count {
            let factor = downsample.saturating_mul(1_u32 << level_index as u32);
            let target_size = Extent2d::new(
                (descriptor.size.width() / factor).max(1),
                (descriptor.size.height() / factor).max(1),
            )?;
            let target = create_render_texture_target(
                device,
                &format!(
                    "{label} backdrop blur variant {variant_index} level {level_index}"
                ),
                target_size,
                descriptor.format,
            )?;
            let mut pass_resource_sets = Vec::with_capacity(descriptor.frame_buffers.len());
            for (frame_index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
                pass_resource_sets.push(device.create_resource_set(&ResourceSetDescriptor {
                    label: Some(format!(
                        "{label} backdrop blur variant {variant_index} level {level_index} frame {frame_index} pass resource set"
                    )),
                    layout: descriptor.pass_resource_set_layout,
                    bindings: backdrop_blur_pass_resource_bindings(
                        target.texture_view,
                        descriptor.sampler,
                        buffers.backdrop_blur_pass_buffer,
                    ),
                })?);
            }
            levels.push(NovaBackdropBlurLevelTarget {
                texture: target.texture,
                texture_view: target.texture_view,
                pass_resource_sets,
            });
        }

        let mut target_resource_sets = Vec::with_capacity(descriptor.frame_buffers.len());
        for (frame_index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
            let source_texture_view = levels
                .first()
                .map_or(source.texture_view, |level| level.texture_view);
            target_resource_sets.push(device.create_resource_set(&ResourceSetDescriptor {
                label: Some(format!(
                    "{label} backdrop blur variant {variant_index} frame {frame_index} resource set"
                )),
                layout: descriptor.blur_resource_set_layout,
                bindings: backdrop_blur_resource_bindings(
                    buffers.global_buffer,
                    source_texture_view,
                    descriptor.sampler,
                    buffers.backdrop_blur_buffer,
                ),
            })?);
        }

        variants.push(NovaBackdropBlurVariantTargets {
            config,
            levels,
            target_resource_sets,
        });
    }

    Ok(NovaBackdropBlurTargets {
        downsample: config_set,
        source,
        source_pass_resource_sets,
        variants,
    })
}

pub(super) fn destroy_backdrop_blur_target_chain<D>(
    device: &mut D,
    targets: NovaBackdropBlurTargets,
    backend_name: &str,
) where
    D: BackendResources + BackendPipelines,
{
    for resource_set in targets.source_pass_resource_sets {
        if let Err(error) = device.destroy_resource_set(resource_set) {
            log::debug!(
                "failed to destroy {backend_name} backdrop blur source resource set: {error}"
            );
        }
    }
    for variant in targets.variants {
        for resource_set in variant.target_resource_sets {
            if let Err(error) = device.destroy_resource_set(resource_set) {
                log::debug!(
                    "failed to destroy {backend_name} backdrop blur target resource set: {error}"
                );
            }
        }
        for target in variant.levels {
            for resource_set in target.pass_resource_sets {
                if let Err(error) = device.destroy_resource_set(resource_set) {
                    log::debug!(
                        "failed to destroy {backend_name} backdrop blur level resource set: {error}"
                    );
                }
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
