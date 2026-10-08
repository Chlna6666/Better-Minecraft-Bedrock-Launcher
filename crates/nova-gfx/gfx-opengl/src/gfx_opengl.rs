//! OpenGL backend for nova-gfx.
//!
//! Desktop OpenGL 4.5 through native WGL on Windows and EGL on Linux.
//! The device, contexts and resource operations belong to the graphics owner.
//! WGSL is translated at build time; the native driver compiles and links GLSL.

#[cfg(any(windows, target_os = "linux"))]
mod bindings;
#[cfg(target_os = "windows")]
mod bootstrap;
#[cfg(any(windows, target_os = "linux"))]
mod commands;
#[cfg(any(windows, target_os = "linux"))]
mod device;
#[cfg(any(windows, target_os = "linux"))]
mod pipelines;
#[cfg(any(windows, target_os = "linux"))]
mod registry;
#[cfg(any(windows, target_os = "linux"))]
mod render;
#[cfg(any(windows, target_os = "linux"))]
mod resources;
#[cfg(any(windows, target_os = "linux"))]
mod surface;

#[cfg(any(windows, target_os = "linux"))]
pub use device::OpenGlDevice;

#[cfg(all(test, target_os = "windows"))]
mod tests;
