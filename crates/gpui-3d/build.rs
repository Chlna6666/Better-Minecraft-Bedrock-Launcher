//! Compiles the scene-view and offscreen-renderer shaders into embedded backend artifacts.
//!
//! The renderers resolve their shader modules from these generated tables, so creating a scene
//! view or an offscreen renderer never runs the WGSL frontend or the Direct3D compiler.

use gfx_shader_build::{
    BackendSelection, Dx12ArtifactPolicy, MslVersion, Shader, ShaderSet, ShaderStage,
};

fn main() {
    let shaders = ShaderSet::new("scene_view")
        .dx12_artifact_policy(Dx12ArtifactPolicy::RequireBytecode)
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
        )
        .shader(
            Shader::new("offscreen_shadow")
                .wgsl_file("src/render/shadow.wgsl")
                .msl_version(MslVersion::V1_2)
                .entry("vs_shadow_depth", ShaderStage::Vertex)
                .entry("fs_shadow_depth", ShaderStage::Fragment),
        )
        .shader(
            Shader::new("offscreen_scene")
                .wgsl_file("src/render/scene.wgsl")
                .msl_version(MslVersion::V1_2)
                .entry("vs_scene", ShaderStage::Vertex)
                .entry("fs_scene", ShaderStage::Fragment),
        )
        .shader(
            Shader::new("offscreen_post")
                .wgsl_file("src/render/post.wgsl")
                .msl_version(MslVersion::V1_2)
                .entry("vs_post", ShaderStage::Vertex)
                .entry("fs_post", ShaderStage::Fragment),
        )
        .shader(
            Shader::new("offscreen_bloom")
                .wgsl_file("src/render/bloom.wgsl")
                .msl_version(MslVersion::V1_2)
                .entry("vs_bloom", ShaderStage::Vertex)
                .entry("fs_bloom", ShaderStage::Fragment),
        )
        .shader(
            Shader::new("offscreen_sky")
                .wgsl_file("src/render/sky.wgsl")
                .msl_version(MslVersion::V1_2)
                .entry("vs_sky", ShaderStage::Vertex)
                .entry("fs_sky", ShaderStage::Fragment),
        );

    if let Err(error) = shaders.emit() {
        println!("cargo::error={error}");
        std::process::exit(1);
    }
}
