use crate::{
    device::{Dx11Device, backend, required},
    frame_pacing::FrameLatencyWait,
    resources::format,
};
use gfx_core::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND},
        Graphics::{
            Direct3D11::*,
            DirectComposition::*,
            Dxgi::{Common::*, *},
        },
    },
    core::Interface,
};

mod composition;
use composition::Composition;
pub(crate) struct Swapchain {
    pub(crate) native: IDXGISwapChain3,
    pub(crate) view: Option<ID3D11RenderTargetView>,
    pub(crate) config: SurfaceConfig,
    pub(crate) source_extent: Extent2d,
    surface: SurfaceId,
    composition: Option<Composition>,
    pub(crate) latency: HANDLE,
    pub(crate) pacing: FrameLatencyWait,
    pub(crate) generation: u64,
    pub(crate) resize_retirement: Option<SubmissionId>,
}
impl Drop for Swapchain {
    fn drop(&mut self) {
        self.pacing.cancel();
        // SAFETY: DXGI hands ownership of its waitable handle to the caller. Wait callbacks
        // have drained above; close it once before releasing the swapchain.
        if !self.latency.is_invalid() {
            if let Err(error) = unsafe { CloseHandle(self.latency) } {
                log::warn!("D3D11 latency handle cleanup failed: {error}");
            }
        }
    }
}

impl SurfaceDevice for Dx11Device {
    type SurfaceTarget = dyn HasWindowHandle;
    fn create_surface(
        &mut self,
        target: &Self::SurfaceTarget,
        _desc: &SurfaceDescriptor,
    ) -> Result<SurfaceId> {
        let handle = target
            .window_handle()
            .map_err(|error| Error::InvalidInput(error.to_string()))?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(Error::InvalidInput("D3D11 requires an HWND".into()));
        };
        Ok(self.surfaces.insert(HWND(handle.hwnd.get() as *mut _)))
    }
    fn create_swapchain(
        &mut self,
        surface: SurfaceId,
        config: SurfaceConfig,
    ) -> Result<SwapchainId> {
        let hwnd = *self.surfaces.get(surface)?;
        let composited = match config.alpha_mode {
            CompositeAlphaMode::Opaque | CompositeAlphaMode::Auto => false,
            CompositeAlphaMode::Premultiplied => true,
            _ => {
                return Err(Error::Unavailable(
                    "D3D11 composition requires premultiplied alpha".into(),
                ));
            }
        };
        if config.format == Format::Depth32Float || config.format.is_srgb() {
            return Err(Error::InvalidInput(
                "DXGI flip swapchains require a non-sRGB color format".into(),
            ));
        }
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: config.size.width(),
            Height: config.size.height(),
            Format: format(config.format),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            // An HWND can grow before the matching Scene/ResizeBuffers is ready.
            // Let DWM scale the last presented frame across that exposed client
            // strip instead of showing undefined black pixels. The swapchain is
            // resized to the exact client extent before the matching Scene is
            // presented, so the settled frame is still presented one-to-one.
            // Composition requires STRETCH by contract as well.
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: if composited {
                DXGI_ALPHA_MODE_PREMULTIPLIED
            } else {
                DXGI_ALPHA_MODE_IGNORE
            },
            Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
            ..Default::default()
        };
        // SAFETY: hwnd stays alive through surface lifetime; native device/factory are live.
        let native: IDXGISwapChain3 = unsafe {
            if composited {
                self.factory
                    .CreateSwapChainForComposition(&self.native, &desc, None)
            } else {
                self.factory
                    .CreateSwapChainForHwnd(&self.native, hwnd, &desc, None, None)
            }
        }
        .map_err(backend)?
        .cast()
        .map_err(backend)?;
        // SAFETY: live DXGI swapchain owns native pacing state.
        unsafe { native.SetMaximumFrameLatency(1) }.map_err(backend)?;
        let latency = unsafe { native.GetFrameLatencyWaitableObject() };
        if latency.is_invalid() {
            return Err(Error::Backend(
                "DXGI returned no frame latency handle".into(),
            ));
        }
        let source_extent = config.size;
        let mut chain = Swapchain {
            native,
            view: None,
            config,
            source_extent,
            surface,
            composition: None,
            latency,
            pacing: FrameLatencyWait::default(),
            generation: 0,
            resize_retirement: None,
        };
        if composited {
            chain.composition = Some(composition::create(&self.native, hwnd)?);
        }
        chain.view = Some(backbuffer_view(&self.native, &chain.native)?);
        Ok(self.swapchains.insert(chain))
    }
    fn prepare_swapchain_resize(&mut self, id: SwapchainId) -> Result<bool> {
        let retirement = match self.swapchains.get(id)?.resize_retirement {
            Some(retirement) => retirement,
            None => {
                let retirement = self.signal()?;
                self.swapchains.get_mut(id)?.resize_retirement = Some(retirement);
                retirement
            }
        };
        Ok(SubmissionDevice::poll_submission(self, retirement)? == SubmissionStatus::Complete)
    }
    fn set_swapchain_source_extent(&mut self, id: SwapchainId, extent: Extent2d) -> Result<()> {
        let chain = self.swapchains.get_mut(id)?;
        if extent.width() == 0
            || extent.height() == 0
            || extent.width() > chain.config.size.width()
            || extent.height() > chain.config.size.height()
        {
            return Err(Error::InvalidInput(
                "D3D11 presentation source extent exceeds its swapchain".into(),
            ));
        }
        chain.source_extent = extent;
        Ok(())
    }
    fn resize_swapchain(&mut self, id: SwapchainId, size: Extent2d) -> Result<()> {
        Self::resize_swapchain(self, id, size.width(), size.height())
    }

    fn recreate_swapchain(
        &mut self,
        id: SwapchainId,
        config: SurfaceConfig,
    ) -> Result<SwapchainId> {
        Self::recreate_swapchain(self, id, config)
    }

    fn destroy_swapchain(&mut self, id: SwapchainId) -> Result<()> {
        if let Some(composition) = &self.swapchains.get(id)?.composition {
            composition.detach()?;
        }
        // SAFETY: clear context bindings so DXGI resources can be released at this lifecycle barrier.
        unsafe {
            self.context.ClearState();
        }
        self.swapchains.take(id)?;
        // SAFETY: flush deferred destruction after dropping the old HWND swapchain;
        // a later transparency transition may create another chain for this HWND.
        unsafe {
            self.context.Flush();
        }
        Ok(())
    }
    fn destroy_surface(&mut self, id: SurfaceId) -> Result<()> {
        if self.swapchains.values().any(|chain| chain.surface == id) {
            return Err(Error::InvalidInput(
                "surface still has a live swapchain".into(),
            ));
        }
        self.surfaces.take(id)?;
        Ok(())
    }
}

