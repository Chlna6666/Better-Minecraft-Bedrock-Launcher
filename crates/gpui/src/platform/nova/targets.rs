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
    /// Shared accumulated scene color used as the ordered backdrop source.
    pub(super) source: NovaTextureTarget,
    pub(super) source_pass_resource_sets: Vec<ResourceSetId>,
    pub(super) variants: Vec<NovaBackdropBlurVariantTargets>,
}

#[derive(Clone)]
pub(super) struct NovaBackdropBlurVariantTargets {
    pub(super) config: NovaBackdropBlurConfig,
    /// Two separable Gaussian targets with axis-specific resolution:
    /// levels[0] = X-blurred target at `(W/downsample, H)`;
    /// levels[1] = Y-blurred final target at `(W/downsample, H/downsample)`.
    /// This prevents vertical aliasing from shrinking Y before a vertical low-pass has run.
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
            .find(|variant| variant.config.covers(config))?
            .target_resource_sets
            .get(frame_resource_index)
            .copied()
    }

    /// Returns whether the existing GPU texture layout can serve the next frame.
    ///
    /// Bounds are intentionally ignored. Moving/resizing an animated glass surface updates only
    /// CPU metadata and scissors instead of destroying and recreating GPU textures.
    pub(super) fn is_layout_compatible(&self, next: &NovaBackdropBlurConfigSet) -> bool {
        self.variants.len() == next.configs().len()
            && self
                .variants
                .iter()
                .zip(next.configs())
                .all(|(variant, config)| {
                    variant.config.reuse_key() == config.reuse_key()
                        && variant.config.downsample() == config.downsample()
                        && variant.config.levels() == config.levels()
                })
    }

    /// Updates per-frame canonical bounds without reallocating GPU resources.
    pub(super) fn update_configs(&mut self, next: NovaBackdropBlurConfigSet) {
        for (variant, config) in self.variants.iter_mut().zip(next.configs()) {
            variant.config = *config;
        }
        self.downsample = next;
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
        &format!("{label} backdrop scene color"),
        descriptor.size,
        descriptor.format,
    )?;
    let mut source_pass_resource_sets = Vec::with_capacity(descriptor.frame_buffers.len());
    for (index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
        source_pass_resource_sets.push(device.create_resource_set(&ResourceSetDescriptor {
            label: Some(format!(
                "{label} backdrop scene color frame {index} resource set"
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
        let horizontal_size = Extent2d::new(
            descriptor.size.width().div_ceil(downsample).max(1),
            descriptor.size.height(),
        )?;
        let final_size = Extent2d::new(
            descriptor.size.width().div_ceil(downsample).max(1),
            descriptor.size.height().div_ceil(downsample).max(1),
        )?;
        let pass_sizes = [horizontal_size, final_size];
        let mut levels = Vec::with_capacity(pass_sizes.len());
        for (pass_index, target_size) in pass_sizes.into_iter().enumerate() {
            let target = create_render_texture_target(
                device,
                &format!(
                    "{label} backdrop gaussian variant {variant_index} pass {pass_index}"
                ),
                target_size,
                descriptor.format,
            )?;
            let mut pass_resource_sets = Vec::with_capacity(descriptor.frame_buffers.len());
            for (frame_index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
                pass_resource_sets.push(device.create_resource_set(&ResourceSetDescriptor {
                    label: Some(format!(
                        "{label} backdrop gaussian variant {variant_index} pass {pass_index} frame {frame_index} resource set"
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
                .last()
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
        log::debug!("failed to destroy {backend_name} old path mask texture view: {error}");
    }
    if let Err(error) = device.destroy_texture(texture) {
        log::debug!("failed to destroy {backend_name} old path mask texture: {error}");
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
