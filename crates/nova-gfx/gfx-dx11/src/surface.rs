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

pub(crate) struct Composition {
    device: IDCompositionDesktopDevice,
    target: IDCompositionTarget,
    visual: IDCompositionVisual,
}
pub(crate) struct Swapchain {
    pub(crate) native: IDXGISwapChain3,
    pub(crate) view: Option<ID3D11RenderTargetView>,
    pub(crate) config: SurfaceConfig,
    surface: SurfaceId,
    composition: Option<Composition>,
    pub(crate) latency: HANDLE,
    pub(crate) pacing: FrameLatencyWait,
}
impl Composition {
    fn detach(&self) -> Result<()> {
        // SAFETY: target, visual and device are live on their native owner thread.
        let result = unsafe {
            self.target
                .SetRoot(None)
                .and_then(|()| self.device.Commit())
        };
        if let Err(error) = result {
            // Restore the last committed visual before the caller discards the replacement.
            if let Err(restore_error) = unsafe {
                self.target
                    .SetRoot(&self.visual)
                    .and_then(|()| self.device.Commit())
            } {
                log::warn!("D3D11 composition rollback failed: {restore_error}");
            }
            return Err(backend(error));
        }
        Ok(())
    }
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
            Scaling: if composited {
                DXGI_SCALING_STRETCH
            } else {
                DXGI_SCALING_NONE
            },
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
        let mut chain = Swapchain {
            native,
            view: None,
            config,
            surface,
            composition: None,
            latency,
            pacing: FrameLatencyWait::default(),
        };
        if composited {
            chain.composition = Some(composition(&self.native, hwnd, &chain.native)?);
        }
        chain.view = Some(backbuffer_view(&self.native, &chain.native)?);
        Ok(self.swapchains.insert(chain))
    }
    fn destroy_swapchain(&mut self, id: SwapchainId) -> Result<()> {
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
    /// # Errors
    /// Returns a native resize error; existing buffers are reacquired on failure.
    pub fn resize_swapchain(&mut self, id: SwapchainId, width: u32, height: u32) -> Result<()> {
        let size = Extent2d::new(width, height)?;
        let chain = self.swapchains.get_mut(id)?;
        chain.pacing.cancel();
        // SAFETY: clear indirect references before DXGI ResizeBuffers; immediate commands
        // retain native resources until execution retires, and DXGI synchronizes the resize.
        unsafe {
            self.context.ClearState();
            self.context.Flush();
        }
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
        chain.view = Some(backbuffer_view(&self.native, &chain.native)?);
        result.map_err(backend)?;
        chain.config.size = size;
        chain.pacing.reset();
        self.stretch(id, None)
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
            let next = SurfaceDevice::create_swapchain(self, surface, config)?;
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
        let chain = self.swapchains.get(id)?;
        if let Some(composition) = &chain.composition {
            let scale = scale.unwrap_or([1.0, 1.0]);
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
        }
        Ok(())
    }
    pub(crate) fn present_steps(
        &mut self,
        id: SwapchainId,
        pass: RenderPassId,
        steps: RenderStepList<'_>,
        color: ClearColor,
        depth: Option<RenderPassDepthAttachment>,
    ) -> Result<SubmissionId> {
        // D3D11 rotates the identity of buffer zero at Present. Its retained view always
        // targets the next writable image; D3D12's physical image indexing does not apply.
        self.render_target(
            RenderTarget::Swapchain {
                swapchain: id,
                image_index: 0,
            },
            pass,
            steps,
            LoadOp::Clear(color),
            depth,
        )?;
        let chain = self.swapchains.get_mut(id)?;
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
        self.signal()
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

fn composition(native: &ID3D11Device, hwnd: HWND, chain: &IDXGISwapChain3) -> Result<Composition> {
    let dxgi: IDXGIDevice = native.cast().map_err(backend)?;
    // SAFETY: all COM inputs are live and hwnd outlives this surface/visual.
    unsafe {
        let device: IDCompositionDesktopDevice =
            DCompositionCreateDevice2(&dxgi).map_err(backend)?;
        let target = device.CreateTargetForHwnd(hwnd, true).map_err(backend)?;
        let visual = device.CreateVisual().map_err(backend)?;
        visual.SetContent(chain).map_err(backend)?;
        target.SetRoot(&visual).map_err(backend)?;
        device.Commit().map_err(backend)?;
        Ok(Composition {
            device,
            target,
            visual: visual.into(),
        })
    }
}