impl Dx11Device {
    /// Queries DXGI frame latency without waiting on the graphics owner.
    /// # Errors
    /// Returns an error for stale swapchains or failed native waits.
    pub fn swapchain_frame_ready(&mut self, id: SwapchainId) -> Result<bool> {
        let chain = self.swapchains.get(id)?;
        chain.pacing.ready(chain.latency)
    }
    /// Recreates the backbuffer views for the latest drawable extent.
    ///
    /// Call [`SurfaceDevice::prepare_swapchain_resize`] first to poll this window's prior
    /// rendering/Present work. DXGI still synchronizes the native replacement. Successful
    /// HWND ResizeBuffers advances the buffer generation and consumes its retirement marker,
    /// including when subsequent view acquisition fails. Retain the native swapchain attached
    /// to the composition visual; bitmap presentation and visual-tree commits are independent.
    /// # Errors
    /// Returns a native resize error; existing buffers are reacquired on failure.
    pub fn resize_swapchain(&mut self, id: SwapchainId, width: u32, height: u32) -> Result<()> {
        let size = Extent2d::new(width, height)?;
        let resize_started = std::time::Instant::now();
        let chain = self.swapchains.get_mut(id)?;
        chain.pacing.cancel();
        let previous_size = chain.config.size;
        let previous_source = chain.source_extent;
        let cancel_elapsed = resize_started.elapsed();
        // SAFETY: clear indirect references before DXGI ResizeBuffers; immediate commands
        // retain native resources until execution retires, and DXGI synchronizes the resize.
        unsafe {
            self.context.ClearState();
            self.context.Flush();
        }
        let flush_elapsed = resize_started.elapsed().saturating_sub(cancel_elapsed);
        chain.view = None;
        // SAFETY: backbuffer references were released; flags preserve the waitable chain.
        let result = unsafe {
            chain.native.ResizeBuffers(
                0,
                width,
                height,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
            )
        };
        let dxgi_elapsed = resize_started
            .elapsed()
            .saturating_sub(cancel_elapsed + flush_elapsed);
        if result.is_ok() {
            chain.generation = chain.generation.wrapping_add(1);
            chain.resize_retirement = None;
            chain.config.size = size;
            chain.source_extent = size;
            rebuild_backbuffer_view(&self.native, chain, previous_size, previous_source)?;
        } else {
            chain.view = Some(backbuffer_view(&self.native, &chain.native)?);
        }
        let view_elapsed = resize_started
            .elapsed()
            .saturating_sub(cancel_elapsed + flush_elapsed + dxgi_elapsed);
        result.map_err(backend)?;
        // Preserve DXGI frame-ready credit acquired from the auto-reset waitable
        // object before ResizeBuffers. cancel() above already drains callbacks
        // without dropping an acquired signal. reset() here would clear the
        // credit even though no Present occurred and leave the first new-size
        // frame waiting on a signal that may require that very Present.
        //
        // present_steps() consumes the credit only after successful Present.
        // A replacement swapchain uses a new FrameLatencyWait; old credit cannot
        // leak across a swapchain-identity change.
        // Keep the last frame's coverage until present_steps publishes the new-size
        // pixels. Resetting the visual here races the first replacement-buffer Present.
        let total = resize_started.elapsed();
        // Emit outliers only: tracing every WM_SIZE would itself disrupt high-refresh resize.
        if total >= std::time::Duration::from_millis(16) {
            log::warn!(
                target: "nova_dx11_surface",
                "slow_dx11_resize_phases physical_px={}x{} total_us={} pacing_cancel_us={} context_flush_us={} dxgi_resize_buffers_us={} backbuffer_view_us={}",
                width,
                height,
                total.as_micros(),
                cancel_elapsed.as_micros(),
                flush_elapsed.as_micros(),
                dxgi_elapsed.as_micros(),
                view_elapsed.as_micros(),
            );
        }
        Ok(())
    }
    /// Replaces alpha/presentation configuration at a swapchain lifecycle boundary.
    /// # Errors
    /// Returns a configuration/native error. Format changes require a new surface.
    /// Size/present-mode changes retain the chain; opaque/premultiplied transitions
    /// prepare a replacement and retain the previous chain if preparation fails.
    pub fn recreate_swapchain(
        &mut self,
        id: SwapchainId,
        config: SurfaceConfig,
    ) -> Result<SwapchainId> {
        let old = self.swapchains.get(id)?;
        if old.config.format != config.format {
            return Err(Error::InvalidInput(
                "DX11 format changes require a new surface".into(),
            ));
        }
        let next_composited = match config.alpha_mode {
            CompositeAlphaMode::Auto | CompositeAlphaMode::Opaque => false,
            CompositeAlphaMode::Premultiplied => true,
            _ => {
                return Err(Error::Unavailable(
                    "D3D11 composition requires premultiplied alpha".into(),
                ));
            }
        };
        if old.composition.is_some() != next_composited {
            let surface = old.surface;
            let fallback = old.composition.is_none().then(|| old.native.clone());
            let next = SurfaceDevice::create_swapchain(self, surface, config)?;
            if let Some(fallback) = fallback {
                if let Some(composition) = &mut self.swapchains.get_mut(next)?.composition {
                    composition.retain_hwnd(fallback);
                }
            }
            if let Some(composition) = &self.swapchains.get(id)?.composition
                && let Err(error) = composition.detach()
            {
                if let Err(cleanup_error) = SurfaceDevice::destroy_swapchain(self, next) {
                    log::warn!("D3D11 replacement swapchain cleanup failed: {cleanup_error}");
                }
                return Err(error);
            }
            SurfaceDevice::destroy_swapchain(self, id)?;
            return Ok(next);
        }
        self.resize_swapchain(id, config.size.width(), config.size.height())?;
        self.swapchains.get_mut(id)?.config = config;
        Ok(id)
    }
    pub(crate) fn stretch(&mut self, id: SwapchainId, scale: Option<[f32; 2]>) -> Result<()> {
        let chain = self.swapchains.get_mut(id)?;
        if let Some(composition) = &mut chain.composition {
            let scale = scale.unwrap_or([1.0, 1.0]);
            if composition.scale == scale {
                return Ok(());
            }
            // SAFETY: matrix has finite scale supplied by the renderer; COM references stay live.
            unsafe {
                composition
                    .visual
                    .SetTransform2(&windows_numerics::Matrix3x2 {
                        M11: scale[0],
                        M22: scale[1],
                        ..Default::default()
                    })
                    .map_err(backend)?;
                composition.device.Commit().map_err(backend)?;
            }
            composition.scale = scale;
        }
        Ok(())
    }
    pub(crate) fn present_steps(
        &mut self,
        id: SwapchainId,
        pass: RenderPassId,
        commands: &[RenderCommand],
        load: LoadOp<ClearColor>,
        depth: Option<DepthAttachment>,
    ) -> Result<SubmissionId> {
        // D3D11 rotates the identity of buffer zero at Present. Its retained view always
        // targets the next writable image; D3D12's physical image indexing does not apply.
        self.render_target(
            RenderTarget::Swapchain {
                swapchain: id,
                image_index: 0,
            },
            pass,
            commands,
            load,
            depth,
        )?;
        let chain = self.swapchains.get_mut(id)?;
        let source: IDXGISwapChain2 = chain.native.cast().map_err(backend)?;
        // Apply the source rectangle only after drawing the matching new-size scene. Before this
        // Present, DXGI continues showing the previously committed frame.
        unsafe { source.SetSourceSize(chain.source_extent.width(), chain.source_extent.height()) }
            .map_err(backend)?;
        let interval = if chain.config.present_mode == PresentMode::Fifo {
            1
        } else {
            0
        };
        // SAFETY: immediate rendering has been recorded and output bindings removed. No tearing.
        unsafe { chain.native.Present(interval, DXGI_PRESENT(0)) }
            .ok()
            .map_err(backend)?;
        chain.pacing.consume_ready();
        let publication = if let Some(composition) = &mut chain.composition {
            composition.publish(&chain.native)
        } else {
            Ok(())
        };
        let submission = self.signal()?;
        // This query follows Present, unlike an arbitrary device-wide pending count.
        self.swapchains.get_mut(id)?.resize_retirement = Some(submission);
        if let Err(error) = publication {
            // The renderer receives no submission handle on error. Retire the already
            // issued GPU writes before it may reuse that failed frame's upload slot.
            SubmissionDevice::wait_submission(self, submission)?;
            return Err(error);
        }
        Ok(submission)
    }
}

