use super::*;

mod layout;

#[derive(Clone)]
pub(super) struct PathMaskTarget {
    pub(super) texture: TextureId,
    pub(super) texture_view: TextureViewId,
    pub(super) bind_groups: Vec<BindGroupId>,
}

pub(super) struct PathMaskTargetDescriptor {
    pub(super) size: Extent2d,
    pub(super) format: Format,
    pub(super) bind_group_layout: BindGroupLayoutId,
    pub(super) frame_buffers: Vec<FrameResourceBuffers>,
    pub(super) sampler: SamplerId,
}

#[derive(Clone)]
pub(super) struct BackdropBlurTargets {
    /// Shared scene color for root backdrop barriers; absent for isolated-only captures.
    pub(super) source: Option<RenderTarget>,
    pub(super) source_pass_resource_sets: Vec<BindGroupId>,
    /// Per-element sources. Each CSS blur group starts from a clean attachment so one group's
    /// content can never become the backdrop source of a sibling group.
    pub(super) isolated_sources: Vec<IsolatedBlurSource>,
    /// Only the active prefix participates in the current filter graph. Spare
    /// scratch targets remain GPU-resident across popup insertion/removal.
    pub(super) active_isolated_source_count: usize,
    pub(super) variants: Vec<BackdropBlurVariantTargets>,
    /// Live prefix; spare popup targets remain allocated until the chain is replaced or trimmed.
    pub(super) active_variant_count: usize,
}

/// Unique native lifetime authority for the full-resolution Root Color.
/// Filter Graph consumers resolve its descriptor IDs directly here;
/// only this owner retires the allocation and its per-frame ResourceSets.
pub(super) struct RootColorTarget {
    pub(super) target: RenderTarget,
    pub(super) pass_resource_sets: Vec<BindGroupId>,
    pub(super) composite_resource_sets: Vec<BindGroupId>,
}

impl BackdropBlurTargets {
    /// Only a single-member zero-filter output has an independently initialized texture.
    pub(super) fn exclusive_zero_radius_capture_view(&self, index: u32) -> Option<TextureViewId> {
        let mut candidates = self
            .variants
            .iter()
            .take(self.active_variant_count)
            .filter(|variant| variant.config.contains_member_index(index));
        let variant = candidates.next()?;
        if candidates.next().is_some()
            || !variant.config.exclusively_contains_member_index(index)
            || variant.config.radius() != 0.0
            || variant.levels.len() != 1
        {
            return None;
        }
        Some(variant.levels[0].texture_view)
    }

    pub(super) fn byte_size(&self) -> u64 {
        let mut textures = FxHashSet::default();
        self.isolated_sources
            .iter()
            .fold(
                self.source.map_or(0, |source| source.byte_size),
                |total, source| total.saturating_add(source.target.byte_size),
            )
            .saturating_add(
                self.variants
                    .iter()
                    .flat_map(|variant| &variant.levels)
                    .filter(|level| textures.insert(level.texture))
                    .map(|level| level.byte_size)
                    .sum::<u64>(),
            )
    }
}

#[derive(Clone)]
pub(super) struct IsolatedBlurSource {
    pub(super) index: u32,
    pub(super) target: RenderTarget,
    pub(super) pass_resource_sets: Vec<BindGroupId>,
}

#[derive(Clone)]
pub(super) struct BackdropBlurVariantTargets {
    pub(super) config: BackdropBlurConfig,
    /// Positive-radius Gaussian filters own two axis-specific targets:
    /// levels[0] = X-blurred target at `(W/downsample, H)`;
    /// levels[1] = Y-blurred final target at `(W/downsample, H/downsample)`.
    /// Zero-filter compositor layers own only levels[0], a full-resolution retained final texture;
    /// they execute no Gaussian passes and render their captured subtree directly into that target.
    pub(super) levels: Vec<BackdropBlurLevelTarget>,
    pub(super) target_resource_sets: Vec<BindGroupId>,
}

#[derive(Clone)]
pub(super) struct BackdropBlurLevelTarget {
    pub(super) byte_size: u64,
    pub(super) texture: TextureId,
    pub(super) texture_view: TextureViewId,
    pub(super) pass_resource_sets: Vec<BindGroupId>,
}

pub(super) struct BackdropBlurTargetDescriptor {
    pub(super) size: Extent2d,
    pub(super) format: Format,
    pub(super) configs: Vec<BackdropBlurConfig>,
    /// Root backdrop barriers need a shared source; isolated captures already own their source.
    pub(super) needs_root_source: bool,
    pub(super) isolated_source_indices: Vec<u32>,
    pub(super) pass_resource_set_layout: BindGroupLayoutId,
    pub(super) blur_resource_set_layout: BindGroupLayoutId,
    pub(super) frame_buffers: Vec<FrameResourceBuffers>,
    pub(super) sampler: SamplerId,
}

