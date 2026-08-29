use super::*;

impl NovaRenderer {
    pub(crate) fn resize(&mut self, size: Size<DevicePixels>) -> Result<()> {
        let width = size.width.0.max(1) as u32;
        let height = size.height.0.max(1) as u32;
        let next_size = DrawableSize { width, height };
        if next_size == self.current_size {
            return Ok(());
        }
        self.prepare_for_resize()?;
        let target_size = Extent2d::new(width, height)?;
        let surface_config = SurfaceConfig {
            size: target_size,
            format: self.surface_format,
            present_mode: self.present_mode,
            alpha_mode: self.surface_alpha.swapchain_mode,
        };
        let path_mask_target_descriptor = self.path_mask_target_descriptor(target_size);
        let backdrop_blur_target_descriptor = self.backdrop_blur_target_descriptor(target_size);
        let old_path_mask_target = self.current_path_mask_target();
        let old_backdrop_blur_targets = self.current_backdrop_blur_targets();
        let old_depth_texture = self.depth_texture;
        let old_depth_texture_view = self.depth_texture_view;
        let (next_path_mask_target, next_backdrop_blur_targets): (
            PathMaskTarget,
            Option<BackdropBlurTargets>,
        ) = match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                resize_dx12_swapchain(device, self.swapchain, surface_config)?;
                let next_path_mask_target =
                    create_path_mask_target(device, "gpui nova dx12", path_mask_target_descriptor)?;
                let next_backdrop_blur_targets = if old_backdrop_blur_targets.is_some() {
                    Some(create_backdrop_blur_target_chain(
                        device,
                        "gpui nova dx12",
                        backdrop_blur_target_descriptor,
                    )?)
                } else {
                    None
                };
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova dx12", target_size)?;
                destroy_path_mask_target(device, old_path_mask_target, "DX12");
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "DX12");
                }
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "DX12");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (next_path_mask_target, next_backdrop_blur_targets)
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => {
                device.resize_swapchain(self.swapchain, width, height)?;
                let next_path_mask_target = create_path_mask_target(
                    device,
                    "gpui nova metal",
                    path_mask_target_descriptor,
                )?;
                let next_backdrop_blur_targets = if old_backdrop_blur_targets.is_some() {
                    Some(create_backdrop_blur_target_chain(
                        device,
                        "gpui nova metal",
                        backdrop_blur_target_descriptor,
                    )?)
                } else {
                    None
                };
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova metal", target_size)?;
                destroy_path_mask_target(device, old_path_mask_target, "Metal");
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "Metal");
                }
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "Metal");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (next_path_mask_target, next_backdrop_blur_targets)
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => {
                resize_vulkan_swapchain(device, self.swapchain, surface_config)?;
                let next_path_mask_target = create_path_mask_target(
                    device,
                    "gpui nova vulkan",
                    path_mask_target_descriptor,
                )?;
                let next_backdrop_blur_targets = if old_backdrop_blur_targets.is_some() {
                    Some(create_backdrop_blur_target_chain(
                        device,
                        "gpui nova vulkan",
                        backdrop_blur_target_descriptor,
                    )?)
                } else {
                    None
                };
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova vulkan", target_size)?;
                destroy_path_mask_target(device, old_path_mask_target, "Vulkan");
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "Vulkan");
                }
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "Vulkan");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (next_path_mask_target, next_backdrop_blur_targets)
            }
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {
                anyhow::bail!("nova-gfx renderer requires an explicit nova-gfx backend feature")
            }
        };
        self.path_texture = next_path_mask_target.texture;
        self.path_texture_view = next_path_mask_target.texture_view;
        self.update_path_mask_resource_sets(&next_path_mask_target.resource_sets)?;
        self.backdrop_blur_targets = next_backdrop_blur_targets;
        self.invalidate_backdrop_blur_cache();
        self.activate_frame_resources(self.current_frame_resource_index)?;
        self.surface_config = surface_config;
        self.current_size = next_size;
        self.swapchain_warmup_frames = SWAPCHAIN_WARMUP_FRAME_COUNT;
        if let Err(error) = self
            .backend
            .set_swapchain_content_stretch(self.swapchain, None)
        {
            log::warn!("failed to reset nova-gfx live-resize stretch: {error:#}");
        }
        Ok(())
    }

    /// Stretches the previous frame over the client area while a native resize
    /// is pending, mirroring the scaling HWND flip swapchains get from DXGI.
    ///
    /// Compositor-backed swapchains (DirectComposition) do not scale on their
    /// own, so the platform layer calls this synchronously from the resize
    /// event to keep the old frame covering the new client size until the
    /// next frame applies the resize to the swapchain buffers.
    pub(crate) fn stretch_surface_for_pending_resize(&mut self, size: Size<DevicePixels>) {
        let width = size.width.0.max(1) as u32;
        let height = size.height.0.max(1) as u32;
        if width == self.current_size.width && height == self.current_size.height {
            return;
        }
        let scale = [
            width as f32 / self.current_size.width.max(1) as f32,
            height as f32 / self.current_size.height.max(1) as f32,
        ];
        if let Err(error) = self
            .backend
            .set_swapchain_content_stretch(self.swapchain, Some(scale))
        {
            log::warn!("failed to stretch nova-gfx surface during live resize: {error:#}");
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub(crate) fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        self.pending_drawable_size = Some(size);
    }

    /// Applies the newest coalesced drawable size only when the GPU/swapchain can be replaced
    /// immediately. Returning `false` tells the caller to skip rendering this frame; importantly,
    /// no new old-size GPU work is submitted while resize backpressure is active.
    pub(super) fn apply_pending_drawable_size(&mut self) -> Result<bool> {
        let Some(size) = self.pending_drawable_size.take() else {
            return Ok(true);
        };
        match self.try_resize(size) {
            Ok(true) => Ok(true),
            Ok(false) => {
                self.pending_drawable_size = Some(size);
                Ok(false)
            }
            Err(error) => {
                self.pending_drawable_size = Some(size);
                Err(error)
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub(crate) fn has_pending_drawable_size(&self) -> bool {
        self.pending_drawable_size.is_some()
    }

    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub(crate) fn viewport_size(&self) -> Size<DevicePixels> {
        if let Some(size) = self.pending_drawable_size {
            return size;
        }
        Size {
            width: DevicePixels(self.current_size.width as i32),
            height: DevicePixels(self.current_size.height as i32),
        }
    }

    pub(crate) fn update_transparency(&mut self, is_transparent: bool) {
        let previous_alpha = self.surface_alpha;
        let next_alpha = match self.alpha_state_for_current_backend_transparency(is_transparent) {
            Ok(alpha) => alpha,
            Err(error) => {
                log::warn!(
                    "failed to resolve nova-gfx surface alpha mode: backend={} error={error:#}",
                    self.backend.label(),
                );
                return;
            }
        };
        if self.surface_alpha == next_alpha {
            return;
        }
        if let Err(error) = self.reconfigure_surface_alpha(next_alpha) {
            log::warn!(
                concat!(
                    "failed to reconfigure nova-gfx surface alpha mode: backend={} ",
                    "swapchain=index:{} generation:{} old_swapchain={:?} old_output={:?} ",
                    "new_swapchain={:?} new_output={:?} error={:#}"
                ),
                self.backend.label(),
                self.swapchain.index(),
                self.swapchain.generation(),
                previous_alpha.swapchain_mode,
                previous_alpha.output_mode,
                next_alpha.swapchain_mode,
                next_alpha.output_mode,
                error
            );
        }
    }

    fn alpha_state_for_window_transparency(is_transparent: bool) -> SurfaceAlphaState {
        SurfaceAlphaState::for_window_transparency(is_transparent)
    }

    pub(in crate::platform::nova) fn alpha_state_for_window_transparency_on_backend(
        _backend: RendererBackend,
        is_transparent: bool,
    ) -> SurfaceAlphaState {
        Self::alpha_state_for_window_transparency(is_transparent)
    }

    fn alpha_state_for_current_backend_transparency(
        &self,
        is_transparent: bool,
    ) -> Result<SurfaceAlphaState> {
        let requested = Self::alpha_state_for_window_transparency(is_transparent);
        match &self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(_) => Ok(requested),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(_) => Ok(requested),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => Ok(SurfaceAlphaState::new(
                device.resolve_surface_alpha_mode(self.surface, requested.swapchain_mode)?,
            )),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => Ok(requested),
        }
    }

    fn reconfigure_surface_alpha(&mut self, alpha: SurfaceAlphaState) -> Result<()> {
        self.wait_for_pending_submissions()?;
        if self.surface_alpha.swapchain_mode == alpha.swapchain_mode {
            log::debug!(
                concat!(
                    "nova-gfx surface alpha output changed without swapchain reconfigure: ",
                    "backend={} swapchain=index:{} generation:{} swapchain_alpha={:?} ",
                    "old_output={:?} new_output={:?}"
                ),
                self.backend.label(),
                self.swapchain.index(),
                self.swapchain.generation(),
                alpha.swapchain_mode,
                self.surface_alpha.output_mode,
                alpha.output_mode,
            );
            self.surface_alpha = alpha;
            return Ok(());
        }

        let config = SurfaceConfig {
            size: Extent2d::new(self.current_size.width, self.current_size.height)?,
            format: self.surface_format,
            present_mode: self.present_mode,
            alpha_mode: alpha.swapchain_mode,
        };
        let path_mask_target_descriptor = self.path_mask_target_descriptor(config.size);
        let backdrop_blur_target_descriptor = self.backdrop_blur_target_descriptor(config.size);
        let old_path_mask_target = self.current_path_mask_target();
        let old_backdrop_blur_targets = self.current_backdrop_blur_targets();
        let old_depth_texture = self.depth_texture;
        let old_depth_texture_view = self.depth_texture_view;
        let (next_path_mask_target, next_backdrop_blur_targets): (
            PathMaskTarget,
            Option<BackdropBlurTargets>,
        ) = match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                self.swapchain =
                    recreate_dx12_swapchain_for_config(device, self.swapchain, config)?;
                let next_path_mask_target =
                    create_path_mask_target(device, "gpui nova dx12", path_mask_target_descriptor)?;
                let next_backdrop_blur_targets = if old_backdrop_blur_targets.is_some() {
                    Some(create_backdrop_blur_target_chain(
                        device,
                        "gpui nova dx12",
                        backdrop_blur_target_descriptor,
                    )?)
                } else {
                    None
                };
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova dx12", config.size)?;
                destroy_path_mask_target(device, old_path_mask_target, "DX12");
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "DX12");
                }
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "DX12");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (next_path_mask_target, next_backdrop_blur_targets)
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => {
                device.resize_swapchain(
                    self.swapchain,
                    config.size.width(),
                    config.size.height(),
                )?;
                let next_path_mask_target = create_path_mask_target(
                    device,
                    "gpui nova metal",
                    path_mask_target_descriptor,
                )?;
                let next_backdrop_blur_targets = if old_backdrop_blur_targets.is_some() {
                    Some(create_backdrop_blur_target_chain(
                        device,
                        "gpui nova metal",
                        backdrop_blur_target_descriptor,
                    )?)
                } else {
                    None
                };
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova metal", config.size)?;
                destroy_path_mask_target(device, old_path_mask_target, "Metal");
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "Metal");
                }
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "Metal");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (next_path_mask_target, next_backdrop_blur_targets)
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => {
                device.reconfigure_swapchain(self.swapchain, config)?;
                let next_path_mask_target = create_path_mask_target(
                    device,
                    "gpui nova vulkan",
                    path_mask_target_descriptor,
                )?;
                let next_backdrop_blur_targets = if old_backdrop_blur_targets.is_some() {
                    Some(create_backdrop_blur_target_chain(
                        device,
                        "gpui nova vulkan",
                        backdrop_blur_target_descriptor,
                    )?)
                } else {
                    None
                };
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova vulkan", config.size)?;
                destroy_path_mask_target(device, old_path_mask_target, "Vulkan");
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "Vulkan");
                }
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "Vulkan");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (next_path_mask_target, next_backdrop_blur_targets)
            }
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {
                anyhow::bail!("nova-gfx renderer requires an explicit nova-gfx backend feature")
            }
        };
        self.path_texture = next_path_mask_target.texture;
        self.path_texture_view = next_path_mask_target.texture_view;
        self.update_path_mask_resource_sets(&next_path_mask_target.resource_sets)?;
        self.backdrop_blur_targets = next_backdrop_blur_targets;
        self.invalidate_backdrop_blur_cache();
        self.swapchain_warmup_frames = SWAPCHAIN_WARMUP_FRAME_COUNT;
        self.activate_frame_resources(self.current_frame_resource_index)?;
        self.surface_alpha = alpha;
        Ok(())
    }

    fn current_path_mask_target(&self) -> PathMaskTarget {
        PathMaskTarget {
            texture: self.path_texture,
            texture_view: self.path_texture_view,
            resource_sets: self
                .frame_resources
                .iter()
                .map(|resources| resources.path_resource_set)
                .collect(),
        }
    }

    pub(super) fn current_backdrop_blur_targets(&self) -> Option<BackdropBlurTargets> {
        self.backdrop_blur_targets.clone()
    }

    fn path_mask_target_descriptor(&self, size: Extent2d) -> PathMaskTargetDescriptor {
        PathMaskTargetDescriptor {
            size,
            format: self.surface_format,
            resource_set_layout: self.path_resource_set_layout,
            frame_buffers: self.frame_resource_buffers(),
            sampler: self.atlas_sampler,
        }
    }

    pub(super) fn backdrop_blur_target_descriptor(
        &self,
        size: Extent2d,
    ) -> BackdropBlurTargetDescriptor {
        let mut isolated_source_indices: Vec<_> = self
            .frame_upload
            .blur_content_ranges()
            .into_iter()
            .map(|range| range.index)
            .collect();
        isolated_source_indices.sort_unstable();
        isolated_source_indices.dedup();
        BackdropBlurTargetDescriptor {
            size,
            format: self.surface_format,
            configs: self.frame_upload.backdrop_blur_configs().to_vec(),
            isolated_source_indices,
            pass_resource_set_layout: self.backdrop_blur_pass_resource_set_layout,
            blur_resource_set_layout: self.backdrop_blur_resource_set_layout,
            frame_buffers: self.frame_resource_buffers(),
            sampler: self.atlas_sampler,
        }
    }
}
