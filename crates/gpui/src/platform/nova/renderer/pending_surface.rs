//! Independent pending Depth/Path attachments for native surface resize.
//! These targets do not refer to Swapchain images and can be allocated while
//! Active still has GPU readers. Publication requires a matching WSI extent.
use super::*;

pub(super) struct PendingSurfaceAttachments {
    pub(super) size: DrawableSize,
    pub(super) path_size: Extent2d,
    pub(super) path: Option<PathMaskTarget>,
    pub(super) depth: (TextureId, TextureViewId),
}

impl PendingSurfaceAttachments {
    pub(super) fn matches(
        &self,
        size: DrawableSize,
        path_size: Extent2d,
        needs_new_path: bool,
    ) -> bool {
        self.size == size
            && self.path_size == path_size
            && self.path.is_some() == needs_new_path
    }
}

fn create<D: BackendResources + BackendPipelines>(
    device: &mut D,
    label: &str,
    size: DrawableSize,
    path_size: Extent2d,
    needs_new_path: bool,
    descriptor: PathMaskTargetDescriptor,
) -> Result<PendingSurfaceAttachments> {
    let path = if needs_new_path {
        Some(create_path_mask_target(device, label, descriptor)?)
    } else {
        None
    };
    let extent = Extent2d::new(size.width, size.height)?;
    let depth = match create_depth_target(device, label, extent) {
        Ok(depth) => depth,
        Err(error) => {
            if let Some(path) = path {
                destroy_path_mask_target(device, path, label);
            }
            return Err(error);
        }
    };
    Ok(PendingSurfaceAttachments { size, path_size, path, depth })
}

pub(super) fn destroy<D: BackendResources + BackendPipelines>(
    device: &mut D,
    backend: &str,
    pending: PendingSurfaceAttachments,
) {
    if let Some(path) = pending.path {
        destroy_path_mask_target(device, path, backend);
    }
    destroy_depth_target(device, pending.depth.0, pending.depth.1, backend);
}

pub(super) struct SurfaceAttachments {
    pub(super) path: PathMaskTarget,
    pub(super) depth: (TextureId, TextureViewId),
}

/// WSI Resize is the transaction boundary. A prepared bundle has no readers,
/// but it can only be used when BOTH the native drawable and the path-mask
/// extent match. Vulkan may resolve a different physical image size.
pub(super) fn acquire_for_resize<D: BackendResources + BackendPipelines>(
    device: &mut D,
    backend: &str,
    size: DrawableSize,
    path_size: Extent2d,
    old_path: PathMaskTarget,
    needs_new_path: bool,
    descriptor: PathMaskTargetDescriptor,
    staged: &mut Option<PendingSurfaceAttachments>,
) -> Result<SurfaceAttachments> {
    if let Some(pending) = staged.take() {
        if pending.matches(size, path_size, needs_new_path) {
            log::trace!(
                target: "gpui::diagnostics",
                "nova prepared surface attachments installed: physical_px={}x{} path_new={}",
                size.width, size.height, needs_new_path,
            );
            return Ok(SurfaceAttachments {
                path: pending.path.unwrap_or(old_path),
                depth: pending.depth,
            });
        }
        // Superseded WM_SIZE or Vulkan WSI extent. Never stretch these buffers.
        log::trace!(
            target: "gpui::diagnostics",
            "nova prepared surface attachments discarded: prepared_px={}x{} actual_px={}x{}",
            pending.size.width, pending.size.height, size.width, size.height,
        );
        destroy(device, backend, pending);
    }
    let created = create(device, backend, size, path_size, needs_new_path, descriptor)?;
    Ok(SurfaceAttachments {
        path: created.path.unwrap_or(old_path),
        depth: created.depth,
    })
}

impl NovaRenderer {
    /// Prepare once per window while its previous native GPU readers are
    /// in flight. Do not mutate Active path resource sets or its depth view.
    pub(super) fn prepare_pending_surface_attachments(
        &mut self,
        requested: DrawableSize,
    ) -> Result<()> {
        if self.pending_surface_attachments.is_some() {
            return Ok(());
        }
        let extent = Extent2d::new(requested.width, requested.height)?;
        let path_size =
            super::surface_lifecycle::planned_path_mask_size(
                self.path_texture_size,
                self.current_size,
                extent,
            )?;
        let needs_new_path = path_size != self.path_texture_size;
        let descriptor = self.path_mask_target_descriptor(path_size);
        let pending: PendingSurfaceAttachments = match &mut *lock_backend(&self.backend) {
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) =>
                create(device, "DX11", requested, path_size, needs_new_path, descriptor)?,
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) =>
                create(device, "DX12", requested, path_size, needs_new_path, descriptor)?,
            #[cfg(all(feature = "nova-gfx-opengl", any(target_os = "windows", target_os = "linux")))]
            NovaBackend::OpenGl(device) =>
                create(device, "OpenGL", requested, path_size, needs_new_path, descriptor)?,
            #[cfg(all(feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")))]
            NovaBackend::Vulkan(device) =>
                create(device, "Vulkan", requested, path_size, needs_new_path, descriptor)?,
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) =>
                create(device, "Metal", requested, path_size, needs_new_path, descriptor)?,
            #[cfg(not(any(
                all(feature = "nova-gfx-dx11", target_os = "windows"),
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-opengl", any(target_os = "windows", target_os = "linux")),
                all(feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")),
                all(feature = "nova-gfx-metal", target_os = "macos")
            )))]
            NovaBackend::Unavailable =>
                anyhow::bail!("pending surface attachments require a Nova backend"),
        };
        log::trace!(
            target: "gpui::diagnostics",
            "nova pending surface attachments prepared: physical_px={}x{} path_px={}x{} path_new={} in_flight={}",
            requested.width, requested.height,
            path_size.width(), path_size.height(),
            pending.path.is_some(), self.pending_submissions.len(),
        );
        self.pending_surface_attachments = Some(pending);
        Ok(())
    }

    pub(super) fn discard_pending_surface_attachments(&mut self) {
        let Some(pending) = self.pending_surface_attachments.take() else {
            return;
        };
        match &mut *lock_backend(&self.backend) {
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => destroy(device, "DX11", pending),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => destroy(device, "DX12", pending),
            #[cfg(all(feature = "nova-gfx-opengl", any(target_os = "windows", target_os = "linux")))]
            NovaBackend::OpenGl(device) => destroy(device, "OpenGL", pending),
            #[cfg(all(feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")))]
            NovaBackend::Vulkan(device) => destroy(device, "Vulkan", pending),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => destroy(device, "Metal", pending),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx11", target_os = "windows"),
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-opengl", any(target_os = "windows", target_os = "linux")),
                all(feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")),
                all(feature = "nova-gfx-metal", target_os = "macos")
            )))]
            NovaBackend::Unavailable => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_surface_requires_exact_native_and_path_extents() {
        let size = DrawableSize { width: 900, height: 600 };
        let path_size = Extent2d::new(900, 600).unwrap();
        let pending = PendingSurfaceAttachments {
            size, path_size, path: None,
            depth: (TextureId::from_parts(1, 1), TextureViewId::from_parts(2, 1)),
        };
        assert!(pending.matches(size, path_size, false));
        assert!(!pending.matches(DrawableSize { width: 901, height: 600 }, path_size, false));
        assert!(!pending.matches(size, Extent2d::new(901, 600).unwrap(), false));
        assert!(!pending.matches(size, path_size, true),
            "a missing path allocation must not be published when resize needs a new mask");
    }
}
