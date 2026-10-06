//! Compiles the scene-view shader into embedded backend artifacts.
//!
//! The renderer resolves its shader modules from this generated table, so creating a
//! scene view never runs the WGSL frontend or the Direct3D compiler.

use gfx_shader_build::{BackendSelection, MslVersion, Shader, ShaderSet, ShaderStage};

fn main() {
    let shaders = ShaderSet::new("scene_view")
        // The host application decides which backend renders, and this crate's own
        // features only forward to `gpui`, so artifacts are generated for every backend
        // the platform supports rather than only for requested features.
        .backend_selection(BackendSelection::Platform)
        .shader(
            Shader::new("scene_view")
                .wgsl_file("src/scene_view.wgsl")
                // The scene-view shader uses the `instance_id` attribute, which MSL 1.2 adds.
                .msl_version(MslVersion::V1_2)
                .entry("vs_main", ShaderStage::Vertex)
                .entry("fs_main", ShaderStage::Fragment),
        );

    if let Err(error) = shaders.emit() {
        println!("cargo::error={error}");
        std::process::exit(1);
    }
}
