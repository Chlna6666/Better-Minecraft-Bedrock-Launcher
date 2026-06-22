use gfx_core::GfxError;
use thiserror::Error;

/// Direct3D 12-specific error.
#[derive(Debug, Error)]
pub enum Dx12Error {
    /// A D3D12 resource was unavailable.
    #[error("Direct3D 12 resource unavailable: {0}")]
    Unavailable(String),
    /// A D3D12 operation failed.
    #[error("Direct3D 12 backend error: {0}")]
    Backend(String),
}

impl From<Dx12Error> for GfxError {
    fn from(error: Dx12Error) -> Self {
        match error {
            Dx12Error::Unavailable(message) => Self::Unavailable(message),
            Dx12Error::Backend(message) => Self::Backend(message),
        }
    }
}
