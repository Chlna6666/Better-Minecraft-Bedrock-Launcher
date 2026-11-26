//! Build-generated Nova shader artifacts.
//!
//! `build.rs` declares the production Nova entry points through `gfx-shader-build`,
//! which translates them for the backends this build enables and, when the build host
//! can run the Direct3D compiler, compiles DX12 shaders all the way to Direct3D
//! bytecode. The generated table below therefore serves ready-to-use
//! `gfx_core::EmbeddedShader` values, so no shader compiler runs while the application
//! starts.
//!
//! Besides one `nova_<backend>_shader` lookup per enabled backend, the table carries a
//! `NOVA_<BACKEND>_SHADER_ARTIFACT_KIND` description that the renderer logs once, so an
//! application log states which shader path a binary actually uses.

include!(concat!(env!("OUT_DIR"), "/shaders_bytes.rs"));
