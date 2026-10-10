//! OpenGL backend for nova-gfx.
//!
//! Desktop OpenGL 4.5 through native WGL on Windows and EGL on Linux.
//! The device, contexts and resource operations belong to the graphics owner.
//! WGSL is translated at build time; the native driver compiles and links GLSL.

#[cfg(any(windows, target_os = "linux"))]
#[expect(unsafe_code, reason = "OpenGL context and glow driver calls require audited unsafe FFI boundaries")]
mod bindings;
#[cfg(target_os = "windows")]
#[expect(unsafe_code, reason = "Win32 WGL bootstrap window lifecycle requires unsafe FFI")]
mod bootstrap;
#[cfg(any(windows, target_os = "linux"))]
#[expect(unsafe_code, reason = "OpenGL context and glow driver calls require audited unsafe FFI boundaries")]
mod commands;
#[cfg(any(windows, target_os = "linux"))]
#[expect(unsafe_code, reason = "OpenGL context and glow driver calls require audited unsafe FFI boundaries")]
mod device;
#[cfg(any(windows, target_os = "linux"))]
#[expect(unsafe_code, reason = "OpenGL context and glow driver calls require audited unsafe FFI boundaries")]
mod pipelines;
#[cfg(any(windows, target_os = "linux"))]
mod registry;
#[cfg(any(windows, target_os = "linux"))]
#[expect(unsafe_code, reason = "OpenGL context and glow driver calls require audited unsafe FFI boundaries")]
mod render;
#[cfg(any(windows, target_os = "linux"))]
#[expect(unsafe_code, reason = "OpenGL context and glow driver calls require audited unsafe FFI boundaries")]
mod resources;
#[cfg(any(windows, target_os = "linux"))]
#[expect(unsafe_code, reason = "OpenGL context and glow driver calls require audited unsafe FFI boundaries")]
mod surface;

#[cfg(any(windows, target_os = "linux"))]
pub use device::OpenGlDevice;

#[cfg(all(test, target_os = "windows"))]
mod tests;
