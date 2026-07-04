use gpui::{GpuMesh3dShader, WgslShaderSource};
use std::sync::{Arc, OnceLock};

const SKIN_PREVIEW_SHADER_SOURCE: &str = include_str!("skin_preview.wgsl");

pub(super) fn skin_preview_shader() -> Result<Arc<GpuMesh3dShader>, String> {
    static SHADER: OnceLock<Result<Arc<GpuMesh3dShader>, String>> = OnceLock::new();
    SHADER
        .get_or_init(|| {
            let source = WgslShaderSource::from_source(
                "src/ui/window/skin_pack/skin_preview.wgsl",
                SKIN_PREVIEW_SHADER_SOURCE,
            )
            .map_err(|error| error.to_string())?;
            Ok(Arc::new(GpuMesh3dShader::new(
                Arc::new(source),
                "vs_skin_preview",
                "fs_skin_preview",
            )))
        })
        .clone()
}
