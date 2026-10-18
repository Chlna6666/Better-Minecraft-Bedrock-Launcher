//! Native buffer retirement and replacement on the D3D12 device owner.
use super::*;

impl Dx12Device {
    pub(super) fn poll_swapchain_resize(&mut self, id: SwapchainId) -> Result<bool> {
        let fence = match self.swapchains.get(id)?.resize_retirement {
            Some(fence) => fence,
            None => {
                // A render submission's fence precedes Present. This signal covers the
                // later DXGI queue work as well and is issued only once per preparation.
                let fence = self.signal_frame()?;
                self.swapchains.get_mut(id)?.resize_retirement = Some(fence);
                fence
            }
        };
        self.poll_cleanup();
        Ok(self.completed_fence_value("DX12 swapchain retirement")? >= fence)
    }

    pub(super) fn retire_swapchain_for_resize(&mut self, id: SwapchainId) -> Result<()> {
        self.poll_swapchain_resize(id)?;
        let fence = self
            .swapchains
            .get(id)?
            .resize_retirement
            .expect("successful preparation installs a retirement fence");
        self.wait_for_fence_value(fence)?;
        // Only release payloads covered by completed fences. Another window may
        // have submitted after this retirement point; its resources remain live.
        self.poll_cleanup();
        Ok(())
    }

    pub(super) fn bind_swapchain_generation(
        &mut self,
        encoder: CommandEncoderId,
        id: SwapchainId,
    ) -> Result<()> {
        let generation = self.swapchains.get(id)?.generation;
        self.command_encoders.get_mut(encoder)?.swapchain = Some((id, generation));
        Ok(())
    }

    pub(super) fn validate_swapchain_submission(
        &mut self,
        encoder: CommandEncoderId,
    ) -> Result<()> {
        if let Some((id, generation)) = self.command_encoders.get(encoder)?.swapchain {
            let chain = self.swapchains.get_mut(id)?;
            if chain.generation != generation {
                return Err(Error::InvalidInput(
                    "DX12 command list refers to a retired swapchain buffer generation".into(),
                ));
            }
            chain.resize_retirement = None;
        }
        Ok(())
    }

