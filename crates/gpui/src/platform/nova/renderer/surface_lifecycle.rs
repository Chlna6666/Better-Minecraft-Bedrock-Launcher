use super::*;

/// A path mask promoted to the drawable extent must track Resize. Its 1x1
/// placeholder can survive all intermediate client sizes without reallocation.
fn planned_path_mask_size(
    existing: Extent2d,
    old_drawable: DrawableSize,
    new_drawable: Extent2d,
) -> Result<Extent2d> {
    if existing.width() == old_drawable.width
        && existing.height() == old_drawable.height
    {
        Ok(new_drawable)
    } else {
        Ok(Extent2d::new(1, 1)?)
    }
}

#[cfg(test)]
mod resize_plan_tests {
    use super::*;

    #[test]
    fn equal_extents_still_require_the_resize_generation_to_match() {
        let size = crate::size(DevicePixels(500), DevicePixels(500));
        let request = DrawableSizeRequest {
            size,
            epoch: Some(7),
        };

        assert!(!request.applies_to(Some(6)));
        assert!(request.applies_to(Some(7)));
        assert!(!request.applies_to(None));
    }

    #[test]
    fn resize_reuses_unpromoted_one_pixel_path_mask() {
        let old = DrawableSize { width: 1000, height: 700 };
        let placeholder = Extent2d::new(1, 1).unwrap();
        let next = Extent2d::new(1600, 900).unwrap();
        assert_eq!(planned_path_mask_size(placeholder, old, next).unwrap(), placeholder);
    }

    #[test]
    fn resize_promotes_an_active_path_mask_to_new_client_extent() {
        let old = DrawableSize { width: 1000, height: 700 };
        let active = Extent2d::new(1000, 700).unwrap();
        let next = Extent2d::new(1600, 900).unwrap();
        assert_eq!(planned_path_mask_size(active, old, next).unwrap(), next);
    }
}

impl NovaRenderer {
    /// The physical swapchain extent already committed by Nova; the pending resize
    /// request is deliberately excluded because it may still be fence-blocked.
    #[cfg(target_os = "windows")]
    pub(crate) fn committed_drawable_extent(&self) -> (u32, u32) {
        (self.current_size.width, self.current_size.height)
    }

