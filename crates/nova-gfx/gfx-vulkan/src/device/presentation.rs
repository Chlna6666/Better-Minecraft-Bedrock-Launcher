//! Optional retained-capacity one-to-one presentation; unsupported surfaces keep exact-size swapchains.
use super::*;

/// Native presentation references outlive the graphics submit fence. Reuse only signaled
/// present fences and keep every unretired operation attached to its own swapchain.
#[derive(Default)]
pub(super) struct Retirement {
    pending: Vec<vk::Fence>,
    available: Vec<vk::Fence>,
    uncertain: bool,
}

impl Retirement {
    pub(super) fn poll(&mut self, device: &ash::Device) -> Result<bool> {
        if self.uncertain {
            // Let the caller reach the conservative native retirement boundary.
            return Ok(true);
        }
        let mut index = 0;
        while index < self.pending.len() {
            // SAFETY: all pending fences were submitted and remain owned here.
            if unsafe { device.get_fence_status(self.pending[index]) }.map_err(VulkanError::from)? {
                self.available.push(self.pending.swap_remove(index));
            } else {
                index += 1;
            }
        }
        Ok(self.pending.is_empty())
    }

    pub(super) fn acquire(&mut self, device: &ash::Device) -> Result<vk::Fence> {
        if self.uncertain {
            // An indeterminate enqueue must be retired once before further presentation;
            // otherwise completed later fences would accumulate behind that uncertainty.
            unsafe { device.device_wait_idle() }.map_err(VulkanError::from)?;
            self.complete_after_idle();
        }
        self.poll(device)?;
        if let Some(fence) = self.available.pop() {
            // SAFETY: this fence's previous presentation operation completed.
            if let Err(error) = unsafe { device.reset_fences(&[fence]) } {
                self.available.push(fence);
                return Err(VulkanError::from(error).into());
            }
            Ok(fence)
        } else {
            // SAFETY: unsignaled fence for one future queue_present operation.
            unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
                .map_err(|error| VulkanError::from(error).into())
        }
    }

    pub(super) fn record(
        &mut self,
        fence: vk::Fence,
        result: std::result::Result<bool, vk::Result>,
    ) {
        match result {
            // These WSI rejections still enqueue the presentation queue operations.
            Ok(_)
            | Err(
                vk::Result::ERROR_OUT_OF_DATE_KHR
                | vk::Result::ERROR_SURFACE_LOST_KHR
                | vk::Result::ERROR_FULL_SCREEN_EXCLUSIVE_MODE_LOST_EXT
                | vk::Result::SUBOPTIMAL_KHR,
            ) => {
                self.pending.push(fence);
            }
            Err(vk::Result::ERROR_OUT_OF_HOST_MEMORY | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY) => {
                // Enqueue failed; no operation will signal this fence.
                self.available.push(fence);
            }
            Err(_) => {
                self.pending.push(fence);
                self.uncertain = true;
            }
        }
    }

    pub(super) fn needs_idle(&self) -> bool {
        self.uncertain
    }

    pub(super) fn wait(&mut self, device: &ash::Device) -> Result<()> {
        if !self.pending.is_empty() {
            // SAFETY: callers use this only for operations known to have been enqueued.
            unsafe { device.wait_for_fences(&self.pending, true, u64::MAX) }
                .map_err(VulkanError::from)?;
        }
        self.complete_after_idle();
        Ok(())
    }

    pub(super) fn complete_after_idle(&mut self) {
        self.available.append(&mut self.pending);
        self.uncertain = false;
    }

    pub(super) fn destroy(self, device: &ash::Device) {
        for fence in self.pending.into_iter().chain(self.available) {
            // SAFETY: the swapchain's destruction boundary retired all its operations.
            unsafe { device.destroy_fence(fence, None) };
        }
    }
}

