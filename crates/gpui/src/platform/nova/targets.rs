use super::*;

#[derive(Clone)]
pub(super) struct PathMaskTarget {
    pub(super) texture: TextureId,
    pub(super) texture_view: TextureViewId,
    pub(super) resource_sets: Vec<ResourceSetId>,
}

pub(super) struct PathMaskTargetDescriptor {
    pub(super) size: Extent2d,
    pub(super) format: Format,
    pub(super) resource_set_layout: ResourceSetLayoutId,
    pub(super) frame_buffers: Vec<FrameResourceBuffers>,
    pub(super) sampler: SamplerId,
}

#[derive(Clone)]
pub(super) struct BackdropBlurTargets {
    /// Shared accumulated scene color used as the ordered backdrop source.
    pub(super) source: TextureTarget,
    pub(super) source_pass_resource_sets: Vec<ResourceSetId>,
    /// Per-element sources. Each CSS blur group starts from a clean attachment so one group's
    /// content can never become the backdrop source of a sibling group.
    pub(super) isolated_sources: Vec<IsolatedBlurSource>,
    pub(super) variants: Vec<BackdropBlurVariantTargets>,
}

#[derive(Clone)]
pub(super) struct IsolatedBlurSource {
    pub(super) index: u32,
    pub(super) target: TextureTarget,
    pub(super) pass_resource_sets: Vec<ResourceSetId>,
}

#[derive(Clone)]
pub(super) struct BackdropBlurVariantTargets {
    pub(super) config: BackdropBlurConfig,
    /// Two separable Gaussian targets with axis-specific resolution:
    /// levels[0] = X-blurred target at `(W/downsample, H)`;
    /// levels[1] = Y-blurred final target at `(W/downsample, H/downsample)`.
    /// This prevents vertical aliasing from shrinking Y before a vertical low-pass has run.
    pub(super) levels: Vec<BackdropBlurLevelTarget>,
    pub(super) target_resource_sets: Vec<ResourceSetId>,
}

#[derive(Clone)]
pub(super) struct BackdropBlurLevelTarget {
    pub(super) texture: TextureId,
    pub(super) texture_view: TextureViewId,
    pub(super) pass_resource_sets: Vec<ResourceSetId>,
}

pub(super) struct BackdropBlurTargetDescriptor {
    pub(super) size: Extent2d,
    pub(super) format: Format,
    pub(super) configs: Vec<BackdropBlurConfig>,
    pub(super) isolated_source_indices: Vec<u32>,
    pub(super) pass_resource_set_layout: ResourceSetLayoutId,
    pub(super) blur_resource_set_layout: ResourceSetLayoutId,
    pub(super) frame_buffers: Vec<FrameResourceBuffers>,
    pub(super) sampler: SamplerId,
}

#[derive(Clone, Copy)]
pub(super) struct TextureTarget {
    pub(super) texture: TextureId,
    pub(super) texture_view: TextureViewId,
}

fn blur_requires_isolated_source(configs: &[BackdropBlurConfig], index: u32) -> bool {
    configs
        .iter()
        .copied()
        .any(|config| config.contains_member_index(index) && config.radius() > 0.0)
}

impl BackdropBlurTargets {
    pub(super) fn resource_set_for_config(
        &self,
        config: BackdropBlurConfig,
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
    pub(super) fn isolated_source_texture_view(&self, index: u32) -> Option<TextureViewId> {
        self.isolated_sources
            .iter()
            .find(|source| source.index == index)
            .map(|source| source.target.texture_view)
    }

    pub(super) fn isolated_source_resource_set(
        &self,
        index: u32,
        frame_resource_index: usize,
    ) -> Option<ResourceSetId> {
        self.isolated_sources
            .iter()
            .find(|source| source.index == index)?
            .pass_resource_sets
            .get(frame_resource_index)
            .copied()
    }

    pub(super) fn is_layout_compatible(
        &self,
        next: &[BackdropBlurConfig],
        next_isolated_source_indices: &[u32],
    ) -> bool {
        self.variants.len() == next.len()
            && self
                .isolated_sources
                .iter()
                .map(|source| source.index)
                .eq(next_isolated_source_indices.iter().copied().filter(|index| {
                    blur_requires_isolated_source(next, *index)
                }))
            && self
                .variants
                .iter()
                .zip(next)
                .all(|(variant, config)| variant.config.same_target_slot(*config))
    }
}

pub(super) fn create_path_mask_target<D>(
    device: &mut D,
    label: &str,
    descriptor: PathMaskTargetDescriptor,
) -> Result<PathMaskTarget>
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
    Ok(PathMaskTarget {
        texture,
        texture_view,
        resource_sets,
    })
}

pub(super) fn destroy_path_mask_target<D>(
    device: &mut D,
    target: PathMaskTarget,
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
    mut descriptor: BackdropBlurTargetDescriptor,
) -> Result<BackdropBlurTargets>
where
    D: BackendResources + BackendPipelines,
{
    let configs = std::mem::take(&mut descriptor.configs);
    create_backdrop_blur_target_chain_with_configs(device, label, descriptor, &configs)
}

fn create_backdrop_blur_target_chain_with_configs<D>(
    device: &mut D,
    label: &str,
    descriptor: BackdropBlurTargetDescriptor,
    configs: &[BackdropBlurConfig],
) -> Result<BackdropBlurTargets>
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

    let mut isolated_source_indices = descriptor.isolated_source_indices;
    isolated_source_indices.sort_unstable();
    isolated_source_indices.dedup();
    isolated_source_indices.retain(|index| blur_requires_isolated_source(configs, *index));
    let mut isolated_sources = Vec::with_capacity(isolated_source_indices.len());
    for index in isolated_source_indices {
        let target = create_render_texture_target(
            device,
            &format!("{label} element blur {index} scene color"),
            descriptor.size,
            descriptor.format,
        )?;
        let mut pass_resource_sets = Vec::with_capacity(descriptor.frame_buffers.len());
        for (frame_index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
            pass_resource_sets.push(device.create_resource_set(&ResourceSetDescriptor {
                label: Some(format!(
                    "{label} element blur {index} scene color frame {frame_index} resource set"
                )),
                layout: descriptor.pass_resource_set_layout,
                bindings: backdrop_blur_pass_resource_bindings(
                    target.texture_view,
                    descriptor.sampler,
                    buffers.backdrop_blur_pass_buffer,
                ),
            })?);
        }
        isolated_sources.push(IsolatedBlurSource {
            index,
            target,
            pass_resource_sets,
        });
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
                &format!("{label} backdrop gaussian variant {variant_index} pass {pass_index}"),
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
            levels.push(BackdropBlurLevelTarget {
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

        variants.push(BackdropBlurVariantTargets {
            config,
            levels,
            target_resource_sets,
        });
    }

    Ok(BackdropBlurTargets {
        source,
        source_pass_resource_sets,
        isolated_sources,
        variants,
    })
}

pub(super) fn destroy_backdrop_blur_target_chain<D>(
    device: &mut D,
    targets: BackdropBlurTargets,
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
    for source in targets.isolated_sources {
        for resource_set in source.pass_resource_sets {
            if let Err(error) = device.destroy_resource_set(resource_set) {
                log::debug!(
                    "failed to destroy {backend_name} element blur source resource set: {error}"
                );
            }
        }
        destroy_render_texture_target(device, source.target, backend_name);
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
                TextureTarget {
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
) -> Result<TextureTarget>
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
    Ok(TextureTarget {
        texture,
        texture_view,
    })
}

fn destroy_render_texture_target<D>(device: &mut D, target: TextureTarget, backend_name: &str)
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
