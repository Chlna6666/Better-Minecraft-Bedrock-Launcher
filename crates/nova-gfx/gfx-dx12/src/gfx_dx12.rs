//! Direct3D 12 backend for nova-gfx.
//!
//! This crate implements the `gfx-core` device traits for Direct3D 12 on
//! Windows. Non-Windows builds expose a minimal stub that returns
//! `GfxError::Unavailable`.
//!
//! Chinese documentation is available in `README.zh-CN.md` in the crate source
//! package.

mod device;
mod error;
#[cfg(windows)]
mod registry;

pub use device::*;
pub use error::Dx12Error;
