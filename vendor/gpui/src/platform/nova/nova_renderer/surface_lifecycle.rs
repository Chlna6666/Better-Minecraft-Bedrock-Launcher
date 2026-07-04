use super::*;

impl NovaRenderer {
    pub(crate) fn resize(&mut self, size: Size<DevicePixels>) -> Result<()> {
        self.prepare_for_resize()?;
        let width = size.width.0.max(1) as u32;
        let height = size.height.0.max(1) as u32;
        let next_size = DrawableSize { width, height };
        if next_size == self.current_size {
            return Ok(());
        }
        let target_size = Extent2d::new(width, height)?;
        let surface_config = SurfaceConfig {
            size: target_size,
            format: self.surface_format,
            present_mode: self.present_mode,
            alpha_mode: self.surface_alpha.swapchain_mode,
        };
        let path_mask_target_descriptor = self.path_mask_target_descriptor(target_size);
        let backdrop_blur_target_descriptor = self.backdrop_blur_target_descriptor(target_size);
        let present_cache_target_descriptor = self.present_cache_target_descriptor(target_size);
        let old_path_mask_target = self.current_path_mask_target();
        let old_backdrop_blur_targets = self.current_backdrop_blur_targets();
        let old_present_cache_target = self.current_present_cache_target();
        let old_depth_texture = self.depth_texture;
        let old_depth_texture_view = self.depth_texture_view;
        let (next_path_mask_target, next_backdrop_blur_targets, next_present_cache_target): (
            NovaPathMaskTarget,
            Option<NovaBackdropBlurTargets>,
            NovaPresentCacheTarget,
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
                let next_present_cache_target = create_present_cache_target(
                    device,
                    "gpui nova dx12",
                    present_cache_target_descriptor,
                )?;
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova dx12", target_size)?;
                destroy_path_mask_target(device, old_path_mask_target, "DX12");
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "DX12");
                }
                destroy_present_cache_target(device, old_present_cache_target, "DX12");
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "DX12");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (
                    next_path_mask_target,
                    next_backdrop_blur_targets,
                    next_present_cache_target,
                )
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
                let next_present_cache_target = create_present_cache_target(
                    device,
                    "gpui nova metal",
                    present_cache_target_descriptor,
                )?;
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova metal", target_size)?;
                destroy_path_mask_target(device, old_path_mask_target, "Metal");
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "Metal");
                }
                destroy_present_cache_target(device, old_present_cache_target, "Metal");
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "Metal");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (
                    next_path_mask_target,
                    next_backdrop_blur_targets,
                    next_present_cache_target,
                )
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
                let next_present_cache_target = create_present_cache_target(
                    device,
                    "gpui nova vulkan",
                    present_cache_target_descriptor,
                )?;
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova vulkan", target_size)?;
                destroy_path_mask_target(device, old_path_mask_target, "Vulkan");
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "Vulkan");
                }
                destroy_present_cache_target(device, old_present_cache_target, "Vulkan");
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "Vulkan");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (
                    next_path_mask_target,
                    next_backdrop_blur_targets,
                    next_present_cache_target,
                )
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
        self.path_resource_set = next_path_mask_target.resource_set;
        self.backdrop_blur_targets = next_backdrop_blur_targets;
        self.present_cache_texture = next_present_cache_target.texture;
        self.present_cache_texture_view = next_present_cache_target.texture_view;
        self.present_cache_resource_set = next_present_cache_target.resource_set;
        self.surface_config = surface_config;
        self.current_size = next_size;
        self.needs_full_redraw_after_resize = true;
        self.present_cache_valid = false;
        Ok(())
    }

    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub(crate) fn viewport_size(&self) -> Size<DevicePixels> {
        Size {
            width: DevicePixels(self.current_size.width as i32),
            height: DevicePixels(self.current_size.height as i32),
        }
    }

    pub(crate) fn update_transparency(&mut self, is_transparent: bool) {
        let previous_alpha = self.surface_alpha;
        let next_alpha = self.alpha_state_for_current_backend_transparency(is_transparent);
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

    fn alpha_state_for_window_transparency(is_transparent: bool) -> NovaSurfaceAlphaState {
        NovaSurfaceAlphaState::for_window_transparency(is_transparent)
    }

    pub(in crate::platform::nova) fn alpha_state_for_window_transparency_on_backend(
        _backend: RendererBackend,
        is_transparent: bool,
    ) -> NovaSurfaceAlphaState {
        Self::alpha_state_for_window_transparency(is_transparent)
    }

    fn alpha_state_for_current_backend_transparency(
        &self,
        is_transparent: bool,
    ) -> NovaSurfaceAlphaState {
        let backend = match self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(_) => RendererBackend::NovaDx12,
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(_) => RendererBackend::NovaMetal,
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(_) => RendererBackend::NovaVulkan,
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {
                return NovaSurfaceAlphaState::for_window_transparency(is_transparent);
            }
        };
        Self::alpha_state_for_window_transparency_on_backend(backend, is_transparent)
    }

    fn reconfigure_surface_alpha(&mut self, alpha: NovaSurfaceAlphaState) -> Result<()> {
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
            self.needs_full_redraw_after_resize = true;
            self.present_cache_valid = false;
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
        let present_cache_target_descriptor = self.present_cache_target_descriptor(config.size);
        let old_path_mask_target = self.current_path_mask_target();
        let old_backdrop_blur_targets = self.current_backdrop_blur_targets();
        let old_present_cache_target = self.current_present_cache_target();
        let old_depth_texture = self.depth_texture;
        let old_depth_texture_view = self.depth_texture_view;
        let (next_path_mask_target, next_backdrop_blur_targets, next_present_cache_target): (
            NovaPathMaskTarget,
            Option<NovaBackdropBlurTargets>,
            NovaPresentCacheTarget,
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
                let next_present_cache_target = create_present_cache_target(
                    device,
                    "gpui nova dx12",
                    present_cache_target_descriptor,
                )?;
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova dx12", config.size)?;
                destroy_path_mask_target(device, old_path_mask_target, "DX12");
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "DX12");
                }
                destroy_present_cache_target(device, old_present_cache_target, "DX12");
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "DX12");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (
                    next_path_mask_target,
                    next_backdrop_blur_targets,
                    next_present_cache_target,
                )
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
                let next_present_cache_target = create_present_cache_target(
                    device,
                    "gpui nova metal",
                    present_cache_target_descriptor,
                )?;
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova metal", config.size)?;
                destroy_path_mask_target(device, old_path_mask_target, "Metal");
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "Metal");
                }
                destroy_present_cache_target(device, old_present_cache_target, "Metal");
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "Metal");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (
                    next_path_mask_target,
                    next_backdrop_blur_targets,
                    next_present_cache_target,
                )
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
                let next_present_cache_target = create_present_cache_target(
                    device,
                    "gpui nova vulkan",
                    present_cache_target_descriptor,
                )?;
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova vulkan", config.size)?;
                destroy_path_mask_target(device, old_path_mask_target, "Vulkan");
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "Vulkan");
                }
                destroy_present_cache_target(device, old_present_cache_target, "Vulkan");
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "Vulkan");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (
                    next_path_mask_target,
                    next_backdrop_blur_targets,
                    next_present_cache_target,
                )
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
        self.path_resource_set = next_path_mask_target.resource_set;
        self.backdrop_blur_targets = next_backdrop_blur_targets;
        self.present_cache_texture = next_present_cache_target.texture;
        self.present_cache_texture_view = next_present_cache_target.texture_view;
        self.present_cache_resource_set = next_present_cache_target.resource_set;
        self.surface_alpha = alpha;
        self.needs_full_redraw_after_resize = true;
        self.present_cache_valid = false;
        Ok(())
    }

    fn current_path_mask_target(&self) -> NovaPathMaskTarget {
        NovaPathMaskTarget {
            texture: self.path_texture,
            texture_view: self.path_texture_view,
            resource_set: self.path_resource_set,
        }
    }

    pub(super) fn current_backdrop_blur_targets(&self) -> Option<NovaBackdropBlurTargets> {
        self.backdrop_blur_targets.clone()
    }

    fn current_present_cache_target(&self) -> NovaPresentCacheTarget {
        NovaPresentCacheTarget {
            texture: self.present_cache_texture,
            texture_view: self.present_cache_texture_view,
            resource_set: self.present_cache_resource_set,
        }
    }

    fn path_mask_target_descriptor(&self, size: Extent2d) -> NovaPathMaskTargetDescriptor {
        NovaPathMaskTargetDescriptor {
            size,
            format: self.surface_format,
            resource_set_layout: self.path_resource_set_layout,
            global_buffer: self.global_buffer,
            path_sprite_buffer: self.path_sprite_buffer,
            sampler: self.atlas_sampler,
        }
    }

    fn present_cache_target_descriptor(&self, size: Extent2d) -> NovaPresentCacheTargetDescriptor {
        NovaPresentCacheTargetDescriptor {
            size,
            format: self.surface_format,
            resource_set_layout: self.poly_sprite_resource_set_layout,
            global_buffer: self.global_buffer,
            sprite_buffer: self.present_copy_sprite_buffer,
            sampler: self.atlas_sampler,
        }
    }

    pub(super) fn backdrop_blur_target_descriptor(
        &self,
        size: Extent2d,
    ) -> NovaBackdropBlurTargetDescriptor {
        NovaBackdropBlurTargetDescriptor {
            size,
            format: self.surface_format,
            downsample: self.frame_upload.backdrop_blur_downsample(),
            pass_resource_set_layout: self.backdrop_blur_pass_resource_set_layout,
            blur_resource_set_layout: self.backdrop_blur_resource_set_layout,
            global_buffer: self.global_buffer,
            pass_buffer: self.backdrop_blur_pass_buffer,
            blur_buffer: self.backdrop_blur_buffer,
            sampler: self.atlas_sampler,
        }
    }
}
