//! Native D3D11 rendering with build-time SM5.0 bytecode.
//!
//! Devices and their immediate context belong to one graphics owner. Resource group zero
//! maps directly to D3D11 b/t/s slots; b13 is reserved for WGSL draw offsets. Native
//! resources are retained by D3D11 while submitted commands use them. Offscreen passes,
//! mip uploads, indexed instancing, depth and transparent DirectComposition surfaces
//! share this device; there is no runtime shader compiler or DX12 compatibility layer.

#[cfg(windows)]
mod commands;
#[cfg(windows)]
mod device;
#[cfg(windows)]
mod frame_pacing;
#[cfg(windows)]
mod pipelines;
#[cfg(windows)]
mod registry;
#[cfg(windows)]
mod resources;
#[cfg(windows)]
mod surface;
mod transfer;

#[cfg(windows)]
pub use device::{Dx11Device, enumerate_adapter_info};

#[cfg(all(test, windows))]
mod tests;
