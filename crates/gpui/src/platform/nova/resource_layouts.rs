use super::*;

/// Resource and pipeline layouts created for one device.
///
/// Every field is a device-registry handle, so a value can be copied into each window's
/// renderer instead of re-creating the layout set per window.
#[derive(Clone, Copy)]
pub(super) struct ResourceLayouts {
    pub(super) quad_resource_set_layout: BindGroupLayoutId,
    pub(super) shadow_resource_set_layout: BindGroupLayoutId,
    pub(super) path_rasterization_resource_set_layout: BindGroupLayoutId,
    pub(super) path_resource_set_layout: BindGroupLayoutId,
    pub(super) mono_resource_set_layout: BindGroupLayoutId,
    pub(super) poly_resource_set_layout: BindGroupLayoutId,
    pub(super) underline_resource_set_layout: BindGroupLayoutId,
    pub(super) backdrop_blur_pass_resource_set_layout: BindGroupLayoutId,
    pub(super) backdrop_blur_resource_set_layout: BindGroupLayoutId,
    pub(super) quad_pipeline_layout: PipelineLayoutId,
    pub(super) shadow_pipeline_layout: PipelineLayoutId,
    pub(super) path_rasterization_pipeline_layout: PipelineLayoutId,
    pub(super) path_pipeline_layout: PipelineLayoutId,
    pub(super) mono_pipeline_layout: PipelineLayoutId,
    pub(super) poly_pipeline_layout: PipelineLayoutId,
    pub(super) underline_pipeline_layout: PipelineLayoutId,
    pub(super) backdrop_blur_pass_pipeline_layout: PipelineLayoutId,
    pub(super) backdrop_blur_pipeline_layout: PipelineLayoutId,
}

