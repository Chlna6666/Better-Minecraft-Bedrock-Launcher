use ash::vk;
use gfx_core::GfxError;
use thiserror::Error;

/// Vulkan-specific error.
#[derive(Debug, Error)]
pub enum VulkanError {
    /// A Vulkan loader operation failed.
    #[error("Vulkan loader error: {0}")]
    Loader(String),
    /// A Vulkan call returned an error result.
    #[error("Vulkan error: {0:?}")]
    Vk(vk::Result),
    /// A requested Vulkan resource was unavailable.
    #[error("Vulkan resource unavailable: {0}")]
    Unavailable(String),
}

impl From<vk::Result> for VulkanError {
    fn from(error: vk::Result) -> Self {
        Self::Vk(error)
    }
}

impl From<VulkanError> for GfxError {
    fn from(error: VulkanError) -> Self {
        match error {
            VulkanError::Loader(message) | VulkanError::Unavailable(message) => {
                Self::Backend(message)
            }
            VulkanError::Vk(error) => Self::Backend(format!("{error:?}")),
        }
    }
}
