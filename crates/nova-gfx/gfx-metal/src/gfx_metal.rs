//! Metal backend for nova-gfx.
//!
//! This crate implements the `gfx-core` device traits for Metal on Apple
//! targets. Non-Apple builds expose a minimal stub that returns
//! `GfxError::Unavailable`.
//!
//! Chinese documentation is available in `README.zh-CN.md` in the crate source
//! package.

mod device;
mod error;
#[cfg(target_vendor = "apple")]
mod registry;

pub use device::*;
pub use error::MetalError;
