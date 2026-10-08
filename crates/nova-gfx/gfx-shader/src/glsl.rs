use crate::{Result, ShaderError, WgslModule};
use gfx_core::{ShaderBinary, ShaderStage};

impl WgslModule {
    /// Translates one WGSL entry point to desktop GLSL 4.50 and resource reflection.
    ///
    /// Only group-zero read-only buffers and sampled textures are supported. Reflection
    /// preserves logical resource slots while the runtime assigns compact native slots.
    /// GL4.5 clip control retains WGSL coordinates; instance offsets use Naga's
    /// dedicated first-instance uniform, without requiring a GL 4.6 extension.
    /// # Errors
    /// Returns a GLSL error for unsupported resources, features or translation failures.
    pub fn compile_glsl(&self, stage: ShaderStage, entry_point: &str) -> Result<ShaderBinary> {
        use gfx_core::{
            GlslBufferBinding, GlslShader, GlslTextureBinding, ResourceBindingType, ShaderCode,
        };
        use naga::back::glsl;
        for (_, variable) in self.module.global_variables.iter() {
            if variable.binding.is_some_and(|binding| binding.group != 0)
                || matches!(variable.space, naga::AddressSpace::Storage { access } if access != naga::StorageAccess::LOAD)
            {
                return Err(ShaderError::Glsl(
                    "OpenGL supports group-zero read-only resources".into(),
                ));
            }
            if variable.space == naga::AddressSpace::Handle {
                let supported = matches!(
                    self.module.types[variable.ty].inner,
                    naga::TypeInner::Image {
                        dim: naga::ImageDimension::D2,
                        arrayed: false,
                        class: naga::ImageClass::Sampled {
                            kind: naga::ScalarKind::Float,
                            multi: false
                        }
                    } | naga::TypeInner::Sampler { comparison: false }
                );
                if !supported {
                    return Err(ShaderError::Glsl(
                        "OpenGL supports non-comparison samplers and sampled float 2D textures"
                            .into(),
                    ));
                }
            }
        }
        // The GL4.5 device uses ClipControl(UPPER_LEFT, ZERO_TO_ONE), keeping offscreen
        // row zero and fragment coordinates consistent with CPU uploads and WGSL.
        let options = glsl::Options {
            version: glsl::Version::Desktop(450),
            writer_flags: glsl::WriterFlags::empty(),
            ..Default::default()
        };
        let pipeline = glsl::PipelineOptions {
            shader_stage: crate::shader_stage_to_naga(stage),
            entry_point: entry_point.into(),
            multiview: None,
        };
        let mut source = String::new();
        let reflection = glsl::Writer::new(
            &mut source,
            &self.module,
            &self.info,
            &options,
            &pipeline,
            naga::proc::BoundsCheckPolicies::default(),
        )
        .and_then(|mut writer| writer.write())
        .map_err(|error| ShaderError::Glsl(error.to_string()))?;
        let mut buffers = Vec::new();
        for (handle, name) in reflection.uniforms {
            let variable = &self.module.global_variables[handle];
            let Some(binding) = variable.binding else {
                continue;
            };
            let kind = match variable.space {
                naga::AddressSpace::Uniform => ResourceBindingType::UniformBuffer,
                naga::AddressSpace::Storage { .. } => ResourceBindingType::StorageBuffer,
                _ => continue,
            };
            buffers.push(GlslBufferBinding {
                name: name.into(),
                binding: binding.binding,
                kind,
            });
        }
        let mut textures = Vec::new();
        for (name, mapping) in reflection.texture_mapping {
            let texture = self.module.global_variables[mapping.texture]
                .binding
                .ok_or_else(|| ShaderError::Glsl("unbound texture".into()))?;
            let sampler = match mapping.sampler {
                Some(handle) => Some(
                    self.module.global_variables[handle]
                        .binding
                        .ok_or_else(|| ShaderError::Glsl("unbound sampler".into()))?
                        .binding,
                ),
                None => None,
            };
            textures.push(GlslTextureBinding {
                name: name.into(),
                texture: texture.binding,
                sampler,
            });
        }
        buffers.sort_by(|left, right| left.name.cmp(&right.name));
        textures.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(ShaderBinary {
            stage,
            entry_point: entry_point.into(),
            code: ShaderCode::Glsl(GlslShader {
                source: source.into(),
                buffers,
                textures,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx_core::{ResourceBindingType, ShaderCode};

    #[test]
    fn reflection_retains_sparse_logical_slots_and_first_instance() {
        let module = WgslModule::parse("@group(0) @binding(31) var<storage,read> values:array<vec4<f32>>; @vertex fn vs(@builtin(instance_index) i:u32)->@builtin(position) vec4<f32>{return values[i];}").unwrap();
        let ShaderCode::Glsl(shader) = module.compile_glsl(ShaderStage::Vertex, "vs").unwrap().code
        else {
            panic!("GLSL");
        };
        assert!(shader.source.starts_with("#version 450 core"));
        assert!(shader.source.contains("naga_vs_first_instance"));
        assert_eq!(shader.buffers.len(), 1);
        assert_eq!(shader.buffers[0].binding, 31);
        assert_eq!(shader.buffers[0].kind, ResourceBindingType::StorageBuffer);
    }

    #[test]
    fn unsupported_resource_groups_fail_before_native_compilation() {
        let module = WgslModule::parse("@group(1) @binding(0) var<uniform> color:vec4<f32>; @fragment fn fs()->@location(0) vec4<f32>{return color;}").unwrap();
        assert!(matches!(
            module.compile_glsl(ShaderStage::Fragment, "fs"),
            Err(ShaderError::Glsl(_))
        ));
    }

    #[test]
    fn sampled_texture_reflection_preserves_texture_and_sampler_bindings() {
        let module = WgslModule::parse("@group(0) @binding(19) var image:texture_2d<f32>; @group(0) @binding(23) var image_sampler:sampler; @fragment fn fs()->@location(0) vec4<f32>{return textureSample(image,image_sampler,vec2<f32>(0.5));}").unwrap();
        let ShaderCode::Glsl(shader) = module
            .compile_glsl(ShaderStage::Fragment, "fs")
            .unwrap()
            .code
        else {
            panic!("GLSL");
        };
        assert_eq!(shader.textures.len(), 1);
        assert_eq!(shader.textures[0].texture, 19);
        assert_eq!(shader.textures[0].sampler, Some(23));
    }
}
