//! WGSL validation and backend shader generation for nova-gfx.
//!
//! This crate emits `gfx_core::ShaderBinary` payloads for Vulkan, Direct3D 12,
//! and Metal backends.
//!
//! Chinese documentation is available in `README.zh-CN.md` in the crate source
//! package.

use gfx_core::{BackendKind, ShaderBinary, ShaderStage};
use naga::{
    ShaderStage as NagaShaderStage,
    back::{hlsl, msl, spv},
    valid::{Capabilities, ValidationFlags, Validator},
};
use thiserror::Error;

/// Result type used by shader compilation.
pub type Result<T> = std::result::Result<T, ShaderError>;

/// Shader parse, validation, and translation errors.
#[derive(Debug, Error)]
pub enum ShaderError {
    /// WGSL parsing failed.
    #[error("WGSL parse failed: {0}")]
    Parse(String),
    /// Naga validation failed.
    #[error("WGSL validation failed: {0}")]
    Validate(String),
    /// SPIR-V generation failed.
    #[error("SPIR-V generation failed: {0}")]
    Spirv(String),
    /// HLSL generation failed.
    #[error("HLSL generation failed: {0}")]
    Hlsl(String),
    /// MSL generation failed.
    #[error("MSL generation failed: {0}")]
    Msl(String),
}

/// A validated WGSL module and its Naga metadata.
pub struct WgslModule {
    module: naga::Module,
    info: naga::valid::ModuleInfo,
}

impl WgslModule {
    /// Parses and validates WGSL source.
    ///
    /// # Errors
    ///
    /// Returns [`ShaderError`] when parsing or validation fails.
    pub fn parse(source: &str) -> Result<Self> {
        let module = naga::front::wgsl::parse_str(source)
            .map_err(|error| ShaderError::Parse(error.emit_to_string(source)))?;
        let info = Validator::new(ValidationFlags::all(), Capabilities::empty())
            .validate(&module)
            .map_err(|error| ShaderError::Validate(error.to_string()))?;
        Ok(Self { module, info })
    }

    /// Compiles one entry point to SPIR-V.
    ///
    /// # Errors
    ///
    /// Returns [`ShaderError::Spirv`] when the entry point or translation fails.
    pub fn compile_spirv(&self, stage: ShaderStage, entry_point: &str) -> Result<ShaderBinary> {
        let pipeline_options = spv::PipelineOptions {
            shader_stage: shader_stage_to_naga(stage),
            entry_point: entry_point.to_string(),
        };
        let options = spv::Options::default();
        let spirv = spv::write_vec(&self.module, &self.info, &options, Some(&pipeline_options))
            .map_err(|error| ShaderError::Spirv(error.to_string()))?;

        Ok(ShaderBinary::spirv(stage, entry_point, spirv))
    }

    /// Compiles one entry point to HLSL source.
    ///
    /// # Errors
    ///
    /// Returns [`ShaderError::Hlsl`] when the entry point or translation fails.
    pub fn compile_hlsl(&self, stage: ShaderStage, entry_point: &str) -> Result<ShaderBinary> {
        let options = hlsl::Options {
            // D3D12's SV_InstanceID/SV_VertexID are local to the draw call. WGSL's
            // instance_index/vertex_index follow WebGPU/Vulkan semantics and include
            // first_instance/first_vertex, so the DX12 backend supplies them here.
            special_constants_binding: Some(hlsl::BindTarget {
                space: 254,
                register: 0,
                ..hlsl::BindTarget::default()
            }),
            ..hlsl::Options::default()
        };
        let pipeline_options = hlsl::PipelineOptions {
            entry_point: Some((shader_stage_to_naga(stage), entry_point.to_string())),
        };
        let mut source = String::new();
        {
            let mut writer = hlsl::Writer::new(&mut source, &options, &pipeline_options);
            writer
                .write(&self.module, &self.info, None)
                .map_err(|error| ShaderError::Hlsl(error.to_string()))?;
        }

        Ok(ShaderBinary::hlsl(stage, entry_point, source))
    }

