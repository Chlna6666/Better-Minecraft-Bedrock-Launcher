//! Build-generated scene-view shader artifacts.
//!
//! `build.rs` compiles the scene-view WGSL for the backends this build enables, so
//! creating a scene view never runs a shader compiler. A binary that was
//! cross-compiled without a Direct3D compiler embeds HLSL instead and reports that
//! through `SCENE_VIEW_DX12_SHADER_ARTIFACT_KIND`.

include!(concat!(env!("OUT_DIR"), "/shaders_bytes.rs"));