    pub(crate) fn resize(&mut self, size: Size<DevicePixels>) -> Result<()> {
        let resize_started_at = Instant::now();
        let width = size.width.0.max(1) as u32;
        let height = size.height.0.max(1) as u32;
        let next_size = DrawableSize { width, height };
        if next_size == self.current_size {
            return Ok(());
        }
        let wait_started_at = Instant::now();
        self.prepare_for_resize()?;
        let submission_wait = wait_started_at.elapsed();
        let target_size = Extent2d::new(width, height)?;
        let surface_config = SurfaceConfig {
            size: target_size,
            format: self.surface_format,
            present_mode: self.present_mode,
            alpha_mode: self.surface_alpha.swapchain_mode,
        };
        let next_path_mask_size =
            planned_path_mask_size(self.path_texture_size, self.current_size, target_size)?;
        // Most windows never render a path mask; their 1x1 placeholder and
        // frame-slot resource sets are still valid after a drawable resize.
        // Reusing this object avoids per-WM_SIZE texture/view/set retirement and
        // driver allocations. A promoted path mask changes size and is rebuilt.
        let reuse_path_mask_target = next_path_mask_size == self.path_texture_size;
        let path_mask_target_descriptor = self.path_mask_target_descriptor(next_path_mask_size);
        // Blur topology belongs to the next scene, not the previously packed
        // frame. Retire old-size targets here and allocate once after pack_scene
        // knows the new scene's captures, quality and retained-root policy.
        let old_path_mask_target = self.current_path_mask_target();
        let old_backdrop_blur_targets = self.current_backdrop_blur_targets();
        let old_depth_texture = self.depth_texture;
        let old_depth_texture_view = self.depth_texture_view;
        let swapchain_resize;
        let (next_path_mask_target, surface_config): (PathMaskTarget, SurfaceConfig) =
            match &mut *lock_backend(&self.backend) {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => {
                let next_path_mask_target = if reuse_path_mask_target {
                    old_path_mask_target.clone()
                } else {
                    create_path_mask_target(device, "gpui nova opengl", path_mask_target_descriptor)?
                };
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova opengl", target_size)?;
                // Keep the last presented swapchain valid while the expensive size-dependent
                // targets are prepared. Replacing its buffers first exposes an unpresented
                // surface to DWM for the remainder of this transaction during live resize.
                let swapchain_started_at = Instant::now();
                if let Err(error) = resize_opengl_swapchain(device, self.swapchain, surface_config)
                {
                    if !reuse_path_mask_target {
                        destroy_path_mask_target(device, next_path_mask_target, "OpenGL");
                    }
                    destroy_depth_target(
                        device,
                        next_depth_texture,
                        next_depth_texture_view,
                        "OpenGL",
                    );
                    return Err(error);
                }
                swapchain_resize = swapchain_started_at.elapsed();
                if !reuse_path_mask_target {
                    destroy_path_mask_target(device, old_path_mask_target, "OpenGL");
                }
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "OpenGL");
                }
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "OpenGL");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (next_path_mask_target, surface_config)
            }
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => {
                let next_path_mask_target = if reuse_path_mask_target {
                    old_path_mask_target.clone()
                } else {
                    create_path_mask_target(device, "gpui nova dx11", path_mask_target_descriptor)?
                };
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova dx11", target_size)?;
                // Keep the last presented swapchain valid while the expensive size-dependent
                // targets are prepared. Replacing its buffers first exposes an unpresented
                // surface to DWM for the remainder of this transaction during live resize.
                let swapchain_started_at = Instant::now();
                if let Err(error) = resize_dx11_swapchain(device, self.swapchain, surface_config) {
                    if !reuse_path_mask_target {
                        destroy_path_mask_target(device, next_path_mask_target, "DX11");
                    }
                    destroy_depth_target(
                        device,
                        next_depth_texture,
                        next_depth_texture_view,
                        "DX11",
                    );
                    return Err(error);
                }
                swapchain_resize = swapchain_started_at.elapsed();
                if !reuse_path_mask_target {
                    destroy_path_mask_target(device, old_path_mask_target, "DX11");
                }
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "DX11");
                }
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "DX11");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (next_path_mask_target, surface_config)
            }
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                let next_path_mask_target = if reuse_path_mask_target {
                    old_path_mask_target.clone()
                } else {
                    create_path_mask_target(device, "gpui nova dx12", path_mask_target_descriptor)?
                };
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova dx12", target_size)?;
                // Keep the last presented swapchain valid while the expensive size-dependent
                // targets are prepared. Replacing its buffers first exposes an unpresented
                // surface to DWM for the remainder of this transaction during live resize.
                let swapchain_started_at = Instant::now();
                if let Err(error) = resize_dx12_swapchain(device, self.swapchain, surface_config) {
                    if !reuse_path_mask_target {
                        destroy_path_mask_target(device, next_path_mask_target, "DX12");
                    }
                    destroy_depth_target(
                        device,
                        next_depth_texture,
                        next_depth_texture_view,
                        "DX12",
                    );
                    return Err(error);
                }
                swapchain_resize = swapchain_started_at.elapsed();
                if !reuse_path_mask_target {
                    destroy_path_mask_target(device, old_path_mask_target, "DX12");
                }
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "DX12");
                }
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "DX12");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (next_path_mask_target, surface_config)
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => {
                let swapchain_started_at = Instant::now();
                device.resize_swapchain(self.swapchain, width, height)?;
                swapchain_resize = swapchain_started_at.elapsed();
                let next_path_mask_target = if reuse_path_mask_target {
                    old_path_mask_target.clone()
                } else {
                    create_path_mask_target(device, "gpui nova metal", path_mask_target_descriptor)?
                };
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova metal", target_size)?;
                if !reuse_path_mask_target {
                    destroy_path_mask_target(device, old_path_mask_target, "Metal");
                }
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "Metal");
                }
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "Metal");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (next_path_mask_target, surface_config)
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => {
                // WSI may resolve a different extent from the requested client size.
                // Build attachments against the allocated image extent, never a stale
                // native event size; otherwise viewport projection scales the scene
                // and the depth attachment may be smaller than its framebuffer.
                let swapchain_started_at = Instant::now();
                resize_vulkan_swapchain(device, self.swapchain, surface_config)?;
                swapchain_resize = swapchain_started_at.elapsed();
                let actual_size = device.swapchain_size(self.swapchain)?;
                let surface_config = SurfaceConfig { size: actual_size, ..surface_config };
                let next_path_mask_size =
                    planned_path_mask_size(self.path_texture_size, self.current_size, actual_size)?;
                let reuse_path_mask_target = next_path_mask_size == self.path_texture_size;
                let mut path_mask_target_descriptor = path_mask_target_descriptor;
                path_mask_target_descriptor.size = next_path_mask_size;
                let next_path_mask_target = if reuse_path_mask_target {
                    old_path_mask_target.clone()
                } else {
                    create_path_mask_target(device, "gpui nova vulkan", path_mask_target_descriptor)?
                };
                let (next_depth_texture, next_depth_texture_view) =
                    match create_depth_target(device, "gpui nova vulkan", actual_size) {
                        Ok(target) => target,
                        Err(error) => {
                            if !reuse_path_mask_target {
                                destroy_path_mask_target(device, next_path_mask_target, "Vulkan");
                            }
                            return Err(error);
                        }
                    };
                if !reuse_path_mask_target {
                    destroy_path_mask_target(device, old_path_mask_target, "Vulkan");
                }
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "Vulkan");
                }
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "Vulkan");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (next_path_mask_target, surface_config)
            }
            #[cfg(not(any(
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
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
        let next_size = DrawableSize {
            width: surface_config.size.width(),
            height: surface_config.size.height(),
        };
        let next_path_mask_size =
            planned_path_mask_size(self.path_texture_size, self.current_size, surface_config.size)?;
        let reuse_path_mask_target = next_path_mask_size == self.path_texture_size;
        self.path_texture = next_path_mask_target.texture;
        self.path_texture_view = next_path_mask_target.texture_view;
        self.path_texture_size = next_path_mask_size;
        if !reuse_path_mask_target {
            self.update_path_mask_resource_sets(&next_path_mask_target.resource_sets)?;
        }
        self.filters.targets = None;
        self.invalidate_backdrop_blur_cache();
        self.activate_frame_resources(self.current_frame_resource_index)?;
        self.surface_config = surface_config;
        self.current_size = next_size;
        self.swapchain_warmup_frames = SWAPCHAIN_WARMUP_FRAME_COUNT;
        let total = resize_started_at.elapsed();
        let resources = total.saturating_sub(submission_wait.saturating_add(swapchain_resize));
        crate::diagnostics::performance_metrics::record_surface_resize(
            submission_wait,
            swapchain_resize,
            resources,
            total,
        );
        // Resizing path/depth targets and native swapchains
        // serializes one shared GPU owner. Emit the expensive phase only on an
        // actual outlier; no per-frame idle logging or additional GPU waits.
        if total >= std::time::Duration::from_millis(16) {
            log::warn!(
                target: "gpui_gpu_resize",
                "slow_surface_resize backend={} physical_px={}x{} total_us={} submission_wait_us={} swapchain_us={} resources_us={}",
                self.backend_info.label(),
                next_size.width,
                next_size.height,
                total.as_micros(),
                submission_wait.as_micros(),
                swapchain_resize.as_micros(),
                resources.as_micros(),
            );
        }
        Ok(())
    }

    pub(crate) fn update_drawable_size(&mut self, size: Size<DevicePixels>, epoch: Option<u64>) {
        let request = DrawableSizeRequest { size, epoch };
        if self.live_resize_active {
            self.live_resize_size = Some(request);
        }
        self.pending_drawable_size = Some(request);
    }

    /// Applies the newest coalesced drawable size only when the GPU/swapchain can be replaced
    /// immediately. Returning `false` tells the caller to skip rendering this frame; importantly,
    /// no new old-size GPU work is submitted while resize backpressure is active.
    pub(super) fn apply_pending_drawable_size(
        &mut self,
        scene_epoch: Option<u64>,
    ) -> Result<bool> {
        #[cfg(all(
            feature = "nova-gfx-vulkan",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        ))]
        if self.pending_drawable_size.is_none() {
            // Outdated-surface recovery may replace WSI images independently of
            // native event delivery. Reconcile attachments before sampling them.
            if let NovaBackend::Vulkan(device) = &*lock_backend(&self.backend) {
                let actual = device.swapchain_size(self.swapchain)?;
                if actual.width() != self.current_size.width
                    || actual.height() != self.current_size.height
                {
                    self.pending_drawable_size = Some(DrawableSizeRequest {
                        size: crate::size(
                            DevicePixels(actual.width() as i32),
                            DevicePixels(actual.height() as i32),
                        ),
                        epoch: None,
                    });
                }
            }
        }
        let Some(request) = self.pending_drawable_size.take() else {
            return Ok(true);
        };
        if !request.applies_to(scene_epoch) {
            self.pending_drawable_size = Some(request);
            return Ok(false);
        }
        match self.try_resize(request.size) {
            Ok(true) => Ok(true),
            Ok(false) => {
                self.pending_drawable_size = Some(request);
                Ok(false)
            }
            Err(error) => {
                self.pending_drawable_size = Some(request);
                Err(error)
            }
        }
    }

    pub(crate) fn has_pending_drawable_size(&self) -> bool {
        self.pending_drawable_size.is_some()
    }

    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub(crate) fn viewport_size(&self) -> Size<DevicePixels> {
        if let Some(size) = self.pending_drawable_size {
            return size.size;
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
                    self.backend_info.label(),
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
                self.backend_info.label(),
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
        match &*lock_backend(&self.backend) {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(_) => Ok(requested),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(_) => Ok(requested),
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
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
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
                self.backend_info.label(),
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
        let next_path_mask_size = self.path_texture_size;
        let path_mask_target_descriptor = self.path_mask_target_descriptor(next_path_mask_size);
        let backdrop_blur_target_descriptor = self.backdrop_blur_target_descriptor(config.size);
        let old_path_mask_target = self.current_path_mask_target();
        let old_backdrop_blur_targets = self.current_backdrop_blur_targets();
        let old_depth_texture = self.depth_texture;
        let old_depth_texture_view = self.depth_texture_view;
        let (next_path_mask_target, next_backdrop_blur_targets): (
            PathMaskTarget,
            Option<BackdropBlurTargets>,
        ) = match &mut *lock_backend(&self.backend) {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => {
                self.swapchain =
                    recreate_opengl_swapchain_for_config(device, self.swapchain, config)?;
                let next_path_mask_target = create_path_mask_target(
                    device,
                    "gpui nova opengl",
                    path_mask_target_descriptor,
                )?;
                let next_backdrop_blur_targets = if old_backdrop_blur_targets.is_some() {
                    Some(create_backdrop_blur_target_chain(
                        device,
                        "gpui nova opengl",
                        backdrop_blur_target_descriptor,
                    )?)
                } else {
                    None
                };
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova opengl", config.size)?;
                destroy_path_mask_target(device, old_path_mask_target, "OpenGL");
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "OpenGL");
                }
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "OpenGL");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (next_path_mask_target, next_backdrop_blur_targets)
            }
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => {
                self.swapchain =
                    recreate_dx11_swapchain_for_config(device, self.swapchain, config)?;
                let next_path_mask_target =
                    create_path_mask_target(device, "gpui nova dx11", path_mask_target_descriptor)?;
                let next_backdrop_blur_targets = if old_backdrop_blur_targets.is_some() {
                    Some(create_backdrop_blur_target_chain(
                        device,
                        "gpui nova dx11",
                        backdrop_blur_target_descriptor,
                    )?)
                } else {
                    None
                };
                let (next_depth_texture, next_depth_texture_view) =
                    create_depth_target(device, "gpui nova dx11", config.size)?;
                destroy_path_mask_target(device, old_path_mask_target, "DX11");
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "DX11");
                }
                destroy_depth_target(device, old_depth_texture, old_depth_texture_view, "DX11");
                self.depth_texture = next_depth_texture;
                self.depth_texture_view = next_depth_texture_view;
                (next_path_mask_target, next_backdrop_blur_targets)
            }
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
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
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
        self.path_texture_size = next_path_mask_size;
        self.update_path_mask_resource_sets(&next_path_mask_target.resource_sets)?;
        self.filters.targets = next_backdrop_blur_targets;
        self.invalidate_backdrop_blur_cache();
        self.swapchain_warmup_frames = SWAPCHAIN_WARMUP_FRAME_COUNT;
        self.activate_frame_resources(self.current_frame_resource_index)?;
        self.surface_alpha = alpha;
        Ok(())
    }

    pub(super) fn current_path_mask_target(&self) -> PathMaskTarget {
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
        self.filters.targets.clone()
    }

    pub(super) fn path_mask_target_descriptor(&self, size: Extent2d) -> PathMaskTargetDescriptor {
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
            needs_root_source: super::present::has_root_backdrop_blurs(&self.frame_upload),
            isolated_source_indices,
            pass_resource_set_layout: self.backdrop_blur_pass_resource_set_layout,
            blur_resource_set_layout: self.backdrop_blur_resource_set_layout,
            frame_buffers: self.frame_resource_buffers(),
            sampler: self.atlas_sampler,
        }
    }
}
