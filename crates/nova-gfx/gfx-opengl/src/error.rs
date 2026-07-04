//! OpenGL backend errors.

use thiserror::Error;

/// Errors produced by the OpenGL backend.
#[derive(Debug, Error)]
pub enum OpenGlError {
    /// The backend is not implemented for the current build.
    #[error("OpenGL backend is unavailable: {0}")]
    Unavailable(String),
}