fn backbuffer_view(
    device: &ID3D11Device,
    chain: &IDXGISwapChain3,
) -> Result<ID3D11RenderTargetView> {
    // SAFETY: D3D11's writable flip-chain image is always logical buffer zero.
    let buffer: ID3D11Texture2D = unsafe { chain.GetBuffer(0) }.map_err(backend)?;
    let mut view = None;
    unsafe { device.CreateRenderTargetView(&buffer, None, Some(&mut view)) }.map_err(backend)?;
    required(view)
}

fn rebuild_backbuffer_view(
    device: &ID3D11Device,
    chain: &mut Swapchain,
    previous_size: Extent2d,
    previous_source: Extent2d,
) -> Result<()> {
    let error = match backbuffer_view(device, &chain.native) {
        Ok(view) => {
            chain.view = Some(view);
            return Ok(());
        }
        Err(error) => error,
    };
    // SAFETY: failed view construction left no outstanding backbuffer view;
    // the immediate context was unbound before the original ResizeBuffers.
    unsafe {
        chain.native.ResizeBuffers(
            0,
            previous_size.width(),
            previous_size.height(),
            DXGI_FORMAT_UNKNOWN,
            DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
        )
    }
    .map_err(|rollback| {
        Error::Backend(format!("{error}; backbuffer rollback failed: {rollback}"))
    })?;
    chain.generation = chain.generation.wrapping_add(1);
    chain.config.size = previous_size;
    chain.source_extent = previous_source;
    chain.view = Some(backbuffer_view(device, &chain.native).map_err(|rollback| {
        Error::Backend(format!(
            "{error}; restored backbuffer view failed: {rollback}"
        ))
    })?);
    Err(error)
}
