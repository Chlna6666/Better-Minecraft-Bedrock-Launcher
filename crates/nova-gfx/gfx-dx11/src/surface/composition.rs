//! Keep the displayed composition content alive until a candidate has a valid first frame.
use super::*;

pub(super) struct Composition {
    pub(super) device: IDCompositionDesktopDevice,
    target: IDCompositionTarget,
    pub(super) visual: IDCompositionVisual,
    active: Option<IDXGISwapChain3>,
    fallback_hwnd: Option<IDXGISwapChain3>,
}

impl Composition {
    pub(super) fn publish(&mut self, candidate: &IDXGISwapChain3) -> Result<()> {
        if self.active.as_ref() == Some(candidate) {
            return Ok(());
        }
        // SAFETY: the candidate has successfully presented a matching frame. Both native
        // generations stay owned until Commit or rollback completes on this device owner.
        let result = unsafe {
            self.visual
                .SetContent(candidate)
                .and_then(|()| self.target.SetRoot(&self.visual))
                .and_then(|()| self.device.Commit())
        };
        if let Err(error) = result {
            let rollback = unsafe {
                match &self.active {
                    Some(active) => self
                        .visual
                        .SetContent(active)
                        .and_then(|()| self.target.SetRoot(&self.visual)),
                    None => self.target.SetRoot(None),
                }
                .and_then(|()| self.device.Commit())
            };
            if let Err(rollback_error) = rollback {
                log::warn!("D3D11 composition content rollback failed: {rollback_error}");
            }
            return Err(backend(error));
        }
        self.active = Some(candidate.clone());
        self.fallback_hwnd = None;
        Ok(())
    }

    pub(super) fn retain_hwnd(&mut self, content: IDXGISwapChain3) {
        self.fallback_hwnd = Some(content);
    }

    pub(super) fn detach(&self) -> Result<()> {
        // SAFETY: target, visual and device remain live on the native owner.
        let result = unsafe {
            self.target
                .SetRoot(None)
                .and_then(|()| self.device.Commit())
        };
        if let Err(error) = result {
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

pub(super) fn create(native: &ID3D11Device, hwnd: HWND) -> Result<Composition> {
    let dxgi: IDXGIDevice = native.cast().map_err(backend)?;
    // SAFETY: all COM inputs are live and hwnd outlives the unattached target/visual.
    unsafe {
        let device: IDCompositionDesktopDevice =
            DCompositionCreateDevice2(&dxgi).map_err(backend)?;
        let target = device.CreateTargetForHwnd(hwnd, true).map_err(backend)?;
        let visual = device.CreateVisual().map_err(backend)?;
        Ok(Composition {
            device,
            target,
            visual: visual.into(),
            active: None,
            fallback_hwnd: None,
        })
    }
}

pub(super) fn prepare_resize(
    device: &mut Dx11Device,
    id: SwapchainId,
    size: Extent2d,
) -> Result<()> {
    let old = device.swapchains.get(id)?;
    // SAFETY: the old native chain and factory belong to this live device owner.
    let mut descriptor = unsafe { old.native.GetDesc1() }.map_err(backend)?;
    descriptor.Width = size.width();
    descriptor.Height = size.height();
    let native: IDXGISwapChain3 = unsafe {
        device
            .factory
            .CreateSwapChainForComposition(&device.native, &descriptor, None)
    }
    .map_err(backend)?
    .cast()
    .map_err(backend)?;
    unsafe { native.SetMaximumFrameLatency(1) }.map_err(backend)?;
    let latency = unsafe { native.GetFrameLatencyWaitableObject() };
    // Own the wait handle immediately so failed RTV preparation closes it as well.
    let mut next = Swapchain {
        native,
        view: None,
        config: SurfaceConfig { size, ..old.config },
        source_extent: size,
        surface: old.surface,
        composition: None,
        latency,
        pacing: FrameLatencyWait::default(),
        generation: old.generation.wrapping_add(1),
        resize_retirement: None,
    };
    if next.latency.is_invalid() {
        return Err(Error::Backend(
            "DXGI returned no frame latency handle".into(),
        ));
    }
    next.view = Some(backbuffer_view(&device.native, &next.native)?);
    let old = device.swapchains.get_mut(id)?;
    next.composition = old.composition.take();
    // The composition still owns the last successfully published native chain.
    // Superseding an unpresented candidate never replaces that active content.
    let previous = std::mem::replace(old, next);
    drop(previous);
    Ok(())
}
