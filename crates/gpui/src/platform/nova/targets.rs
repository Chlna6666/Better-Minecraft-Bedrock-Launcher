use super::*;

mod layout;

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
    /// Shared scene color for root backdrop barriers; absent for isolated-only captures.
    pub(super) source: Option<RenderTarget>,
    pub(super) source_pass_resource_sets: Vec<ResourceSetId>,
    /// Per-element sources. Each CSS blur group starts from a clean attachment so one group's
    /// content can never become the backdrop source of a sibling group.
    pub(super) isolated_sources: Vec<IsolatedBlurSource>,
    /// Only the active prefix participates in the current filter graph. Spare
    /// scratch targets remain GPU-resident across popup insertion/removal.
    pub(super) active_isolated_source_count: usize,
    pub(super) variants: Vec<BackdropBlurVariantTargets>,
    /// Live prefix; spare popup targets remain allocated until the chain is replaced or trimmed.
    pub(super) active_variant_count: usize,
    /// The window-color target follows its role when popup slots appear/disappear.
    pub(super) retained_root_variant: Option<usize>,
    /// Physical allocation owner for the synthetic Root Color. The matching
    /// zero-radius variant is only a logical Filter/Render Graph binding: it
    /// borrows this Texture/View but stores no Root ResourceSet IDs or ownership.
    /// A later phase can move this owner out of the filter chain without
    /// changing the native texture or its consumers.
    pub(super) root_color_target: Option<RootColorTarget>,
}

/// Unique native lifetime authority for the full-resolution Root Color.
/// Filter Graph consumers resolve its descriptor IDs directly here;
/// only this owner retires the allocation and its per-frame ResourceSets.
#[derive(Clone)]
pub(super) struct RootColorTarget {
    pub(super) target: RenderTarget,
    pub(super) pass_resource_sets: Vec<ResourceSetId>,
    pub(super) composite_resource_sets: Vec<ResourceSetId>,
}

impl BackdropBlurTargets {
    /// The resident full-window color texture is identified by its actual GPU
    /// handle, never by the position of a temporary flattened Blur index.
    /// A popup may move this variant without moving its physical pixels.
    pub(super) fn retained_root_texture_view(&self) -> Option<TextureViewId> {
        self.root_color_texture_identity().map(|(_, view)| view)
    }

    /// Physical identity for the synthetic Root Color (not its flattened
    /// Capture index). Only a one-level, unfiltered active native target is a
    /// valid resident color surface. The view and underlying allocation must
    /// both match the last successful GPU submission.
    pub(super) fn root_color_texture_identity(&self) -> Option<(TextureId, TextureViewId)> {
        let index = self.retained_root_variant?;
        if index >= self.active_variant_count {
            return None;
        }
        let variant = self.variants.get(index)?;
        if variant.config.radius() != 0.0 {
            return None;
        }
        let [level] = variant.levels.as_slice() else {
            return None;
        };
        let root_color = self.root_color_target.as_ref()?;
        if root_color.target.texture != level.texture
            || root_color.target.texture_view != level.texture_view
        {
            return None;
        }
        Some((root_color.target.texture, root_color.target.texture_view))
    }

    /// Changing a Root role is a physical ownership change even if all
    /// filter configurations happen to retain identical member indices.
    /// Prevent the fast path from silently promoting ordinary Filter scratch
    /// into a resident Root without first allocating its dedicated owner.
    pub(super) fn root_capture_is_compatible(
        &self,
        next: &[BackdropBlurConfig],
        root_capture_index: Option<u32>,
    ) -> bool {
        let next_root = root_capture_index.and_then(|index| {
            next.iter().position(|config| {
                config.contains_member_index(index) && config.radius() == 0.0
            })
        });
        if root_capture_index.is_some() && next_root.is_none() {
            return false;
        }
        self.retained_root_variant == next_root
            && (next_root.is_none()
                || (self.root_color_target.is_some()
                    && self.root_color_texture_identity().is_some()))
    }

