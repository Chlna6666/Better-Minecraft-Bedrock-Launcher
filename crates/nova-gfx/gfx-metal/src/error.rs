use gfx_core::GfxError;
use thiserror::Error;

/// Metal-specific error.
#[derive(Debug, Error)]
pub enum MetalError {
    /// A Metal resource was unavailable.
    #[error("Metal resource unavailable: {0}")]
    Unavailable(String),
    /// A Metal operation failed.
    #[error("Metal backend error: {0}")]
    Backend(String),
}

impl From<MetalError> for GfxError {
    fn from(error: MetalError) -> Self {
        match error {
            MetalError::Unavailable(message) => Self::Unavailable(message),
            MetalError::Backend(message) => Self::Backend(message),
        }
    }
}
