use super::*;

pub(super) struct NovaResourceLayouts {
    pub(super) quad_resource_set_layout: ResourceSetLayoutId,
    pub(super) shadow_resource_set_layout: ResourceSetLayoutId,
    pub(super) path_rasterization_resource_set_layout: ResourceSetLayoutId,
    pub(super) path_resource_set_layout: ResourceSetLayoutId,
    pub(super) mono_resource_set_layout: ResourceSetLayoutId,
    pub(super) poly_resource_set_layout: ResourceSetLayoutId,
    pub(super) underline_resource_set_layout: ResourceSetLayoutId,
    pub(super) backdrop_blur_pass_resource_set_layout: ResourceSetLayoutId,
    pub(super) backdrop_blur_resource_set_layout: ResourceSetLayoutId,
    pub(super) custom_mesh_3d_resource_set_layout: ResourceSetLayoutId,
    pub(super) quad_pipeline_layout: PipelineLayoutId,
    pub(super) shadow_pipeline_layout: PipelineLayoutId,
    pub(super) path_rasterization_pipeline_layout: PipelineLayoutId,
    pub(super) path_pipeline_layout: PipelineLayoutId,
    pub(super) mono_pipeline_layout: PipelineLayoutId,
    pub(super) poly_pipeline_layout: PipelineLayoutId,
    pub(super) underline_pipeline_layout: PipelineLayoutId,
    pub(super) backdrop_blur_pass_pipeline_layout: PipelineLayoutId,
    pub(super) backdrop_blur_pipeline_layout: PipelineLayoutId,
    pub(super) custom_mesh_3d_pipeline_layout: PipelineLayoutId,
}