    /// The native Root Color owns its pixels independently of warmed, inactive
    /// filter scratch storage. Capacity retained after closing a Gaussian
    /// dropdown does not change the root's content or physical identity.
    /// Only LIVE filter variants participate in the unfiltered-root proof.
    pub(super) fn can_preserve_root_color_capture(&self) -> bool {
        let Some(index) = self.retained_root_variant else { return false };
        let Some((root_texture, root_view)) = self.root_color_texture_identity() else {
            return false;
        };
        if self.source.is_some()
            || self.variants.iter().take(self.active_variant_count)
                .any(|variant| variant.config.radius() != 0.0)
        {
            return false;
        }
        // An aliased texture cannot be treated as independent Root residency.
        // Compare all allocated (including spare) views, not only active slots.
        !self.variants.iter().enumerate().any(|(other_index, variant)| {
            other_index != index
                && variant.levels.iter().any(|target| {
                    target.texture == root_texture || target.texture_view == root_view
                })
        })
            && !self.isolated_sources.iter().any(|source| {
                source.target.texture == root_texture
                    || source.target.texture_view == root_view
            })
    }

    /// Only a single-member zero-filter output has an independently initialized texture.
    pub(super) fn exclusive_zero_radius_capture_view(&self, index: u32) -> Option<TextureViewId> {
        let mut candidates = self.variants.iter().take(self.active_variant_count)
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
    pub(super) pass_resource_sets: Vec<ResourceSetId>,
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
    pub(super) target_resource_sets: Vec<ResourceSetId>,
}

#[derive(Clone)]
pub(super) struct BackdropBlurLevelTarget {
    pub(super) byte_size: u64,
    pub(super) texture: TextureId,
    pub(super) texture_view: TextureViewId,
    pub(super) pass_resource_sets: Vec<ResourceSetId>,
}

pub(super) struct BackdropBlurTargetDescriptor {
    pub(super) size: Extent2d,
    pub(super) format: Format,
    pub(super) configs: Vec<BackdropBlurConfig>,
    /// Root backdrop barriers need a shared source; isolated captures already own their source.
    pub(super) needs_root_source: bool,
    pub(super) isolated_source_indices: Vec<u32>,
    /// Logical root capture for this Scene, never a Gaussian filter owner.
    pub(super) root_color_capture_index: Option<u32>,
    pub(super) pass_resource_set_layout: ResourceSetLayoutId,
    pub(super) blur_resource_set_layout: ResourceSetLayoutId,
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
    pub(super) fn resource_set_for_config(
        &self,
        config: BackdropBlurConfig,
        frame_resource_index: usize,
    ) -> Option<ResourceSetId> {
        let variant = self.variants
            .iter()
            .take(self.active_variant_count)
            .find(|variant| variant.config.covers(config))?;
        if let (Some(root), [level]) = (self.root_color_target.as_ref(), variant.levels.as_slice()) {
            if root.target.texture == level.texture && root.target.texture_view == level.texture_view {
                return root.composite_resource_sets.get(frame_resource_index).copied();
            }
        }
        variant.target_resource_sets.get(frame_resource_index).copied()
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
    ) -> Option<ResourceSetId> {
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
            && self.isolated_sources
                .get(..self.active_isolated_source_count)
                .is_some_and(|active| active.iter().map(|source| source.index).eq(
                    next_isolated_source_indices
                        .iter()
                        .copied()
                        .filter(|index| blur_requires_isolated_source(next, *index)),
                ))
            && self.variants.iter().take(self.active_variant_count).zip(next).all(|(variant, config)| {
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
    D: BackendResources + BackendPipelines,
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
    let texture_view = device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!("{label} path mask texture view")),
        texture,
        base_mip_level: 0,
        mip_level_count: 1,
        format: descriptor.format,
    })?;
    let mut resource_sets = Vec::with_capacity(descriptor.frame_buffers.len());
    for (index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
        resource_sets.push(device.create_resource_set(&ResourceSetDescriptor {
            label: Some(format!("{label} path mask frame {index} resource set")),
            layout: descriptor.resource_set_layout,
            bindings: path_resource_bindings(&buffers, texture_view, descriptor.sampler),
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
        let mut allocate_set = |device: &mut D, descriptor: &ResourceSetDescriptor| {
            let set = device.create_resource_set(descriptor)?;
            created_resource_sets.push(set);
            Ok::<ResourceSetId, anyhow::Error>(set)
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
                source_pass_resource_sets.push(allocate_set(device, &ResourceSetDescriptor {
                    label: Some(format!(
                        "{label} backdrop scene color frame {index} resource set"
                    )),
                    layout: descriptor.pass_resource_set_layout,
                    bindings: backdrop_blur_pass_resource_bindings(
                        &buffers,
                        source.texture_view,
                        descriptor.sampler,
                    ),
                })?);
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
                pass_resource_sets.push(allocate_set(device, &ResourceSetDescriptor {
                    label: Some(format!(
                        "{label} element blur {index} scene color frame {frame_index} resource set"
                    )),
                    layout: descriptor.pass_resource_set_layout,
                    bindings: backdrop_blur_pass_resource_bindings(&buffers, target.texture_view, descriptor.sampler),
                })?);
            }
            isolated_sources.push(IsolatedBlurSource {
                index,
                target,
                pass_resource_sets,
            });
        }

        // Resolve the Root role before creating any filter scratch. Allocate its
        // native texture ONCE, separately from the Gaussian/zero-radius variants,
        // and lend the view to the matching logical compositor slot.
        let root_variant = descriptor.root_color_capture_index.and_then(|root| {
            configs.iter().position(|config| config.contains_member_index(root)
                && config.radius() == 0.0)
        });
        let root_color_target = if root_variant.is_some() {
            let target = allocate_texture(
                device,
                &format!("{label} independent root color"),
                descriptor.size,
                descriptor.format,
            )?;
            let mut pass_resource_sets = Vec::with_capacity(descriptor.frame_buffers.len());
            let mut composite_resource_sets = Vec::with_capacity(descriptor.frame_buffers.len());
            for (index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
                pass_resource_sets.push(allocate_set(device, &ResourceSetDescriptor {
                    label: Some(format!("{label} root color pass frame {index} resource set")),
                    layout: descriptor.pass_resource_set_layout,
                    bindings: backdrop_blur_pass_resource_bindings(
                        &buffers, target.texture_view, descriptor.sampler,
                    ),
                })?);
                composite_resource_sets.push(allocate_set(device, &ResourceSetDescriptor {
                    label: Some(format!("{label} root color composite frame {index} resource set")),
                    layout: descriptor.blur_resource_set_layout,
                    bindings: backdrop_blur_resource_bindings(
                        &buffers, target.texture_view, descriptor.sampler,
                    ),
                })?);
            }
            Some(RootColorTarget { target, pass_resource_sets, composite_resource_sets })
        } else {
            None
        };
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
                let target = if root_variant == Some(variant_index) {
                    // This slot describes (but does not own) the Root texture.
                    // The only permitted root is a full-resolution, single-pass,
                    // zero-radius capture.
                    debug_assert_eq!(pass_count, 1);
                    root_color_target.as_ref().expect("root texture was allocated").target
                } else {
                    allocate_texture(
                        device,
                        &format!("{label} backdrop variant {variant_index} target {pass_index}"),
                        target_size,
                        descriptor.format,
                    )?
                };
                // A Root logical variant has no descriptor ownership or mirror.
                // The native RootColorTarget is consulted by every Graph consumer.
                let pass_resource_sets = if root_variant == Some(variant_index) {
                    Vec::new()
                } else {
                    let mut sets = Vec::with_capacity(descriptor.frame_buffers.len());
                    for (frame_index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
                        sets.push(allocate_set(device, &ResourceSetDescriptor {
                            label: Some(format!(
                                "{label} backdrop variant {variant_index} target {pass_index} frame {frame_index} resource set"
                            )),
                            layout: descriptor.pass_resource_set_layout,
                            bindings: backdrop_blur_pass_resource_bindings(
                                &buffers, target.texture_view, descriptor.sampler,
                            ),
                        })?);
                    }
                    sets
                };
                levels.push(BackdropBlurLevelTarget {
                    byte_size: target.byte_size,
                    texture: target.texture,
                    texture_view: target.texture_view,
                    pass_resource_sets,
                });
            }

            let target_resource_sets = if root_variant == Some(variant_index) {
                Vec::new()
            } else {
                let mut sets = Vec::with_capacity(descriptor.frame_buffers.len());
                for (frame_index, buffers) in descriptor.frame_buffers.iter().copied().enumerate() {
                    let source_texture_view = levels
                        .last()
                        .expect("every blur variant owns at least one target")
                        .texture_view;
                    sets.push(allocate_set(device, &ResourceSetDescriptor {
                        label: Some(format!(
                            "{label} backdrop blur variant {variant_index} frame {frame_index} resource set"
                        )),
                        layout: descriptor.blur_resource_set_layout,
                        bindings: backdrop_blur_resource_bindings(
                            &buffers, source_texture_view, descriptor.sampler,
                        ),
                    })?);
                }
                sets
            };
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
            retained_root_variant: root_variant,
            root_color_target,
        })
    })();
    if result.is_err() {
        // ResourceSets must be retired before any TextureView they sample.
        // Roll back in reverse creation order, never touching the live chain.
        for set in created_resource_sets.into_iter().rev() {
            if let Err(error) = device.destroy_resource_set(set) {
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
    let root_color_target = targets.root_color_target;
    let mut gathered_textures = FxHashSet::default();
    let mut physical_targets = Vec::new();
    for variant in targets.variants {
        let borrowed_root = root_color_target.as_ref().is_some_and(|root| {
            variant.levels.len() == 1
                && variant.levels[0].texture == root.target.texture
        });
        if !borrowed_root {
            for resource_set in variant.target_resource_sets {
                if let Err(error) = device.destroy_resource_set(resource_set) {
                    log::debug!(
                        "failed to destroy {backend_name} backdrop blur target resource set: {error}"
                    );
                }
            }
        }
        for target in variant.levels {
            if !root_color_target.as_ref().is_some_and(|root| {
                root.target.texture == target.texture
            }) {
                for resource_set in target.pass_resource_sets {
                    if let Err(error) = device.destroy_resource_set(resource_set) {
                        log::debug!(
                            "failed to destroy {backend_name} backdrop blur level resource set: {error}"
                        );
                    }
                }
            }
            // Collect unique physical targets. They must not be destroyed
            // while any other variant's descriptor set still references them.
            if root_color_target.as_ref().is_some_and(|root| root.target.texture == target.texture) {
                // Root's descriptors and Texture/View are retired only by its
                // independent native owner, not this logical Filter variant.
                continue;
            }
            if gathered_textures.insert(target.texture) {
                physical_targets.push(RenderTarget {
                    byte_size: target.byte_size,
                    texture: target.texture,
                    texture_view: target.texture_view,
                });
            }
        }
    }
    for target in physical_targets {
        destroy_render_texture_target(device, target, backend_name);
    }
    // No filter/scratch target path may free this Texture/View; every GPU
    // submission using the old chain must already have retired at this point.
    if let Some(root_color) = root_color_target {
        for resource_set in root_color.composite_resource_sets
            .into_iter().chain(root_color.pass_resource_sets)
        {
            if let Err(error) = device.destroy_resource_set(resource_set) {
                log::debug!(
                    "failed to destroy {backend_name} independent root color resource set: {error}"
                );
            }
        }
        destroy_render_texture_target(device, root_color.target, backend_name);
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
    D: BackendResources,
{
    let texture = create_depth_texture(device, label, size)?;
    let texture_view = device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!("{label} depth texture view")),
        texture,
        base_mip_level: 0,
        mip_level_count: 1,
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
) -> Result<RenderTarget>
where
    D: BackendResources + BackendPipelines,
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
                log::debug!("failed to roll back {label} texture after view failure: {rollback_error}");
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

fn destroy_render_texture_target<D>(device: &mut D, target: RenderTarget, backend_name: &str)
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
