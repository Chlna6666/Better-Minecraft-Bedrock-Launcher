use crate::{Result, ShaderError, WgslModule};
use gfx_core::{ShaderBinary, ShaderStage};

impl WgslModule {
    /// Translates one WGSL entry point to desktop GLSL 4.50 and resource reflection.
    ///
    /// Group-zero buffers and sampled textures are supported. Writable storage is allowed
    /// only for compute entry points; graphics buffers remain read-only. Reflection
    /// preserves logical resource slots while the runtime assigns compact native slots.
    /// GL4.5 clip control retains WGSL coordinates; instance offsets use Naga's
    /// dedicated first-instance uniform, without requiring a GL 4.6 extension.
    /// # Errors
    /// Returns a GLSL error for unsupported resources, features or translation failures.
    pub fn compile_glsl(&self, stage: ShaderStage, entry_point: &str) -> Result<ShaderBinary> {
        use gfx_core::{
            BindingType, GlslBufferBinding, GlslShader, GlslTextureBinding, ShaderCode,
        };
        use naga::back::glsl;
        for (_, variable) in self.module.global_variables.iter() {
            if variable.binding.is_some_and(|binding| binding.group != 0)
                || (stage != ShaderStage::Compute
                    && matches!(variable.space, naga::AddressSpace::Storage { access } if access != naga::StorageAccess::LOAD))
            {
                return Err(ShaderError::Glsl(
                    "OpenGL graphics supports group-zero read-only resources".into(),
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
                            // Multisampled textures map to `sampler2DMS` and are read with
                            // `texelFetch`; GLSL 4.50 has both.
                            multi: _
                        }
                    }
                    // Storage images map to `layout(...) image2D` image units.
                    | naga::TypeInner::Image {
                        dim: naga::ImageDimension::D2,
                        arrayed: false,
                        class: naga::ImageClass::Storage { .. }
                    }
                    | naga::TypeInner::Sampler { comparison: false }
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
                naga::AddressSpace::Uniform => BindingType::UniformBuffer,
                naga::AddressSpace::Storage { .. } => BindingType::StorageBuffer,
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
            // Naga also reports storage images in `texture_mapping`; they are image units,
            // reflected separately below, and must not become sampler slots.
            let handle = &self.module.global_variables[mapping.texture];
            if matches!(
                self.module.types[handle.ty].inner,
                naga::TypeInner::Image {
                    class: naga::ImageClass::Storage { .. },
                    ..
                }
            ) {
                continue;
            }
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
        // Naga exposes image uniform names but no handle mapping, so the WGSL slot is
        // recovered from the generated name and cross-checked against the reflection list.
        let stage_suffix = match stage {
            ShaderStage::Vertex => "vs",
            ShaderStage::Fragment => "fs",
            ShaderStage::Compute => "cs",
        };
        let mut images = Vec::new();
        for (_, variable) in self.module.global_variables.iter() {
            let naga::TypeInner::Image {
                class: naga::ImageClass::Storage { format, access },
                ..
            } = self.module.types[variable.ty].inner
            else {
                continue;
            };
            let Some(binding) = variable.binding else {
                continue;
            };
            let name = format!(
                "_group_{}_binding_{}_{stage_suffix}",
                binding.group, binding.binding
            );
            // Naga exposes no image reflection list, so the generated source is the check.
            // An entry point only emits the images it references, so an absent name means
            // this entry point does not use it (a naming change surfaces as a missing
            // binding when the program is bound).
            if !source.contains(&name) {
                continue;
            }
            images.push(gfx_core::GlslImageBinding {
                name: name.into(),
                binding: binding.binding,
                format: storage_format_to_format(format)?,
                read_only: access == naga::StorageAccess::LOAD,
            });
        }
        buffers.sort_by(|left, right| left.name.cmp(&right.name));
        textures.sort_by(|left, right| left.name.cmp(&right.name));
        images.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(ShaderBinary {
            stage,
            entry_point: entry_point.into(),
            code: ShaderCode::Glsl(GlslShader {
                source: source.into(),
                buffers,
                textures,
                images,
            }),
        })
    }
}

/// Maps one WGSL storage-image format onto a core [`gfx_core::Format`].
fn storage_format_to_format(format: naga::StorageFormat) -> Result<gfx_core::Format> {
    use naga::StorageFormat as S;
    Ok(match format {
        S::R8Unorm => gfx_core::Format::R8Unorm,
        S::Rg8Unorm => gfx_core::Format::Rg8Unorm,
        S::R16Float => gfx_core::Format::R16Float,
        S::Rg16Float => gfx_core::Format::Rg16Float,
        S::R32Float => gfx_core::Format::R32Float,
        S::Rgba8Unorm => gfx_core::Format::Rgba8Unorm,
        S::Rgba16Float => gfx_core::Format::Rgba16Float,
        S::Rgba32Float => gfx_core::Format::Rgba32Float,
        S::R8Uint => gfx_core::Format::R8Uint,
        S::Rg8Uint => gfx_core::Format::Rg8Uint,
        S::Rgba8Uint => gfx_core::Format::Rgba8Uint,
        other => {
            return Err(ShaderError::Glsl(format!(
                "storage image format {other:?} has no portable gfx_core format"
            )));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx_core::{BindingType, ShaderCode};

    #[test]
    fn storage_image_reflection_records_slot_format_and_access() {
        let module = WgslModule::parse("@group(0) @binding(3) var dst: texture_storage_2d<rgba8unorm, write>; @group(0) @binding(4) var src: texture_storage_2d<r32float, read>; @compute @workgroup_size(1) fn cs(@builtin(global_invocation_id) id: vec3<u32>) { textureStore(dst, vec2<i32>(id.xy), textureLoad(src, vec2<i32>(id.xy))); }").unwrap();
        let ShaderCode::Glsl(shader) = module
            .compile_glsl(ShaderStage::Compute, "cs")
            .unwrap()
            .code
        else {
            panic!("GLSL");
        };
        assert_eq!(shader.images.len(), 2);
        assert_eq!(shader.images[0].binding, 3);
        assert_eq!(shader.images[0].format, gfx_core::Format::Rgba8Unorm);
        assert!(!shader.images[0].read_only);
        assert_eq!(shader.images[1].binding, 4);
        assert_eq!(shader.images[1].format, gfx_core::Format::R32Float);
        assert!(shader.images[1].read_only);
        assert!(shader.source.contains(&shader.images[0].name.to_string()));
    }
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
        assert_eq!(shader.buffers[0].kind, BindingType::StorageBuffer);
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
