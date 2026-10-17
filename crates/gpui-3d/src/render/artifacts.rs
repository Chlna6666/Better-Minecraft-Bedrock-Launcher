//! Build-generated offscreen renderer shader artifacts.
//!
//! `build.rs` compiles the shadow, scene, and post WGSL entry points for every backend this
//! build enables, so creating an offscreen renderer never runs a shader compiler.

#![allow(
    dead_code,
    reason = "the generated table lists every embedded shader entry point and one artifact-kind description per backend; a given binary only reads its own backend arm"
)]

include!(concat!(env!("OUT_DIR"), "/shaders_bytes.rs"));

use super::RenderError;
use gfx_core::{BackendKind, EmbeddedShader, ShaderBinary, ShaderStage};

pub(super) fn embedded_shader(
    backend: BackendKind,
    stage: ShaderStage,
    entry_point: &str,
) -> Result<ShaderBinary, RenderError> {
    let artifact = match backend {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        BackendKind::OpenGl => scene_view_opengl_shader(entry_point),
        #[cfg(target_os = "windows")]
        BackendKind::Dx11 => scene_view_dx11_shader(entry_point),
        #[cfg(target_os = "windows")]
        BackendKind::Dx12 => scene_view_dx12_shader(entry_point),
        #[cfg(target_os = "macos")]
        BackendKind::Metal => scene_view_metal_shader(entry_point),
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        BackendKind::Vulkan => scene_view_vulkan_shader(entry_point),
        _ => None,
    };
    let artifact: EmbeddedShader = artifact.ok_or_else(|| {
        RenderError::Gpu(gfx_core::Error::Unavailable(format!(
            "offscreen shader entry point `{entry_point}` is not embedded for backend {backend:?}"
        )))
    })?;
    Ok(artifact.to_binary(stage, entry_point)?)
}
