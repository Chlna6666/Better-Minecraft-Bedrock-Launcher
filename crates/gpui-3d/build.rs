//! Compiles the scene-view and offscreen-renderer shaders into embedded backend artifacts.
//!
//! The renderers resolve their shader modules from these generated tables, so creating a scene
//! view or an offscreen renderer never runs the WGSL frontend or the Direct3D compiler.

use gfx_shader_build::{
    BackendSelection, Dx12ArtifactPolicy, MslVersion, Shader, ShaderSet, ShaderStage,
};

fn main() {
    let shaders = ShaderSet::new("scene_view")
        .shader(
            Shader::new("sidecar_composite")
                .wgsl_file("src/scene_view/sidecar_composite.wgsl")
                .entry("vs_sidecar_composite", ShaderStage::Vertex)
                .entry("fs_sidecar_composite", ShaderStage::Fragment),
        )
        .shader(
            Shader::new("screen_effects")
                .wgsl(concat!(
                    include_str!("src/pbr.wgsl"),
                    "\n",
                    include_str!("src/scene_view/effects/frame.wgsl"),
                    "\n",
                    include_str!("src/scene_view/effects/screen_depth.wgsl"),
                    "\n",
                    include_str!("src/scene_view/effects/screen.wgsl")
                ))
                .entry("vs_effects", ShaderStage::Vertex)
                .entry("fs_effects", ShaderStage::Fragment)
                .entry("fs_composite", ShaderStage::Fragment),
        )
        // Same body, but the depth inputs are `texture_multisampled_2d`, so the screen
        // composite can read the multisampled geometry depth directly at sample zero.
        .shader(
            Shader::new("screen_effects_ms")
                .wgsl(concat!(
                    include_str!("src/pbr.wgsl"),
                    "\n",
                    include_str!("src/scene_view/effects/frame.wgsl"),
                    "\n",
                    include_str!("src/scene_view/effects/screen_depth_ms.wgsl"),
                    "\n",
                    include_str!("src/scene_view/effects/screen.wgsl"),
                    "\n",
                    include_str!("src/scene_view/effects/screen_ms_entries.wgsl")
                ))
                .entry("fs_effects_ms", ShaderStage::Fragment)
                .entry("fs_composite_ms", ShaderStage::Fragment),
        )
        // Depth hierarchy is built by ordered GPU raster passes after the
        // opaque geometry pass. It must use real scene depth, NOT the older
        // approximate 128x128 CPU-box culling Hi-Z used for DrawIndirect.
        .shader(
            Shader::new("ssr_hiz")
                .wgsl_file("src/scene_view/effects/ssr_hiz.wgsl")
                .entry("vs_ssr_hiz", ShaderStage::Vertex)
                .entry("fs_ssr_hiz", ShaderStage::Fragment),
        )
        .shader(
            Shader::new("ssr_hiz_ms")
                .wgsl_file("src/scene_view/effects/ssr_hiz_ms.wgsl")
                .entry("fs_ssr_hiz_ms", ShaderStage::Fragment),
        )
        .shader(
            Shader::new("light_tiles")
                .wgsl(concat!(
                    include_str!("src/scene_view/effects/frame.wgsl"),
                    "\n",
                    include_str!("src/scene_view/effects/tiles.wgsl")
                ))
                .entry("cs_clear_light_tiles", ShaderStage::Compute)
                .entry("cs_light_tiles", ShaderStage::Compute),
        )
        .dx12_artifact_policy(Dx12ArtifactPolicy::RequireBytecode)
        // The host application decides which backend renders, and this crate's own
        // features only forward to `gpui`, so artifacts are generated for every backend
        // the platform supports rather than only for requested features.
        .backend_selection(BackendSelection::Platform)
        .shader(
            Shader::new("scene_view")
                .wgsl(concat!(
                    include_str!("src/pbr.wgsl"),
                    "\n",
                    include_str!("src/scene_view.wgsl")
                ))
                // The scene-view shader uses the `instance_id` attribute, which MSL 1.2 adds.
                .msl_version(MslVersion::V1_2)
                .entry("vs_main", ShaderStage::Vertex)
                .entry("fs_main", ShaderStage::Fragment),
        )
        .shader(
            Shader::new("scene_attributes")
                .wgsl(concat!(
                    include_str!("src/pbr.wgsl"),
                    "\n",
                    include_str!("src/scene_view.wgsl")
                ))
                .entry("fs_hdr", ShaderStage::Fragment)
                .entry("fs_scene_surface", ShaderStage::Fragment)
                .entry("fs_scene_albedo", ShaderStage::Fragment)
                .entry("fs_scene_attributes_mrt", ShaderStage::Fragment)
                .entry("fs_scene_shadow", ShaderStage::Fragment),
        )
        .shader(
            Shader::new("cull")
                .wgsl_file("src/scene_view/cull.wgsl")
                .entry("cs_cull", ShaderStage::Compute)
                .entry("cs_cull_singletons", ShaderStage::Compute),
        )
        .shader(
            Shader::new("hiz")
                .wgsl_file("src/scene_view/hiz.wgsl")
                .entry("cs_hiz_clear", ShaderStage::Compute)
                .entry("cs_hiz_raster_32", ShaderStage::Compute)
                .entry("cs_shadow_raster_32", ShaderStage::Compute)
                .entry("cs_hiz_raster_64", ShaderStage::Compute)
                .entry("cs_shadow_raster_64", ShaderStage::Compute)
                .entry("cs_hiz_raster_128", ShaderStage::Compute)
                .entry("cs_shadow_raster_128", ShaderStage::Compute)
                .entry("cs_hiz_reduce", ShaderStage::Compute),
        )
        .shader(
            Shader::new("mesher")
                .wgsl_file("src/scene_view/mesher.wgsl")
                .entry("cs_mesh", ShaderStage::Compute)
                .entry("cs_mesh_static_counts", ShaderStage::Compute)
                .entry("cs_mesh_static_emit", ShaderStage::Compute)
                .entry("cs_mesh_instance_counts", ShaderStage::Compute)
                .entry("cs_mesh_instance_scan", ShaderStage::Compute)
                .entry("cs_mesh_instance_emit", ShaderStage::Compute)
                .entry("cs_mesh_greedy_mask", ShaderStage::Compute)
                .entry("cs_mesh_rows", ShaderStage::Compute),
        )
        .shader(
            Shader::new("mesher_draws")
                .wgsl_file("src/scene_view/mesher_draws.wgsl")
                .entry("cs_mesh_draws", ShaderStage::Compute),
        )
        .shader(
            Shader::new("mesher_check")
                .wgsl_file("src/scene_view/mesher_check.wgsl")
                .entry("vs_mesh_check", ShaderStage::Vertex)
                .entry("fs_mesh_check", ShaderStage::Fragment)
                .entry("fs_shadow_check", ShaderStage::Fragment)
                .entry("fs_mesh_ao_check", ShaderStage::Fragment),
        )
        .shader(
            Shader::new("voxel")
                .wgsl(concat!(
                    include_str!("src/pbr.wgsl"),
                    "\n",
                    include_str!("src/scene_view/voxel.wgsl"),
                    "\n",
                    include_str!("src/scene_view/voxel_lighting.wgsl")
                ))
                .entry("vs_voxel", ShaderStage::Vertex)
                .entry("vs_voxel_gpu", ShaderStage::Vertex)
                .entry("vs_generated_voxel", ShaderStage::Vertex)
                .entry("fs_voxel", ShaderStage::Fragment),
        )
        .shader(
            Shader::new("voxel_attributes")
                .wgsl(concat!(
                    include_str!("src/pbr.wgsl"),
                    "\n",
                    include_str!("src/scene_view/voxel.wgsl"),
                    "\n",
                    include_str!("src/scene_view/voxel_lighting.wgsl")
                ))
                .entry("fs_voxel_capture", ShaderStage::Fragment)
                .entry("fs_voxel_surface", ShaderStage::Fragment)
                .entry("fs_voxel_albedo", ShaderStage::Fragment)
                .entry("fs_voxel_attributes_mrt", ShaderStage::Fragment)
                .entry("fs_voxel_shadow", ShaderStage::Fragment),
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
                .wgsl(concat!(
                    include_str!("src/pbr.wgsl"),
                    "\n",
                    include_str!("src/render/scene.wgsl")
                ))
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
