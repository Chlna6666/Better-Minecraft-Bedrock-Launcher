//! Build-generated scene-view shader artifacts.
//!
//! `build.rs` compiles the scene-view WGSL for the backends this build enables, so
//! creating a scene view never runs a shader compiler. DX12 uses strict build-time
//! bytecode: a Windows target whose build host cannot run D3DCompile fails instead
//! of embedding HLSL and moving compilation into renderer creation.

include!(concat!(env!("OUT_DIR"), "/shaders_bytes.rs"));