#[derive(Clone, Copy)]
pub(super) struct RenderTarget {
    pub(super) byte_size: u64,
    pub(super) texture: TextureId,
    pub(super) texture_view: TextureViewId,
}

fn blur_requires_isolated_source(configs: &[BackdropBlurConfig], index: u32) -> bool {
    configs
        .iter()
        .copied()
        .any(|config| config.contains_member_index(index) && config.radius() > 0.0)
}

#[inline]
fn blur_variant_level_count(config: BackdropBlurConfig) -> usize {
    if config.radius() > 0.0 { 2 } else { 1 }
}

impl BackdropBlurTargets {
    pub(super) fn config_bind_group(
        &self,
        config: BackdropBlurConfig,
        frame_resource_index: usize,
    ) -> Option<BindGroupId> {
        let variant = self
            .variants
            .iter()
            .take(self.active_variant_count)
            .find(|variant| variant.config.covers(config))?;
        variant
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
            .take(self.active_isolated_source_count)
            .find(|source| source.index == index)
            .map(|source| source.target.texture_view)
    }

    pub(super) fn isolated_source_resource_set(
        &self,
        index: u32,
        frame_resource_index: usize,
    ) -> Option<BindGroupId> {
        self.isolated_sources
            .iter()
            .take(self.active_isolated_source_count)
            .find(|source| source.index == index)?
            .pass_resource_sets
            .get(frame_resource_index)
            .copied()
    }

    pub(super) fn is_layout_compatible(
        &self,
        next: &[BackdropBlurConfig],
        next_isolated_source_indices: &[u32],
        needs_root_source: bool,
    ) -> bool {
        self.source.is_some() == needs_root_source
            && self.active_variant_count == next.len()
            && self
                .isolated_sources
                .get(..self.active_isolated_source_count)
                .is_some_and(|active| {
                    active
                        .iter()
                        .map(|source| source.index)
                        .eq(next_isolated_source_indices
                            .iter()
                            .copied()
                            .filter(|index| blur_requires_isolated_source(next, *index)))
                })
            && self
                .variants
                .iter()
                .take(self.active_variant_count)
                .zip(next)
                .all(|(variant, config)| {
                    variant.config.same_target_slot(*config)
                        && variant.levels.len() == blur_variant_level_count(*config)
                })
    }
}

pub(super) fn create_path_mask_target<D>(
    device: &mut D,
    label: &str,
    descriptor: PathMaskTargetDescriptor,
) -> Result<PathMaskTarget>
where
    D: ResourceDevice + PipelineDevice,
{
    let texture = device.create_texture(&TextureDescriptor {
        label: Some(format!("{label} path mask texture")),
        size: descriptor.size,
        mip_level_count: 1,
        format: descriptor.format,
        usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
    })?;
    let texture_view = match device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!("{label} path mask texture view")),
        texture,
        base_mip_level: 0,
        mip_level_count: 1,
        format: descriptor.format,
    }) {
        Ok(view) => view,
        Err(error) => {
            let _ = device.destroy_texture(texture);
            return Err(error.into());
        }
    };
    let mut target = PathMaskTarget {
        texture,
        texture_view,
        bind_groups: Vec::with_capacity(descriptor.frame_buffers.len()),
    };
    for (index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
        let result = device.create_bind_group(&BindGroupDescriptor {
            label: Some(format!("{label} path mask frame {index} resource set")),
            layout: descriptor.bind_group_layout,
            entries: path_resource_bindings(&buffers, texture_view, descriptor.sampler),
        });
        match result {
            Ok(set) => target.bind_groups.push(set),
            Err(error) => {
                // A Pending surface can be abandoned at any frame slot.
                // Never leak earlier descriptor sets or its physical texture.
                destroy_path_mask_target(device, target, label);
                return Err(error.into());
            }
        }
    }
    Ok(target)
}