    /// Replaces this swapchain's buffers at the requested nonzero pixel extent.
    ///
    /// Waits for its saved rendering/Present retirement point, creating one if needed.
    /// Use [`SurfaceDevice::prepare_swapchain_resize`] on the device owner to poll first.
    /// Later unrelated submissions are not included in that wait. Successful native
    /// replacement advances the buffer generation and rejects previously recorded frames.
    /// Zero width or height leaves the existing buffers unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] for an invalid handle, device/fence failure or DXGI buffer/view
    /// replacement failure. Rebuild failures attempt to restore the previous configuration.
    pub fn resize_swapchain(
        &mut self,
        swapchain: SwapchainId,
        width: u32,
        height: u32,
    ) -> Result<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        let mut config = self.swapchains.get(swapchain)?.config;
        config.size = gfx_core::Extent2d::new(width, height)?;
        self.reconfigure_swapchain_in_place(swapchain, config)
    }

    /// Selects the client-sized source rectangle for the next presentation.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the swapchain is invalid or the source extent exceeds its buffers.
    pub fn set_swapchain_source_extent(
        &mut self,
        id: SwapchainId,
        extent: gfx_core::Extent2d,
    ) -> Result<()> {
        let swapchain = self.swapchains.get_mut(id)?;
        if extent.width() == 0
            || extent.height() == 0
            || extent.width() > swapchain.config.size.width()
            || extent.height() > swapchain.config.size.height()
        {
            return Err(Error::InvalidInput(
                "DX12 presentation source extent exceeds its swapchain".into(),
            ));
        }
        swapchain.source_extent = extent;
        Ok(())
    }

    /// Replaces buffers and presentation configuration, retaining the device registry handle.
    ///
    /// Uses the same retirement and buffer-generation contract as [`Self::resize_swapchain`].
    /// HWND buffers keep native identity. Composition uses an unattached candidate and
    /// publishes it after its first successful Present. HWND alpha changes require recreation.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the swapchain handle is invalid or recreation fails.
    pub fn reconfigure_swapchain(
        &mut self,
        swapchain: SwapchainId,
        config: SurfaceConfig,
    ) -> Result<()> {
        self.reconfigure_swapchain_in_place(swapchain, config)
    }

    /// Replaces the native swapchain while preserving its device registry handle.
    ///
    /// This stronger lifecycle boundary drains queued GPU/Present work before replacing the
    /// native object and its composition attachment. The buffer generation advances, so old
    /// recorded frames cannot refer to the new native object through the retained handle.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the swapchain handle is invalid or recreation fails.
    pub fn recreate_swapchain(
        &mut self,
        swapchain: SwapchainId,
        config: SurfaceConfig,
    ) -> Result<SwapchainId> {
        self.recreate_swapchain_in_place(swapchain, config)?;
        Ok(swapchain)
    }

    pub(super) fn recreate_swapchain_in_place(
        &mut self,
        swapchain: SwapchainId,
        config: SurfaceConfig,
    ) -> Result<()> {
        if self.swapchains.get(swapchain)?.composition.is_some() {
            self.retire_swapchain_for_resize(swapchain)?;
        } else {
            self.wait_for_pending_work()?;
        }
        let (surface, hwnd, composition) = {
            let swapchain_record = self.swapchains.get(swapchain)?;
            let surface = swapchain_record.surface;
            let hwnd = self.surfaces.get(surface)?.hwnd;
            (surface, hwnd, swapchain_record.composition.clone())
        };

        let generation = self.swapchains.get(swapchain)?.generation.wrapping_add(1);
        let mut next_swapchain = self.build_swapchain(surface, hwnd, config, composition)?;
        if self.swapchains.get(swapchain)?.composition.is_none() {
            if let Some(composition) = &mut next_swapchain.composition {
                composition.fallback_hwnd = Some(self.swapchains.get(swapchain)?.swapchain.clone());
            }
        }
        next_swapchain.generation = generation;
        let old_swapchain = self.swapchains.replace_live(swapchain, next_swapchain)?;
        // This chain's readers retired above. The new composition record still
        // owns the last published native content until its first valid Present.
        self.release_deferred_payload(DeferredDx12Release::Swapchain(old_swapchain));
        Ok(())
    }

    pub(super) fn reconfigure_swapchain_in_place(
        &mut self,
        swapchain: SwapchainId,
        config: SurfaceConfig,
    ) -> Result<()> {
        if self.swapchains.get(swapchain)?.composition.is_some() {
            return self.recreate_swapchain_in_place(swapchain, config);
        }
        // ResizeBuffers retains this waitable object. Preserve any consumed
        // readiness credit while cancelling notifications for the old size.
        self.swapchains.get_mut(swapchain)?.frame_pacing.cancel();
        self.retire_swapchain_for_resize(swapchain)?;
        let device = self.device.clone();
        let previous_config = self.swapchains.get(swapchain)?.config;
        if previous_config.alpha_mode != config.alpha_mode {
            return Err(Error::InvalidInput(format!(
                "DX12 swapchain alpha mode cannot be changed after swapchain creation; \
                     destroy and recreate the swapchain instead: old={:?} new={:?}",
                previous_config.alpha_mode, config.alpha_mode
            )));
        }

        let swapchain = self.swapchains.get_mut(swapchain)?;

        swapchain.render_targets.clear();
        swapchain.rtv_heap = None;
        swapchain.config = config;

        // SAFETY: Swapchain is valid and all references to previous backbuffers were dropped.
        if let Err(error) = unsafe {
            swapchain.swapchain.ResizeBuffers(
                BACK_BUFFER_COUNT,
                config.size.width(),
                config.size.height(),
                format_to_dxgi(config.format),
                DXGI_SWAP_CHAIN_FLAG(swapchain.creation_flags as i32),
            )
        } {
            swapchain.config = previous_config;
            Self::rebuild_render_targets(&device, swapchain).map_err(|rollback_error| {
                    Error::Backend(format!(
                        "DX12 ResizeBuffers failed: {error}; rebuilding previous render targets failed: {rollback_error}"
                    ))
                })?;
            return Err(Error::Backend(error.to_string()));
        }

        swapchain.generation = swapchain.generation.wrapping_add(1);
        swapchain.resize_retirement = None;
        if let Err(error) = Self::rebuild_render_targets(&device, swapchain) {
            swapchain.config = previous_config;
            // SAFETY: Swapchain is valid and the failed render-target rebuild left no live
            // backbuffer references in this registry entry.
            let rollback = unsafe {
                swapchain.swapchain.ResizeBuffers(
                    BACK_BUFFER_COUNT,
                    previous_config.size.width(),
                    previous_config.size.height(),
                    format_to_dxgi(previous_config.format),
                    DXGI_SWAP_CHAIN_FLAG(swapchain.creation_flags as i32),
                )
            };
            if let Err(rollback_error) = rollback {
                return Err(Error::Backend(format!(
                    "DX12 render target rebuild failed after ResizeBuffers: {error}; rollback ResizeBuffers failed: {rollback_error}"
                )));
            }
            swapchain.generation = swapchain.generation.wrapping_add(1);
            Self::rebuild_render_targets(&device, swapchain).map_err(|rollback_error| {
                    Error::Backend(format!(
                        "DX12 render target rebuild failed after ResizeBuffers: {error}; rollback render target rebuild failed: {rollback_error}"
                    ))
                })?;
            return Err(error);
        }

        Ok(())
    }

    pub(super) fn publish_composition_content(swapchain: &mut Dx12Swapchain) -> Result<()> {
        let Some(composition) = &mut swapchain.composition else {
            return Ok(());
        };
        if composition.active_content.as_ref() == Some(&swapchain.swapchain) {
            return Ok(());
        }
        // SAFETY: Present succeeded for this candidate. Both generations remain owned
        // until the tree commit succeeds, including a failed commit/rollback attempt.
        let result = unsafe {
            composition
                .visual
                .SetContent(&swapchain.swapchain)
                .and_then(|()| composition._target.SetRoot(&composition.visual))
                .and_then(|()| composition._device.Commit())
        };
        if let Err(error) = result {
            let rollback = unsafe {
                match &composition.active_content {
                    Some(active) => composition
                        .visual
                        .SetContent(active)
                        .and_then(|()| composition._target.SetRoot(&composition.visual)),
                    None => composition._target.SetRoot(None),
                }
                .and_then(|()| composition._device.Commit())
            };
            if let Err(rollback_error) = rollback {
                log::warn!("DX12 composition content rollback failed: {rollback_error}");
            }
            return Err(Error::Backend(error.to_string()));
        }
        composition.active_content = Some(swapchain.swapchain.clone());
        composition.fallback_hwnd = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
