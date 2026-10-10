//! Native D3D11 rendering with build-time SM5.0 bytecode.
//!
//! Devices and their immediate context belong to one graphics owner. Resource group zero
//! maps directly to D3D11 b/t/s slots; b13 is reserved for WGSL draw offsets. Native
//! resources are retained by D3D11 while submitted commands use them. Offscreen passes,
//! mip uploads, indexed instancing, depth and transparent DirectComposition surfaces
//! share this device; there is no runtime shader compiler or DX12 compatibility layer.

#[cfg(windows)]
#[expect(unsafe_code, reason = "Direct3D 11 native COM/Win32 calls require audited unsafe FFI boundaries")]
mod commands;
#[cfg(windows)]
#[expect(unsafe_code, reason = "Direct3D 11 native COM/Win32 calls require audited unsafe FFI boundaries")]
mod device;
#[cfg(windows)]
#[expect(unsafe_code, reason = "Direct3D 11 native COM/Win32 calls require audited unsafe FFI boundaries")]
mod frame_pacing;
#[cfg(windows)]
#[expect(unsafe_code, reason = "Direct3D 11 native COM/Win32 calls require audited unsafe FFI boundaries")]
mod pipelines;
#[cfg(windows)]
mod registry;
#[cfg(windows)]
#[expect(unsafe_code, reason = "Direct3D 11 native COM/Win32 calls require audited unsafe FFI boundaries")]
mod resources;
#[cfg(windows)]
#[expect(unsafe_code, reason = "Direct3D 11 native COM/Win32 calls require audited unsafe FFI boundaries")]
mod surface;
#[cfg_attr(windows, expect(unsafe_code, reason = "Direct3D 11 texture readback uses native COM and mapped pointers"))]
mod transfer;

#[cfg(windows)]
pub use device::{Dx11Device, enumerate_adapter_info};

#[cfg(all(test, windows))]
mod tests;
