//! Keep the displayed composition content alive until a candidate has a valid first frame.
use super::*;

pub(super) struct Composition {
    pub(super) device: IDCompositionDesktopDevice,
    target: IDCompositionTarget,
    pub(super) visual: IDCompositionVisual,
    pub(super) scale: [f32; 2],
    active: Option<IDXGISwapChain3>,
    fallback_hwnd: Option<IDXGISwapChain3>,
}

impl Composition {
    pub(super) fn publish(&mut self, candidate: &IDXGISwapChain3) -> Result<()> {
        if self.active.as_ref() == Some(candidate) && self.scale == [1.0, 1.0] {
            return Ok(());
        }
        // SAFETY: the candidate has successfully presented a matching frame. Both native
        // generations stay owned until Commit or rollback completes on this device owner.
        let result = self.commit_content(Some(candidate), [1.0, 1.0]);
        if let Err(error) = result {
            let rollback = self.commit_content(self.active.as_ref(), self.scale);
            if let Err(rollback_error) = rollback {
                log::warn!("D3D11 composition content rollback failed: {rollback_error}");
            }
            return Err(backend(error));
        }
        self.active = Some(candidate.clone());
        self.scale = [1.0, 1.0];
        self.fallback_hwnd = None;
        Ok(())
    }

    fn commit_content(
        &self,
        content: Option<&IDXGISwapChain3>,
        scale: [f32; 2],
    ) -> windows::core::Result<()> {
        // SAFETY: content and transform belong to this live composition owner.
        // Publish both in one transaction so new pixels never inherit the old stretch.
        unsafe {
            self.visual.SetTransform2(&windows_numerics::Matrix3x2 {
                M11: scale[0],
                M22: scale[1],
                ..Default::default()
            })?;
            if let Some(content) = content {
                self.visual.SetContent(content)?;
                self.target.SetRoot(&self.visual)?;
            } else {
                self.target.SetRoot(None)?;
            }
            self.device.Commit()
        }
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
            scale: [1.0, 1.0],
            active: None,
            fallback_hwnd: None,
        })
    }
}
