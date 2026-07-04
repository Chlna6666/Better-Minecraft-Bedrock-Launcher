//! OpenGL backend for nova-gfx.
//!
//! This crate currently exposes a stub implementation that reports the backend
//! as unavailable. It keeps the public crate surface stable while the native
//! OpenGL backend is developed.

mod device;
mod error;

pub use device::OpenGlDevice;
pub use error::OpenGlError;