pub(super) fn destroy_path_mask_target<D>(
    device: &mut D,
    target: PathMaskTarget,
    backend_name: &str,
) where
    D: ResourceDevice + PipelineDevice,
{
    for bind_group in target.bind_groups {
        if let Err(error) = device.destroy_bind_group(bind_group) {
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
    D: ResourceDevice + PipelineDevice,
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
    D: ResourceDevice + PipelineDevice,
{
    // This journal is invocation-local: on failure, no partially constructed
    // Filter/Root GPU allocation may escape without a retirement path.
    let mut created_textures = Vec::new();
    let mut created_resource_sets = Vec::new();
    let result = (|| -> Result<BackdropBlurTargets> {
        let mut allocate_texture = |device: &mut D, name: &str, size: Extent2d, format: Format| {
            let target = create_render_texture_target(device, name, size, format)?;
            created_textures.push(target);
            Ok::<RenderTarget, anyhow::Error>(target)
        };
        let mut allocate_set = |device: &mut D, descriptor: &BindGroupDescriptor| {
            let set = device.create_bind_group(descriptor)?;
            created_resource_sets.push(set);
            Ok::<BindGroupId, anyhow::Error>(set)
        };
        // Element captures (including their nested backdrop barriers) have their own source.
        // Allocate the shared full-window source only for a backdrop outside those captures.
        let source = if descriptor.needs_root_source {
            Some(allocate_texture(
                device,
                &format!("{label} backdrop scene color"),
                descriptor.size,
                descriptor.format,
            )?)
        } else {
            None
        };
        let mut source_pass_resource_sets = Vec::new();
        if let Some(source) = source {
            for (index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
                source_pass_resource_sets.push(allocate_set(
                    device,
                    &BindGroupDescriptor {
                        label: Some(format!(
                            "{label} backdrop scene color frame {index} resource set"
                        )),
                        layout: descriptor.pass_resource_set_layout,
                        entries: backdrop_blur_pass_resource_bindings(
                            &buffers,
                            source.texture_view,
                            descriptor.sampler,
                        ),
                    },
                )?);
            }
        }

        let mut isolated_source_indices = descriptor.isolated_source_indices;
        isolated_source_indices.sort_unstable();
        isolated_source_indices.dedup();
        isolated_source_indices.retain(|index| blur_requires_isolated_source(configs, *index));
        let mut isolated_sources = Vec::with_capacity(isolated_source_indices.len());
        for index in isolated_source_indices {
            let target = allocate_texture(
                device,
                &format!("{label} element blur {index} scene color"),
                descriptor.size,
                descriptor.format,
            )?;
            let mut pass_resource_sets = Vec::with_capacity(descriptor.frame_buffers.len());
            for (frame_index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
                pass_resource_sets.push(allocate_set(device, &BindGroupDescriptor {
                    label: Some(format!(
                        "{label} element blur {index} scene color frame {frame_index} resource set"
                    )),
                    layout: descriptor.pass_resource_set_layout,
                    entries: backdrop_blur_pass_resource_bindings(&buffers, target.texture_view, descriptor.sampler),
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
            let final_size = if config.radius() > 0.0 {
                Extent2d::new(
                    descriptor.size.width().div_ceil(downsample).max(1),
                    descriptor.size.height().div_ceil(downsample).max(1),
                )?
            } else {
                // A direct compositor is a pixel cache, not a low-pass filter. Keep it at native
                // resolution even if stale quality metadata happens to carry a downsample factor.
                descriptor.size
            };
            let pass_sizes = if config.radius() > 0.0 {
                [horizontal_size, final_size]
            } else {
                [final_size, final_size]
            };
            let pass_count = blur_variant_level_count(config);
            let mut levels = Vec::with_capacity(pass_count);
            for (pass_index, target_size) in pass_sizes.into_iter().take(pass_count).enumerate() {
                let target = allocate_texture(
                    device,
                    &format!("{label} backdrop variant {variant_index} target {pass_index}"),
                    target_size,
                    descriptor.format,
                )?;
                let mut pass_resource_sets = Vec::with_capacity(descriptor.frame_buffers.len());
                for (frame_index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
                    pass_resource_sets.push(allocate_set(device, &BindGroupDescriptor {
                        label: Some(format!("{label} filter target {variant_index}/{pass_index} frame {frame_index}")),
                        layout: descriptor.pass_resource_set_layout,
                        entries: backdrop_blur_pass_resource_bindings(
                            &buffers, target.texture_view, descriptor.sampler,
                        ),
                    })?);
                }
                levels.push(BackdropBlurLevelTarget {
                    byte_size: target.byte_size,
                    texture: target.texture,
                    texture_view: target.texture_view,
                    pass_resource_sets,
                });
            }

            let mut target_resource_sets = Vec::with_capacity(descriptor.frame_buffers.len());
            for (frame_index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
                let source_texture_view = levels
                    .last()
                    .expect("every filter owns at least one target")
                    .texture_view;
                target_resource_sets.push(allocate_set(
                    device,
                    &BindGroupDescriptor {
                        label: Some(format!(
                            "{label} filter composite {variant_index} frame {frame_index}"
                        )),
                        layout: descriptor.blur_resource_set_layout,
                        entries: backdrop_blur_resource_bindings(
                            &buffers,
                            source_texture_view,
                            descriptor.sampler,
                        ),
                    },
                )?);
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
            active_isolated_source_count: isolated_sources.len(),
            isolated_sources,
            variants,
            active_variant_count: configs.len(),
        })
    })();
    if result.is_err() {
        // ResourceSets must be retired before any TextureView they sample.
        // Roll back in reverse creation order, never touching the live chain.
        for set in created_resource_sets.into_iter().rev() {
            if let Err(error) = device.destroy_bind_group(set) {
                log::debug!("failed to roll back {label} filter resource set: {error}");
            }
        }
        for target in created_textures.into_iter().rev() {
            destroy_render_texture_target(device, target, "filter allocation rollback");
        }
    }
    result
}

pub(super) fn destroy_backdrop_blur_target_chain<D>(
    device: &mut D,
    targets: BackdropBlurTargets,
    backend_name: &str,
) where
    D: ResourceDevice + PipelineDevice,
{
    for bind_group in targets.source_pass_resource_sets {
        if let Err(error) = device.destroy_bind_group(bind_group) {
            log::debug!(
                "failed to destroy {backend_name} backdrop blur source resource set: {error}"
            );
        }
    }
    for source in targets.isolated_sources {
        for bind_group in source.pass_resource_sets {
            if let Err(error) = device.destroy_bind_group(bind_group) {
                log::debug!(
                    "failed to destroy {backend_name} element blur source resource set: {error}"
                );
            }
        }
        destroy_render_texture_target(device, source.target, backend_name);
    }
    let mut physical_targets = Vec::new();
    let mut gathered_textures = FxHashSet::default();
    for variant in targets.variants {
        for set in variant.target_resource_sets {
            if let Err(error) = device.destroy_bind_group(set) {
                log::debug!("failed to retire {backend_name} filter composite set: {error}");
            }
        }
        for level in variant.levels {
            for set in level.pass_resource_sets {
                if let Err(error) = device.destroy_bind_group(set) {
                    log::debug!("failed to retire {backend_name} filter pass set: {error}");
                }
            }
            if gathered_textures.insert(level.texture) {
                physical_targets.push(RenderTarget {
                    byte_size: level.byte_size,
                    texture: level.texture,
                    texture_view: level.texture_view,
                });
            }
        }
    }
    for target in physical_targets {
        destroy_render_texture_target(device, target, backend_name);
    }
    if let Some(source) = targets.source {
        destroy_render_texture_target(device, source, backend_name);
    }
}

pub(super) fn create_depth_target<D>(
    device: &mut D,
    label: &str,
    size: Extent2d,
) -> Result<(TextureId, TextureViewId)>
where
    D: ResourceDevice,
{
    let texture = create_depth_texture(device, label, size)?;
    let texture_view = match device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!("{label} depth texture view")),
        texture,
        base_mip_level: 0,
        mip_level_count: 1,
        format: Format::Depth32Float,
    }) {
        Ok(view) => view,
        Err(error) => {
            let _ = device.destroy_texture(texture);
            return Err(error.into());
        }
    };
    Ok((texture, texture_view))
}

pub(super) fn destroy_depth_target<D>(
    device: &mut D,
    texture: TextureId,
    texture_view: TextureViewId,
    backend_name: &str,
) where
    D: ResourceDevice,
{
    if let Err(error) = device.destroy_texture_view(texture_view) {
        log::debug!("failed to destroy {backend_name} old path mask texture view: {error}");
    }
    if let Err(error) = device.destroy_texture(texture) {
        log::debug!("failed to destroy {backend_name} old path mask texture: {error}");
    }
}

pub(super) fn create_render_texture_target<D>(
    device: &mut D,
    label: &str,
    size: Extent2d,
    format: Format,
) -> Result<RenderTarget>
where
    D: ResourceDevice + PipelineDevice,
{
    let texture = device.create_texture(&TextureDescriptor {
        label: Some(format!("{label} texture")),
        size,
        mip_level_count: 1,
        format,
        usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
    })?;
    let texture_view = match device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!("{label} texture view")),
        texture,
        base_mip_level: 0,
        mip_level_count: 1,
        format,
    }) {
        Ok(view) => view,
        Err(error) => {
            // A failed View allocation still leaves the Texture alive.
            if let Err(rollback_error) = device.destroy_texture(texture) {
                log::debug!(
                    "failed to roll back {label} texture after view failure: {rollback_error}"
                );
            }
            return Err(error.into());
        }
    };
    Ok(RenderTarget {
        byte_size: u64::from(size.width())
            * u64::from(size.height())
            * u64::from(format.bytes_per_pixel()),
        texture,
        texture_view,
    })
}

pub(super) fn destroy_render_texture_target<D>(
    device: &mut D,
    target: RenderTarget,
    backend_name: &str,
) where
    D: ResourceDevice + PipelineDevice,
{
    if let Err(error) = device.destroy_texture_view(target.texture_view) {
        log::debug!("failed to destroy {backend_name} texture target view: {error}");
    }
    if let Err(error) = device.destroy_texture(target.texture) {
        log::debug!("failed to destroy {backend_name} texture target: {error}");
    }
}