impl VulkanDevice {
    pub(super) fn retire_swapchain(&mut self, id: gfx_core::SwapchainId) -> Result<()> {
        let chain = self.swapchains.get_mut(id)?;
        if !self.swapchain_maintenance || chain.presentation.needs_idle() {
            // SAFETY: unsupported/indeterminate native presentation has no finer proof.
            unsafe { self.device.device_wait_idle() }.map_err(VulkanError::from)?;
            chain.presentation.complete_after_idle();
        } else {
            // SAFETY: only this chain's frame slots and present operations are retired.
            unsafe {
                self.device
                    .wait_for_fences(&chain.in_flight_fences, true, u64::MAX)
            }
            .map_err(VulkanError::from)?;
            chain.presentation.wait(&self.device)?;
        }
        let old_fences = chain.in_flight_fences;
        // Retire encoder payloads before their borrowed frame fences are destroyed.
        self.poll_cleanup();
        self.submissions.for_each_live_mut(|submission| {
            if old_fences.contains(&submission.fence) {
                // Preserve the public completion handle after destroying its native fence.
                submission.fence = vk::Fence::null();
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod retirement_tests {
    use super::*;
    use ash::vk::Handle as _;

    #[test]
    fn wsi_rejection_keeps_the_enqueued_present_retirement() {
        for result in [
            Ok(false),
            Ok(true),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR),
            Err(vk::Result::ERROR_SURFACE_LOST_KHR),
        ] {
            let mut retirement = Retirement::default();
            let fence = vk::Fence::from_raw(1);
            retirement.record(fence, result);
            assert_eq!(retirement.pending, [fence]);
            assert!(retirement.available.is_empty());
            assert!(!retirement.needs_idle());
        }
    }

    #[test]
    fn failed_enqueue_does_not_wait_for_an_unsignalable_fence() {
        let mut retirement = Retirement::default();
        let fence = vk::Fence::from_raw(2);
        retirement.record(fence, Err(vk::Result::ERROR_OUT_OF_HOST_MEMORY));
        assert!(retirement.pending.is_empty());
        assert_eq!(retirement.available, [fence]);
    }

    #[test]
    fn uncertain_enqueue_requires_native_retirement_before_reuse() {
        let mut retirement = Retirement::default();
        let fence = vk::Fence::from_raw(3);
        retirement.record(fence, Err(vk::Result::ERROR_UNKNOWN));
        assert!(retirement.needs_idle());
        assert!(retirement.available.is_empty());
        retirement.complete_after_idle();
        assert!(!retirement.needs_idle());
        assert!(retirement.pending.is_empty());
        assert_eq!(retirement.available, [fence]);
    }
}

pub(super) struct Scaling {
    pub(super) info: vk::SwapchainPresentScalingCreateInfoEXT<'static>,
    pub(super) minimum: vk::Extent2D,
    pub(super) maximum: vk::Extent2D,
}

pub(super) fn instance_extension(entry: &Entry) -> Result<Option<&'static CStr>> {
    if !cfg!(target_os = "windows") {
        return Ok(None);
    }
    // SAFETY: instance extension discovery does not submit work or require an instance.
    let extensions = unsafe { entry.enumerate_instance_extension_properties(None) }
        .map_err(VulkanError::from)?;
    let has = |name: &CStr| {
        extensions.iter().any(|extension| {
            // SAFETY: Vulkan supplies null-terminated extension names.
            unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) == name }
        })
    };
    if !has(khr::get_surface_capabilities2::NAME)
        || !has(khr::get_physical_device_properties2::NAME)
    {
        return Ok(None);
    }
    Ok([
        c"VK_KHR_surface_maintenance1",
        ash::ext::surface_maintenance1::NAME,
    ]
    .into_iter()
    .find(|name| has(name)))
}

pub(super) fn device_extension(
    instance_extension: Option<&CStr>,
    extensions: &[vk::ExtensionProperties],
) -> Option<&'static CStr> {
    let name = match instance_extension? {
        name if name == c"VK_KHR_surface_maintenance1" => c"VK_KHR_swapchain_maintenance1",
        _ => ash::ext::swapchain_maintenance1::NAME,
    };
    extensions
        .iter()
        .any(|extension| {
            // SAFETY: Vulkan supplies null-terminated extension names.
            unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) == name }
        })
        .then_some(name)
}

pub(super) fn scaling(
    loader: &khr::get_surface_capabilities2::Instance,
    physical_device: vk::PhysicalDevice,
    surface: vk::SurfaceKHR,
    present_mode: vk::PresentModeKHR,
    extent: vk::Extent2D,
) -> Result<Option<Scaling>> {
    // KHR maintenance1 promotes the EXT structures without changing their wire ABI.
    let mut mode = vk::SurfacePresentModeEXT::default().present_mode(present_mode);
    let info = vk::PhysicalDeviceSurfaceInfo2KHR::default()
        .surface(surface)
        .push_next(&mut mode);
    let mut scaling = vk::SurfacePresentScalingCapabilitiesEXT::default();
    let mut capabilities = vk::SurfaceCapabilities2KHR::default().push_next(&mut scaling);
    // SAFETY: the loader extension was enabled; structures and surface belong to this instance.
    unsafe {
        loader.get_physical_device_surface_capabilities2(physical_device, &info, &mut capabilities)
    }
    .map_err(VulkanError::from)?;
    let one_to_one = vk::PresentScalingFlagsEXT::ONE_TO_ONE;
    let minimum = vk::PresentGravityFlagsEXT::MIN;
    // Win32 surfaces may expose native gravity (zero flags); keep presentation
    // anchored to the top-left so a larger retained image exposes only the client-sized region.
    let gravity = if scaling.supported_present_gravity_x.contains(minimum)
        && scaling.supported_present_gravity_y.contains(minimum)
    {
        Some(minimum)
    } else if cfg!(target_os = "windows")
        && scaling.supported_present_gravity_x.is_empty()
        && scaling.supported_present_gravity_y.is_empty()
    {
        Some(vk::PresentGravityFlagsEXT::empty())
    } else {
        None
    };
    let supported = scaling.supported_present_scaling.contains(one_to_one)
        && gravity.is_some()
        && extent.width >= scaling.min_scaled_image_extent.width
        && extent.height >= scaling.min_scaled_image_extent.height
        && extent.width <= scaling.max_scaled_image_extent.width
        && extent.height <= scaling.max_scaled_image_extent.height;
    Ok(supported.then(|| {
        let gravity = gravity.expect("supported gravity was checked");
        Scaling {
            info: vk::SwapchainPresentScalingCreateInfoEXT::default()
                .scaling_behavior(one_to_one)
                .present_gravity_x(gravity)
                .present_gravity_y(gravity),
            minimum: scaling.min_scaled_image_extent,
            maximum: scaling.max_scaled_image_extent,
        }
    }))
}
