//! Transactional creation of shared layouts and pass-specific pipelines.
use super::*;
impl RendererResources {
    pub(in crate::scene_view) fn new(
        device: &mut dyn ExtensionDevice,
        context: &RendererExtensionContext,
    ) -> gpui::Result<Self> {
        Self::new_pass(device, context, None, PassKind::Color)
    }
    pub(in crate::scene_view) fn new_pass(
        device: &mut dyn ExtensionDevice,
        context: &RendererExtensionContext,
        prototype: Option<&Self>,
        pass: PassKind,
    ) -> gpui::Result<Self> {
        let vertex_binary = embedded_shader(context, ShaderStage::Vertex, "vs_main")?;
        let compute = device.compute_capabilities();
        let voxel_entry = if compute.indirect_draw && !compute.max_workgroups.contains(&0) {
            "vs_voxel_gpu"
        } else {
            "vs_voxel"
        };
        let voxel_vertex_binary = embedded_shader(context, ShaderStage::Vertex, voxel_entry)?;
        let entries = pass.entries();
        let fragment_binary = embedded_shader(context, ShaderStage::Fragment, entries[0])?;
        let voxel_fragment_binary = embedded_shader(context, ShaderStage::Fragment, entries[1])?;
        let mut resources = Self {
            pass,
            owns_layout: prototype.is_none(),
            cull_shader: None,
            singleton_cull_shader: None,
            bind_group_layout: None,
            pipeline_layout: None,
            vertex_shader: None,
            voxel_vertex_shader: None,
            generated_vertex_shader: None,
            fragment_shader: None,
            voxel_fragment_shader: None,
            pipelines: [None; 18],
            fallback_texture: None,
            fallback_texture_view: None,
            samplers: [None; 6],
        };
        let result = (|| {
            resources.generated_vertex_shader =
                Some(device.create_shader_module(&ShaderModuleDescriptor {
                    label: None,
                    binary: embedded_shader(context, ShaderStage::Vertex, "vs_generated_voxel")?,
                })?);
            let compute = device.compute_capabilities();
            if compute.indirect_draw && !compute.max_workgroups.contains(&0) {
                resources.cull_shader =
                    Some(device.create_shader_module(&ShaderModuleDescriptor {
                        label: Some("gpui-3d GPU batch culling".into()),
                        binary: embedded_shader(context, ShaderStage::Compute, "cs_cull")?,
                    })?);
                // This is a performance-only alternative to cs_cull.
                // If a particular backend driver rejects its module, keep
                // the established compute path rather than disabling 3D.
                let singleton =
                    embedded_shader(context, ShaderStage::Compute, "cs_cull_singletons").and_then(
                        |binary| {
                            Ok(device.create_shader_module(&ShaderModuleDescriptor {
                                label: Some("gpui-3d GPU singleton batch culling".into()),
                                binary,
                            })?)
                        },
                    );
                match singleton {
                    Ok(shader) => resources.singleton_cull_shader = Some(shader),
                    Err(error) => {
                        eprintln!(
                            "gpui-3d singleton culling unavailable on {:?}: {error:#}; falling back to cs_cull",
                            context.backend_kind(),
                        );
                    }
                }
            }
            if let Some(prototype) = prototype {
                resources.bind_group_layout = prototype.bind_group_layout;
                resources.pipeline_layout = prototype.pipeline_layout;
            } else {
                resources.bind_group_layout =
                    Some(device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                        label: Some("gpui-3d scene-view resource-set layout".into()),
                        entries: vec![
                            BindGroupLayoutEntry {
                                binding: 15,
                                binding_type: BindingType::StorageBuffer,
                                stages: ShaderStages::FRAGMENT,
                            },
                            BindGroupLayoutEntry {
                                binding: 16,
                                binding_type: BindingType::StorageBuffer,
                                stages: ShaderStages::VERTEX,
                            },
                            BindGroupLayoutEntry {
                                binding: 14,
                                binding_type: BindingType::StorageBuffer,
                                stages: ShaderStages::VERTEX,
                            },
                            BindGroupLayoutEntry {
                                binding: 13,
                                binding_type: BindingType::StorageBuffer,
                                stages: ShaderStages::VERTEX,
                            },
                            BindGroupLayoutEntry {
                                binding: 12,
                                binding_type: BindingType::StorageBuffer,
                                stages: ShaderStages::VERTEX,
                            },
                            BindGroupLayoutEntry {
                                binding: VISIBLE_INSTANCE_BINDING,
                                binding_type: BindingType::StorageBuffer,
                                stages: ShaderStages::VERTEX,
                            },
                            BindGroupLayoutEntry {
                                binding: VERTEX_BINDING,
                                binding_type: BindingType::StorageBuffer,
                                stages: ShaderStages::VERTEX,
                            },
                            BindGroupLayoutEntry {
                                binding: DRAW_BINDING,
                                binding_type: BindingType::StorageBuffer,
                                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                            },
                            BindGroupLayoutEntry {
                                binding: LIGHT_BINDING,
                                binding_type: BindingType::StorageBuffer,
                                stages: ShaderStages::FRAGMENT,
                            },
                            BindGroupLayoutEntry {
                                binding: ALBEDO_BINDING,
                                binding_type: BindingType::SampledTexture,
                                stages: ShaderStages::FRAGMENT,
                            },
                            BindGroupLayoutEntry {
                                binding: SAMPLER_BINDING,
                                binding_type: BindingType::Sampler,
                                stages: ShaderStages::FRAGMENT,
                            },
                            BindGroupLayoutEntry {
                                binding: NORMAL_MAP_BINDING,
                                binding_type: BindingType::SampledTexture,
                                stages: ShaderStages::FRAGMENT,
                            },
                            BindGroupLayoutEntry {
                                binding: OCCLUSION_BINDING,
                                binding_type: BindingType::SampledTexture,
                                stages: ShaderStages::FRAGMENT,
                            },
                            BindGroupLayoutEntry {
                                binding: METALLIC_ROUGHNESS_BINDING,
                                binding_type: BindingType::SampledTexture,
                                stages: ShaderStages::FRAGMENT,
                            },
                            BindGroupLayoutEntry {
                                binding: TANGENT_BINDING,
                                binding_type: BindingType::StorageBuffer,
                                stages: ShaderStages::VERTEX,
                            },
                            BindGroupLayoutEntry {
                                binding: INSTANCE_BINDING,
                                binding_type: BindingType::StorageBuffer,
                                stages: ShaderStages::VERTEX,
                            },
                            BindGroupLayoutEntry {
                                binding: FRAME_BINDING,
                                binding_type: BindingType::UniformBuffer,
                                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                            },
                        ],
                    })?);
                resources.pipeline_layout =
                    Some(device.create_pipeline_layout(&PipelineLayoutDescriptor {
                        label: Some("gpui-3d pipeline layout".into()),
                        bind_group_layouts: vec![resources.bind_group_layout()?],
                    })?);
            }
            resources.vertex_shader =
                Some(device.create_shader_module(&ShaderModuleDescriptor {
                    label: Some("gpui-3d vertex shader".into()),
                    binary: vertex_binary,
                })?);
            resources.voxel_vertex_shader =
                Some(device.create_shader_module(&ShaderModuleDescriptor {
                    label: Some("gpui-3d voxel vertex shader".into()),
                    binary: voxel_vertex_binary,
                })?);
            resources.fragment_shader =
                Some(device.create_shader_module(&ShaderModuleDescriptor {
                    label: Some("gpui-3d fragment shader".into()),
                    binary: fragment_binary,
                })?);
            resources.voxel_fragment_shader =
                Some(device.create_shader_module(&ShaderModuleDescriptor {
                    label: Some("gpui-3d voxel fragment shader".into()),
                    binary: voxel_fragment_binary,
                })?);
            let fallback_texture = device.create_texture(&TextureDescriptor {
                label: Some("gpui-3d fallback texture".into()),
                size: Extent2d::new(1, 1)?,
                mip_level_count: 1,
                format: Format::Rgba8UnormSrgb,
                usage: TextureUsage::SAMPLED | TextureUsage::COPY_DST,
                memory_location: MemoryLocation::GpuOnly,
                dimension: TextureDimension::D2,
                sample_count: 1,
            })?;
            resources.fallback_texture = Some(fallback_texture);
            resources.fallback_texture_view =
                Some(device.create_texture_view(&TextureViewDescriptor {
                    label: Some("gpui-3d fallback texture view".into()),
                    texture: fallback_texture,
                    base_mip_level: 0,
                    mip_level_count: 1,
                    format: Format::Rgba8UnormSrgb,
                })?);
            resources.replace_pipelines(device, context)
        })();
        if let Err(error) = result {
            if let Err(cleanup_error) = resources.destroy(device) {
                return Err(anyhow!(
                    "{error:#}; renderer cleanup also failed: {cleanup_error:#}"
                ));
            }
            return Err(error);
        }
        Ok(resources)
    }
}