pub(super) fn create_resource_layouts<D>(device: &mut D, label: &str) -> Result<NovaResourceLayouts>
where
    D: BackendResources + BackendPipelines,
{
    let quad_resource_set_layout =
        device.create_resource_set_layout(&ResourceSetLayoutDescriptor {
            label: Some(format!("{label} quad layout")),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 1,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
            ],
        })?;
    let shadow_resource_set_layout =
        device.create_resource_set_layout(&ResourceSetLayoutDescriptor {
            label: Some(format!("{label} shadow layout")),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 2,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let path_rasterization_resource_set_layout =
        device.create_resource_set_layout(&ResourceSetLayoutDescriptor {
            label: Some(format!("{label} path rasterization layout")),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 3,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let path_resource_set_layout =
        device.create_resource_set_layout(&ResourceSetLayoutDescriptor {
            label: Some(format!("{label} path layout")),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 4,
                    binding_type: ResourceBindingType::SampledTexture,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 5,
                    binding_type: ResourceBindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 6,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let mono_resource_set_layout =
        device.create_resource_set_layout(&ResourceSetLayoutDescriptor {
            label: Some(format!("{label} mono sprite layout")),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 1,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 4,
                    binding_type: ResourceBindingType::SampledTexture,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 5,
                    binding_type: ResourceBindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 8,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let poly_resource_set_layout =
        device.create_resource_set_layout(&ResourceSetLayoutDescriptor {
            label: Some(format!("{label} poly sprite layout")),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 4,
                    binding_type: ResourceBindingType::SampledTexture,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 5,
                    binding_type: ResourceBindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 9,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let underline_resource_set_layout =
        device.create_resource_set_layout(&ResourceSetLayoutDescriptor {
            label: Some(format!("{label} underline layout")),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 7,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let backdrop_blur_pass_resource_set_layout =
        device.create_resource_set_layout(&ResourceSetLayoutDescriptor {
            label: Some(format!("{label} backdrop blur pass layout")),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 4,
                    binding_type: ResourceBindingType::SampledTexture,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 5,
                    binding_type: ResourceBindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 15,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let backdrop_blur_resource_set_layout =
        device.create_resource_set_layout(&ResourceSetLayoutDescriptor {
            label: Some(format!("{label} backdrop blur layout")),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 4,
                    binding_type: ResourceBindingType::SampledTexture,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 5,
                    binding_type: ResourceBindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 16,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let custom_mesh_3d_resource_set_layout =
        device.create_resource_set_layout(&ResourceSetLayoutDescriptor {
            label: Some(format!("{label} custom GPU mesh 3D layout")),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 20,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 21,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
            ],
        })?;
    let quad_pipeline_layout =
        device.create_pipeline_layout(&PipelineLayoutResourceDescriptor {
            label: Some(format!("{label} quad pipeline layout")),
            resource_set_layouts: vec![quad_resource_set_layout],
        })?;
    let shadow_pipeline_layout =
        device.create_pipeline_layout(&PipelineLayoutResourceDescriptor {
            label: Some(format!("{label} shadow pipeline layout")),
            resource_set_layouts: vec![shadow_resource_set_layout],
        })?;
    let path_rasterization_pipeline_layout =
        device.create_pipeline_layout(&PipelineLayoutResourceDescriptor {
            label: Some(format!("{label} path rasterization pipeline layout")),
            resource_set_layouts: vec![path_rasterization_resource_set_layout],
        })?;
    let path_pipeline_layout =
        device.create_pipeline_layout(&PipelineLayoutResourceDescriptor {
            label: Some(format!("{label} path pipeline layout")),
            resource_set_layouts: vec![path_resource_set_layout],
        })?;
    let mono_pipeline_layout =
        device.create_pipeline_layout(&PipelineLayoutResourceDescriptor {
            label: Some(format!("{label} mono sprite pipeline layout")),
            resource_set_layouts: vec![mono_resource_set_layout],
        })?;
    let poly_pipeline_layout =
        device.create_pipeline_layout(&PipelineLayoutResourceDescriptor {
            label: Some(format!("{label} poly sprite pipeline layout")),
            resource_set_layouts: vec![poly_resource_set_layout],
        })?;
    let underline_pipeline_layout =
        device.create_pipeline_layout(&PipelineLayoutResourceDescriptor {
            label: Some(format!("{label} underline pipeline layout")),
            resource_set_layouts: vec![underline_resource_set_layout],
        })?;
    let backdrop_blur_pass_pipeline_layout =
        device.create_pipeline_layout(&PipelineLayoutResourceDescriptor {
            label: Some(format!("{label} backdrop blur pass pipeline layout")),
            resource_set_layouts: vec![backdrop_blur_pass_resource_set_layout],
        })?;
    let backdrop_blur_pipeline_layout =
        device.create_pipeline_layout(&PipelineLayoutResourceDescriptor {
            label: Some(format!("{label} backdrop blur pipeline layout")),
            resource_set_layouts: vec![backdrop_blur_resource_set_layout],
        })?;
    let custom_mesh_3d_pipeline_layout =
        device.create_pipeline_layout(&PipelineLayoutResourceDescriptor {
            label: Some(format!("{label} custom GPU mesh 3D pipeline layout")),
            resource_set_layouts: vec![custom_mesh_3d_resource_set_layout],
        })?;

    Ok(NovaResourceLayouts {
        quad_resource_set_layout,
        shadow_resource_set_layout,
        path_rasterization_resource_set_layout,
        path_resource_set_layout,
        mono_resource_set_layout,
        poly_resource_set_layout,
        underline_resource_set_layout,
        backdrop_blur_pass_resource_set_layout,
        backdrop_blur_resource_set_layout,
        custom_mesh_3d_resource_set_layout,
        quad_pipeline_layout,
        shadow_pipeline_layout,
        path_rasterization_pipeline_layout,
        path_pipeline_layout,
        mono_pipeline_layout,
        poly_pipeline_layout,
        underline_pipeline_layout,
        backdrop_blur_pass_pipeline_layout,
        backdrop_blur_pipeline_layout,
        custom_mesh_3d_pipeline_layout,
    })
}