pub(super) fn create_resource_layouts<D>(device: &mut D, label: &str) -> Result<ResourceLayouts>
where
    D: ResourceDevice + PipelineDevice,
{
    let quad_resource_set_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some(format!("{label} quad layout")),
        entries: vec![
            BindGroupLayoutEntry {
                binding: 0,
                binding_type: BindingType::UniformBuffer,
                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            },
            BindGroupLayoutEntry {
                binding: 1,
                binding_type: BindingType::StorageBuffer,
                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            },
            BindGroupLayoutEntry {
                binding: 17,
                binding_type: BindingType::StorageBuffer,
                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            },
        ],
    })?;
    let shadow_resource_set_layout =
        device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some(format!("{label} shadow layout")),
            entries: vec![
                BindGroupLayoutEntry {
                    binding: 0,
                    binding_type: BindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                BindGroupLayoutEntry {
                    binding: 2,
                    binding_type: BindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
                BindGroupLayoutEntry {
                    binding: 17,
                    binding_type: BindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let path_rasterization_resource_set_layout =
        device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some(format!("{label} path rasterization layout")),
            entries: vec![
                BindGroupLayoutEntry {
                    binding: 0,
                    binding_type: BindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                BindGroupLayoutEntry {
                    binding: 3,
                    binding_type: BindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let path_resource_set_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some(format!("{label} path layout")),
        entries: vec![
            BindGroupLayoutEntry {
                binding: 0,
                binding_type: BindingType::UniformBuffer,
                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            },
            BindGroupLayoutEntry {
                binding: 4,
                binding_type: BindingType::SampledTexture,
                stages: ShaderStages::FRAGMENT,
            },
            BindGroupLayoutEntry {
                binding: 5,
                binding_type: BindingType::Sampler,
                stages: ShaderStages::FRAGMENT,
            },
            BindGroupLayoutEntry {
                binding: 6,
                binding_type: BindingType::StorageBuffer,
                stages: ShaderStages::VERTEX,
            },
        ],
    })?;
    let mono_resource_set_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some(format!("{label} mono sprite layout")),
        entries: vec![
            BindGroupLayoutEntry {
                binding: 0,
                binding_type: BindingType::UniformBuffer,
                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            },
            BindGroupLayoutEntry {
                binding: 1,
                binding_type: BindingType::UniformBuffer,
                stages: ShaderStages::FRAGMENT,
            },
            BindGroupLayoutEntry {
                binding: 4,
                binding_type: BindingType::SampledTexture,
                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            },
            BindGroupLayoutEntry {
                binding: 5,
                binding_type: BindingType::Sampler,
                stages: ShaderStages::FRAGMENT,
            },
            BindGroupLayoutEntry {
                binding: 8,
                binding_type: BindingType::StorageBuffer,
                stages: ShaderStages::VERTEX,
            },
            BindGroupLayoutEntry {
                binding: 17,
                binding_type: BindingType::StorageBuffer,
                stages: ShaderStages::VERTEX,
            },
        ],
    })?;
    let poly_resource_set_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some(format!("{label} poly sprite layout")),
        entries: vec![
            BindGroupLayoutEntry {
                binding: 0,
                binding_type: BindingType::UniformBuffer,
                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            },
            BindGroupLayoutEntry {
                binding: 4,
                binding_type: BindingType::SampledTexture,
                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            },
            BindGroupLayoutEntry {
                binding: 5,
                binding_type: BindingType::Sampler,
                stages: ShaderStages::FRAGMENT,
            },
            BindGroupLayoutEntry {
                binding: 9,
                binding_type: BindingType::StorageBuffer,
                stages: ShaderStages::VERTEX,
            },
            BindGroupLayoutEntry {
                binding: 17,
                binding_type: BindingType::StorageBuffer,
                stages: ShaderStages::VERTEX,
            },
        ],
    })?;
    let underline_resource_set_layout =
        device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some(format!("{label} underline layout")),
            entries: vec![
                BindGroupLayoutEntry {
                    binding: 0,
                    binding_type: BindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                BindGroupLayoutEntry {
                    binding: 7,
                    binding_type: BindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
                BindGroupLayoutEntry {
                    binding: 17,
                    binding_type: BindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let backdrop_blur_pass_resource_set_layout =
        device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some(format!("{label} backdrop blur pass layout")),
            entries: vec![
                BindGroupLayoutEntry {
                    binding: 4,
                    binding_type: BindingType::SampledTexture,
                    stages: ShaderStages::FRAGMENT,
                },
                BindGroupLayoutEntry {
                    binding: 5,
                    binding_type: BindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
                BindGroupLayoutEntry {
                    binding: 15,
                    binding_type: BindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                BindGroupLayoutEntry {
                    binding: 17,
                    binding_type: BindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let backdrop_blur_resource_set_layout =
        device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some(format!("{label} backdrop blur layout")),
            entries: vec![
                BindGroupLayoutEntry {
                    binding: 0,
                    binding_type: BindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                BindGroupLayoutEntry {
                    binding: 4,
                    binding_type: BindingType::SampledTexture,
                    stages: ShaderStages::FRAGMENT,
                },
                BindGroupLayoutEntry {
                    binding: 5,
                    binding_type: BindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
                BindGroupLayoutEntry {
                    binding: 16,
                    binding_type: BindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
                BindGroupLayoutEntry {
                    binding: 17,
                    binding_type: BindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let quad_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some(format!("{label} quad pipeline layout")),
        bind_group_layouts: vec![quad_resource_set_layout],
    })?;
    let shadow_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some(format!("{label} shadow pipeline layout")),
        bind_group_layouts: vec![shadow_resource_set_layout],
    })?;
    let path_rasterization_pipeline_layout =
        device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some(format!("{label} path rasterization pipeline layout")),
            bind_group_layouts: vec![path_rasterization_resource_set_layout],
        })?;
    let path_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some(format!("{label} path pipeline layout")),
        bind_group_layouts: vec![path_resource_set_layout],
    })?;
    let mono_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some(format!("{label} mono sprite pipeline layout")),
        bind_group_layouts: vec![mono_resource_set_layout],
    })?;
    let poly_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some(format!("{label} poly sprite pipeline layout")),
        bind_group_layouts: vec![poly_resource_set_layout],
    })?;
    let underline_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some(format!("{label} underline pipeline layout")),
        bind_group_layouts: vec![underline_resource_set_layout],
    })?;
    let backdrop_blur_pass_pipeline_layout =
        device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some(format!("{label} backdrop blur pass pipeline layout")),
            bind_group_layouts: vec![backdrop_blur_pass_resource_set_layout],
        })?;
    let backdrop_blur_pipeline_layout =
        device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some(format!("{label} backdrop blur pipeline layout")),
            bind_group_layouts: vec![backdrop_blur_resource_set_layout],
        })?;

    Ok(ResourceLayouts {
        quad_resource_set_layout,
        shadow_resource_set_layout,
        path_rasterization_resource_set_layout,
        path_resource_set_layout,
        mono_resource_set_layout,
        poly_resource_set_layout,
        underline_resource_set_layout,
        backdrop_blur_pass_resource_set_layout,
        backdrop_blur_resource_set_layout,
        quad_pipeline_layout,
        shadow_pipeline_layout,
        path_rasterization_pipeline_layout,
        path_pipeline_layout,
        mono_pipeline_layout,
        poly_pipeline_layout,
        underline_pipeline_layout,
        backdrop_blur_pass_pipeline_layout,
        backdrop_blur_pipeline_layout,
    })
}