    /// Compiles one entry point to Metal Shading Language source.
    ///
    /// # Errors
    ///
    /// Returns [`ShaderError::Msl`] when the entry point or translation fails.
    pub fn compile_msl(&self, stage: ShaderStage, entry_point: &str) -> Result<ShaderBinary> {
        let options = msl::Options::default();
        let pipeline_options = msl::PipelineOptions {
            entry_point: Some((shader_stage_to_naga(stage), entry_point.to_string())),
            ..msl::PipelineOptions::default()
        };
        let (source, _info) =
            msl::write_string(&self.module, &self.info, &options, &pipeline_options)
                .map_err(|error| ShaderError::Msl(error.to_string()))?;

        Ok(ShaderBinary::msl(stage, entry_point, source))
    }

    /// Compiles one entry point to backend-specific code.
    ///
    /// # Errors
    ///
    /// Returns [`ShaderError`] when translation fails for the requested backend.
    pub fn compile_backend(
        &self,
        backend: BackendKind,
        stage: ShaderStage,
        entry_point: &str,
    ) -> Result<ShaderBinary> {
        match backend {
            BackendKind::Vulkan => self.compile_spirv(stage, entry_point),
            BackendKind::Dx12 => self.compile_hlsl(stage, entry_point),
            BackendKind::Metal => self.compile_msl(stage, entry_point),
        }
    }
}

/// Parses and compiles one WGSL entry point to SPIR-V.
///
/// # Errors
///
/// Returns [`ShaderError`] when parsing, validation, or translation fails.
pub fn compile_wgsl_to_spirv(
    source: &str,
    stage: ShaderStage,
    entry_point: &str,
) -> Result<ShaderBinary> {
    WgslModule::parse(source)?.compile_spirv(stage, entry_point)
}

/// Parses and compiles one WGSL entry point to HLSL.
///
/// # Errors
///
/// Returns [`ShaderError`] when parsing, validation, or translation fails.
pub fn compile_wgsl_to_hlsl(
    source: &str,
    stage: ShaderStage,
    entry_point: &str,
) -> Result<ShaderBinary> {
    WgslModule::parse(source)?.compile_hlsl(stage, entry_point)
}

/// Parses and compiles one WGSL entry point to MSL.
///
/// # Errors
///
/// Returns [`ShaderError`] when parsing, validation, or translation fails.
pub fn compile_wgsl_to_msl(
    source: &str,
    stage: ShaderStage,
    entry_point: &str,
) -> Result<ShaderBinary> {
    WgslModule::parse(source)?.compile_msl(stage, entry_point)
}

/// Parses and compiles one WGSL entry point to backend-specific code.
///
/// # Errors
///
/// Returns [`ShaderError`] when parsing, validation, or translation fails.
pub fn compile_wgsl_for_backend(
    source: &str,
    backend: BackendKind,
    stage: ShaderStage,
    entry_point: &str,
) -> Result<ShaderBinary> {
    WgslModule::parse(source)?.compile_backend(backend, stage, entry_point)
}

