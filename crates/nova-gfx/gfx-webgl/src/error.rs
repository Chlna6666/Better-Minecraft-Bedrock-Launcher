//! WebGL backend errors.

use thiserror::Error;

/// Errors produced by the WebGL backend.
#[derive(Debug, Error)]
pub enum WebGlError {
    /// The backend is not implemented for the current build.
    #[error("WebGL backend is unavailable: {0}")]
    Unavailable(String),
}
