//! SM5.0 resource declarations for Naga's HLSL output.
//!
//! Naga 29 emits DX12 sampler heaps even for SM5.0. Only those known generated
//! declarations and SM5.1 constant-buffer syntax are rewritten; shader expressions,
//! buffer accesses and math stay intact.
//! Reject any leftover heap/index-buffer/register-space reference before FXC compilation.

use crate::{Result, ShaderError};

pub(super) fn declarations(source: &str) -> Result<String> {
    let mut result = String::with_capacity(source.len());
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("SamplerState nagaSamplerHeap[2048]")
            || trimmed.starts_with("SamplerComparisonState nagaComparisonSamplerHeap[2048]")
            || trimmed.starts_with("StructuredBuffer<uint> nagaGroup0SamplerIndexArray : register(")
        {
            continue;
        }
        if let Some(declaration) = trimmed.strip_prefix("ConstantBuffer<") {
            let (ty, rest) = declaration.split_once("> ").ok_or_else(unsupported)?;
            let (name, register) = rest.split_once(": register(b").ok_or_else(unsupported)?;
            let name = name.trim_end();
            let register = register.replace(", space0", "");
            let slot = register
                .strip_suffix(");")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|slot| {
                    *slot < 13 || (*slot == 13 && ty == "NagaConstants" && name == "_NagaConstants")
                })
                .ok_or_else(unsupported)?;
            result.push_str(&format!(
                "cbuffer {name}_binding : register(b{slot}) {{ {ty} {name}; }};\n"
            ));
        } else if let Some(declaration) = trimmed.strip_prefix("static const Sampler") {
            let (left, right) = declaration.split_once(" = ").ok_or_else(unsupported)?;
            let (kind, name) = left.split_once(' ').ok_or_else(unsupported)?;
            if !matches!(kind, "State" | "ComparisonState") {
                return Err(unsupported());
            }
            let heap = if kind == "State" {
                "nagaSamplerHeap"
            } else {
                "nagaComparisonSamplerHeap"
            };
            let prefix = format!("{heap}[nagaGroup0SamplerIndexArray[");
            let slot = right
                .strip_prefix(&prefix)
                .and_then(|value| value.strip_suffix("]];"))
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|slot| *slot < 16)
                .ok_or_else(unsupported)?;
            result.push_str(&format!("Sampler{kind} {name} : register(s{slot});\n"));
        } else {
            result.push_str(&line.replace(", space0", ""));
            result.push('\n');
        }
    }
    if result.contains("nagaSamplerHeap")
        || result.contains("nagaComparisonSamplerHeap")
        || result.contains("SamplerIndexArray")
        || result.contains(", space")
        || result.contains("ConstantBuffer<")
    {
        return Err(unsupported());
    }
    Ok(result)
}

fn unsupported() -> ShaderError {
    ShaderError::Hlsl(
        "unsupported D3D11 resource declaration; expected fixed group-zero bindings".into(),
    )
}

#[cfg(test)]
mod tests {
    use crate::{ShaderStage, WgslModule};
    use gfx_core::ShaderCode;

    #[test]
    fn fixed_sampler_keeps_its_native_slot_and_draw_offsets() {
        let module = WgslModule::parse("\
            @group(0) @binding(4) var image: texture_2d<f32>;\n\
            @group(0) @binding(5) var image_sampler: sampler;\n\
            @fragment fn fs() -> @location(0) vec4<f32> { return textureSample(image, image_sampler, vec2<f32>(0.5)); }")
            .expect("valid WGSL");
        let ShaderCode::Hlsl(source) = module
            .compile_hlsl_dx11(ShaderStage::Fragment, "fs")
            .expect("D3D11 lowering")
            .code
        else {
            panic!("expected HLSL")
        };
        assert!(source.contains("register(s5)"));
        assert!(source.contains("register(t4)"));
        assert!(!source.contains("SamplerHeap"));
        assert!(!source.contains("space"));
    }
    #[test]
    fn reserved_constant_slot_is_rejected() {
        let module = WgslModule::parse("@group(0) @binding(13) var<uniform> v: vec4<f32>; @fragment fn fs() -> @location(0) vec4<f32> { return v; }").expect("valid WGSL");
        assert!(
            module
                .compile_hlsl_dx11(ShaderStage::Fragment, "fs")
                .is_err()
        );
    }
    #[test]
    fn unknown_heap_lowering_is_rejected() {
        assert!(
            super::declarations("static const SamplerState s = nagaSamplerHeap[dynamic_index];")
                .is_err()
        );
    }
    #[test]
    fn uniform_keeps_struct_layout_and_member_access() {
        let module = WgslModule::parse("struct Params { color: vec4<f32> }; @group(0) @binding(9) var<uniform> params: Params; @fragment fn fs() -> @location(0) vec4<f32> { return params.color; }").expect("valid WGSL");
        let ShaderCode::Hlsl(source) = module
            .compile_hlsl_dx11(ShaderStage::Fragment, "fs")
            .expect("SM5.0 uniform")
            .code
        else {
            panic!("expected HLSL")
        };
        assert!(source.contains("register(b9)"));
        assert!(source.contains("Params params;"));
        assert!(source.contains("params.color"));
        assert!(!source.contains("ConstantBuffer<"));
    }
}