fn shader_stage_to_naga(stage: ShaderStage) -> NagaShaderStage {
    match stage {
        ShaderStage::Vertex => NagaShaderStage::Vertex,
        ShaderStage::Fragment => NagaShaderStage::Fragment,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRIANGLE_WGSL: &str = r"
        struct VertexOut {
            @builtin(position) position: vec4<f32>,
            @location(0) color: vec3<f32>,
        }

        @vertex
        fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
            var positions = array<vec2<f32>, 3>(
                vec2<f32>(0.0, -0.5),
                vec2<f32>(0.5, 0.5),
                vec2<f32>(-0.5, 0.5),
            );
            var colors = array<vec3<f32>, 3>(
                vec3<f32>(1.0, 0.0, 0.0),
                vec3<f32>(0.0, 1.0, 0.0),
                vec3<f32>(0.0, 0.0, 1.0),
            );
            var out: VertexOut;
            out.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
            out.color = colors[vertex_index];
            return out;
        }

        @fragment
        fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
            return vec4<f32>(in.color, 1.0);
        }
    ";

    const ATLAS_WGSL: &str = r"
        struct Uniforms {
            tint: vec4<f32>,
        }

        struct VertexOut {
            @builtin(position) position: vec4<f32>,
            @location(0) uv: vec2<f32>,
        }

        @group(0) @binding(0)
        var<uniform> uniforms: Uniforms;

        @group(0) @binding(1)
        var atlas_texture: texture_2d<f32>;

        @group(0) @binding(2)
        var atlas_sampler: sampler;

        @vertex
        fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
            var positions = array<vec2<f32>, 6>(
                vec2<f32>(-0.5, -0.5),
                vec2<f32>( 0.5, -0.5),
                vec2<f32>( 0.5,  0.5),
                vec2<f32>(-0.5, -0.5),
                vec2<f32>( 0.5,  0.5),
                vec2<f32>(-0.5,  0.5),
            );
            var uvs = array<vec2<f32>, 6>(
                vec2<f32>(0.0, 1.0),
                vec2<f32>(1.0, 1.0),
                vec2<f32>(1.0, 0.0),
                vec2<f32>(0.0, 1.0),
                vec2<f32>(1.0, 0.0),
                vec2<f32>(0.0, 0.0),
            );
            var out: VertexOut;
            out.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
            out.uv = uvs[vertex_index];
            return out;
        }

        @fragment
        fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
            return textureSample(atlas_texture, atlas_sampler, in.uv) * uniforms.tint;
        }
    ";

    const STORAGE_WGSL: &str = r"
        struct Item {
            position: vec2<f32>,
            color: vec4<f32>,
        }

        struct VertexOut {
            @builtin(position) position: vec4<f32>,
            @location(0) color: vec4<f32>,
        }

        @group(0) @binding(0)
        var<storage, read> items: array<Item>;

        @vertex
        fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
            let item = items[vertex_index];
            var out: VertexOut;
            out.position = vec4<f32>(item.position, 0.0, 1.0);
            out.color = item.color;
            return out;
        }

        @fragment
        fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
            return in.color;
        }
    ";

    #[test]
    fn parses_valid_wgsl() {
        let module = WgslModule::parse(TRIANGLE_WGSL);

        assert!(module.is_ok());
    }

    #[test]
    fn rejects_invalid_wgsl() {
        let module = WgslModule::parse("@vertex fn bad(");

        assert!(module.is_err());
    }

    #[test]
    fn compiles_valid_vertex_entry_point() {
        let binary = compile_wgsl_to_spirv(TRIANGLE_WGSL, ShaderStage::Vertex, "vs_main");

        assert!(binary.is_ok_and(|binary| !binary.is_empty()));
    }

    #[test]
    fn compiles_valid_vertex_entry_point_to_hlsl() {
        let binary = compile_wgsl_to_hlsl(TRIANGLE_WGSL, ShaderStage::Vertex, "vs_main");

        assert!(binary.is_ok_and(|binary| !binary.is_empty()));
    }

    #[test]
    fn compiles_valid_fragment_entry_point_to_msl() {
        let binary = compile_wgsl_to_msl(TRIANGLE_WGSL, ShaderStage::Fragment, "fs_main");

        assert!(binary.is_ok_and(|binary| !binary.is_empty()));
    }

    #[test]
    fn compiles_bound_resources_to_all_backends() {
        let module = WgslModule::parse(ATLAS_WGSL).expect("atlas shader should validate");

        assert!(
            module
                .compile_spirv(ShaderStage::Vertex, "vs_main")
                .is_ok_and(|binary| !binary.is_empty())
        );
        assert!(
            module
                .compile_spirv(ShaderStage::Fragment, "fs_main")
                .is_ok_and(|binary| !binary.is_empty())
        );
        assert!(
            module
                .compile_hlsl(ShaderStage::Fragment, "fs_main")
                .is_ok_and(|binary| !binary.is_empty())
        );
        assert!(
            module
                .compile_msl(ShaderStage::Fragment, "fs_main")
                .is_ok_and(|binary| !binary.is_empty())
        );
    }

    #[test]
    fn hlsl_output_uses_naga_sampler_heap_and_direct_texture_binding() {
        let module = WgslModule::parse(ATLAS_WGSL).expect("atlas shader should validate");
        let binary = module
            .compile_hlsl(ShaderStage::Fragment, "fs_main")
            .expect("atlas fragment shader should compile to HLSL");
        let gfx_core::ShaderCode::Hlsl(source) = binary.code else {
            panic!("expected HLSL shader code");
        };

        assert!(
            source.contains("Texture2D<float4> atlas_texture : register(t1)"),
            "unexpected HLSL output:\n{source}"
        );
        assert!(
            source.contains("SamplerState nagaSamplerHeap[2048]: register(s0, space0)"),
            "unexpected HLSL output:\n{source}"
        );
        assert!(
            source.contains("StructuredBuffer<uint> nagaGroup0SamplerIndexArray"),
            "unexpected HLSL output:\n{source}"
        );
    }

    #[test]
    fn compiles_storage_buffer_resources_to_all_backends() {
        let module = WgslModule::parse(STORAGE_WGSL).expect("storage shader should validate");

        assert!(
            module
                .compile_spirv(ShaderStage::Vertex, "vs_main")
                .is_ok_and(|binary| !binary.is_empty())
        );
        assert!(
            module
                .compile_hlsl(ShaderStage::Vertex, "vs_main")
                .is_ok_and(|binary| !binary.is_empty())
        );
        assert!(
            module
                .compile_msl(ShaderStage::Vertex, "vs_main")
                .is_ok_and(|binary| !binary.is_empty())
        );
    }

    #[test]
    fn hlsl_storage_buffers_use_byte_address_buffer() {
        let module = WgslModule::parse(STORAGE_WGSL).expect("storage shader should validate");
        let binary = module
            .compile_hlsl(ShaderStage::Vertex, "vs_main")
            .expect("storage vertex shader should compile to HLSL");
        let gfx_core::ShaderCode::Hlsl(source) = binary.code else {
            panic!("expected HLSL shader code");
        };

        assert!(
            source.contains("ByteAddressBuffer items : register(t0)"),
            "unexpected HLSL output:\n{source}"
        );
        assert!(
            source.contains("items.Load"),
            "unexpected HLSL output:\n{source}"
        );
    }

    #[test]
    fn hlsl_instance_index_includes_draw_first_instance() {
        let source = r"
            struct VertexOut {
                @builtin(position) position: vec4<f32>,
                @location(0) instance: u32,
            }

            @vertex
            fn vs_main(
                @builtin(vertex_index) vertex_index: u32,
                @builtin(instance_index) instance_index: u32
            ) -> VertexOut {
                var out: VertexOut;
                out.position = vec4<f32>(f32(vertex_index), 0.0, 0.0, 1.0);
                out.instance = instance_index;
                return out;
            }
        ";
        let binary = compile_wgsl_to_hlsl(source, ShaderStage::Vertex, "vs_main")
            .expect("vertex shader should compile to HLSL");
        let gfx_core::ShaderCode::Hlsl(source) = binary.code else {
            panic!("expected HLSL shader code");
        };

        assert!(
            source.contains("ConstantBuffer<NagaConstants> _NagaConstants: register(b0, space254)"),
            "unexpected HLSL output:\n{source}"
        );
        assert!(
            source.contains("_NagaConstants.first_instance"),
            "unexpected HLSL output:\n{source}"
        );
    }
}
