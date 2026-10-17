//! Optional retained-capacity one-to-one presentation; unsupported surfaces keep exact-size swapchains.
use super::*;

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
