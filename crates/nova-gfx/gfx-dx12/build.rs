//! Offline DX12 indirect argument conversion shader (never compiled at runtime).
use gfx_shader_build::{BackendSelection, Dx12ArtifactPolicy, Shader, ShaderSet, ShaderStage};

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let shaders = ShaderSet::new("native_indirect")
        .backend_selection(BackendSelection::Platform)
        .dx12_artifact_policy(Dx12ArtifactPolicy::RequireBytecode)
        .shader(Shader::new("argument_pack")
            .wgsl_file("src/device/platform/native_indirect_pack.wgsl")
            .target_os("windows")
            .entry("cs_pack_native_indirect", ShaderStage::Compute));
    if let Err(error) = shaders.emit() {
        println!("cargo::error={error}");
        std::process::exit(1);
    }
}
