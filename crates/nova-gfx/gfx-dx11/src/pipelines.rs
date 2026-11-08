use crate::device::{Dx11Device, backend, required};
use gfx_core::*;
use windows::Win32::Graphics::Direct3D11::*;

pub(crate) enum Shader {
    Compute(ID3D11ComputeShader),
    Vertex {
        native: ID3D11VertexShader,
        entry: String,
    },
    Fragment {
        native: ID3D11PixelShader,
        entry: String,
    },
}
pub(crate) struct Pipeline {
    pub(crate) vertex: ID3D11VertexShader,
    pub(crate) fragment: ID3D11PixelShader,
    pub(crate) blend: ID3D11BlendState,
    pub(crate) raster: ID3D11RasterizerState,
    pub(crate) depth: ID3D11DepthStencilState,
    pub(crate) desc: RenderPipelineDescriptor,
}

impl PipelineDevice for Dx11Device {
    fn create_pipeline_layout(
        &mut self,
        desc: &PipelineLayoutDescriptor,
    ) -> Result<PipelineLayoutId> {
        if desc.bind_group_layouts.len() > 1 {
            return Err(Error::Unavailable(
                "D3D11 currently supports resource group zero".into(),
            ));
        }
        for layout in &desc.bind_group_layouts {
            self.layouts.get(*layout)?;
        }
        Ok(self.pipeline_layouts.insert(desc.clone()))
    }
    fn create_shader_module(&mut self, desc: &ShaderModuleDescriptor) -> Result<ShaderModuleId> {
        desc.validate()?;
        let bytes = match &desc.binary.code {
            ShaderCode::DxBytecode(bytes) => bytes.as_slice(),
            ShaderCode::DxBytecodeStatic(bytes) => *bytes,
            _ => {
                return Err(Error::Shader(
                    "D3D11 requires build-time SM5.0 bytecode".into(),
                ));
            }
        };
        let entry = desc.binary.entry_point.clone();
        let shader = match desc.binary.stage {
            ShaderStage::Compute => {
                let mut native = None;
                // SAFETY: CreateComputeShader validates and copies the live bytecode slice.
                unsafe {
                    self.native
                        .CreateComputeShader(bytes, None, Some(&mut native))
                }
                .map_err(backend)?;
                Shader::Compute(required(native)?)
            }
            ShaderStage::Vertex => {
                let mut native = None;
                // SAFETY: CreateVertexShader validates and copies this live bytecode slice.
                unsafe {
                    self.native
                        .CreateVertexShader(bytes, None, Some(&mut native))
                }
                .map_err(backend)?;
                Shader::Vertex {
                    native: required(native)?,
                    entry,
                }
            }
            ShaderStage::Fragment => {
                let mut native = None;
                // SAFETY: CreatePixelShader validates and copies this live bytecode slice.
                unsafe {
                    self.native
                        .CreatePixelShader(bytes, None, Some(&mut native))
                }
                .map_err(backend)?;
                Shader::Fragment {
                    native: required(native)?,
                    entry,
                }
            }
        };
        Ok(self.shaders.insert(shader))
    }
    fn create_render_pass(&mut self, desc: &RenderPassDescriptor) -> Result<RenderPassId> {
        desc.validate()?;
        Ok(self.passes.insert(desc.clone()))
    }
    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDescriptor,
        _extent: Extent2d,
    ) -> Result<RenderPipelineId> {
        desc.validate()?;
        let render_pass = self.passes.get(desc.render_pass)?;
        if desc.color_targets.len() != render_pass.color_attachments.len()
            || desc
                .color_targets
                .iter()
                .zip(render_pass.color_attachments.iter())
                .any(|(target, attachment)| target.format != attachment.format)
        {
            return Err(Error::InvalidInput(
                "D3D11 pipeline color targets and render pass attachments differ".into(),
            ));
        }
        if let Some(layout) = desc.pipeline_layout {
            self.pipeline_layouts.get(layout)?;
        }
        if !desc.vertex_buffers.is_empty() {
            return Err(Error::Unavailable(
                "D3D11 uses shader-pulled vertex buffers".into(),
            ));
        }
        let Shader::Vertex {
            native: vertex,
            entry,
        } = self.shaders.get(desc.vertex_shader)?
        else {
            return Err(Error::Shader("expected vertex shader".into()));
        };
        if entry != &desc.vertex_entry_point {
            return Err(Error::Shader("vertex entry mismatch".into()));
        }
        let Shader::Fragment {
            native: fragment,
            entry,
        } = self.shaders.get(desc.fragment_shader)?
        else {
            return Err(Error::Shader("expected fragment shader".into()));
        };
        if entry != &desc.fragment_entry_point {
            return Err(Error::Shader("fragment entry mismatch".into()));
        }
        let (blend, raster, depth) = self.pipeline_states(desc)?;
        Ok(self.pipelines.insert(Pipeline {
            vertex: vertex.clone(),
            fragment: fragment.clone(),
            blend,
            raster,
            depth,
            desc: desc.clone(),
        }))
    }
    fn destroy_pipeline_layout(&mut self, id: PipelineLayoutId) -> Result<()> {
        self.pipeline_layouts.take(id)?;
        Ok(())
    }
    fn destroy_shader_module(&mut self, id: ShaderModuleId) -> Result<()> {
        self.shaders.take(id)?;
        Ok(())
    }
    fn destroy_render_pass(&mut self, id: RenderPassId) -> Result<()> {
        self.passes.take(id)?;
        Ok(())
    }
    fn destroy_render_pipeline(&mut self, id: RenderPipelineId) -> Result<()> {
        self.pipelines.take(id)?;
        Ok(())
    }
}

impl Dx11Device {
    fn pipeline_states(
        &self,
        desc: &RenderPipelineDescriptor,
    ) -> Result<(
        ID3D11BlendState,
        ID3D11RasterizerState,
        ID3D11DepthStencilState,
    )> {
        let mut blend_desc = D3D11_BLEND_DESC {
            IndependentBlendEnable: (desc.color_targets.len() > 1).into(),
            ..Default::default()
        };
        for (index, target) in desc.color_targets.iter().enumerate() {
            let (src, dst) = blend_factors(target.blend);
            blend_desc.RenderTarget[index] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: (target.blend != BlendMode::Replace).into(),
                SrcBlend: src,
                DestBlend: dst,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: match target.blend {
                    BlendMode::Replace => D3D11_BLEND_ZERO,
                    BlendMode::AdditiveAlpha => D3D11_BLEND_ONE,
                    _ => D3D11_BLEND_INV_SRC_ALPHA,
                },
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: color_write_mask(target.write_mask),
            };
        }
        let state = desc.depth_state;
        let mut blend = None;
        let mut raster = None;
        let mut depth = None;
        // SAFETY: all state descriptors use valid enum values and initialized defaults;
        // outputs are owned COM references, independent of descriptor lifetimes.
        unsafe {
            self.native
                .CreateBlendState(&blend_desc, Some(&mut blend))
                .map_err(backend)?;
            self.native
                .CreateRasterizerState(
                    &D3D11_RASTERIZER_DESC {
                        FillMode: D3D11_FILL_SOLID,
                        CullMode: match desc.raster.cull_mode {
                            None => D3D11_CULL_NONE,
                            Some(gfx_core::Face::Front) => D3D11_CULL_FRONT,
                            Some(gfx_core::Face::Back) => D3D11_CULL_BACK,
                        },
                        FrontCounterClockwise: (desc.raster.front_face == gfx_core::FrontFace::Ccw)
                            .into(),
                        DepthClipEnable: true.into(),
                        ScissorEnable: true.into(),
                        ..Default::default()
                    },
                    Some(&mut raster),
                )
                .map_err(backend)?;
            self.native
                .CreateDepthStencilState(
                    &D3D11_DEPTH_STENCIL_DESC {
                        DepthEnable: state.is_some().into(),
                        DepthWriteMask: if state.is_some_and(|state| state.write_enabled) {
                            D3D11_DEPTH_WRITE_MASK_ALL
                        } else {
                            D3D11_DEPTH_WRITE_MASK_ZERO
                        },
                        DepthFunc: comparison(
                            state.map_or(CompareFunction::Always, |state| state.compare),
                        ),
                        StencilEnable: state.is_some_and(|s| s.stencil.is_some()).into(),
                        StencilReadMask: state
                            .and_then(|s| s.stencil)
                            .map_or(0xff, |s| s.read_mask as u8),
                        StencilWriteMask: state
                            .and_then(|s| s.stencil)
                            .map_or(0xff, |s| s.write_mask as u8),
                        FrontFace: state
                            .and_then(|s| s.stencil)
                            .map(d3d11_stencil_face)
                            .unwrap_or_default(),
                        BackFace: state
                            .and_then(|s| s.stencil)
                            .map(d3d11_stencil_face)
                            .unwrap_or_default(),
                        ..Default::default()
                    },
                    Some(&mut depth),
                )
                .map_err(backend)?;
        }
        Ok((required(blend)?, required(raster)?, required(depth)?))
    }
}

fn blend_factors(blend: BlendMode) -> (D3D11_BLEND, D3D11_BLEND) {
    match blend {
        BlendMode::Replace => (D3D11_BLEND_ONE, D3D11_BLEND_ZERO),
        BlendMode::Alpha => (D3D11_BLEND_SRC_ALPHA, D3D11_BLEND_INV_SRC_ALPHA),
        BlendMode::PremultipliedAlpha => (D3D11_BLEND_ONE, D3D11_BLEND_INV_SRC_ALPHA),
        BlendMode::SubpixelDualSource => (D3D11_BLEND_SRC1_COLOR, D3D11_BLEND_INV_SRC1_COLOR),
        BlendMode::AdditiveAlpha => (D3D11_BLEND_ONE, D3D11_BLEND_INV_SRC_ALPHA),
    }
}

fn color_write_mask(write_mask: ColorWrites) -> u8 {
    let mut native = 0_u8;
    if write_mask.contains(ColorWrites::RED) {
        native |= D3D11_COLOR_WRITE_ENABLE_RED.0 as u8;
    }
    if write_mask.contains(ColorWrites::GREEN) {
        native |= D3D11_COLOR_WRITE_ENABLE_GREEN.0 as u8;
    }
    if write_mask.contains(ColorWrites::BLUE) {
        native |= D3D11_COLOR_WRITE_ENABLE_BLUE.0 as u8;
    }
    if write_mask.contains(ColorWrites::ALPHA) {
        native |= D3D11_COLOR_WRITE_ENABLE_ALPHA.0 as u8;
    }
    native
}

pub(crate) fn comparison(value: CompareFunction) -> D3D11_COMPARISON_FUNC {
    match value {
        CompareFunction::Never => D3D11_COMPARISON_NEVER,
        CompareFunction::Less => D3D11_COMPARISON_LESS,
        CompareFunction::Equal => D3D11_COMPARISON_EQUAL,
        CompareFunction::LessEqual => D3D11_COMPARISON_LESS_EQUAL,
        CompareFunction::Greater => D3D11_COMPARISON_GREATER,
        CompareFunction::NotEqual => D3D11_COMPARISON_NOT_EQUAL,
        CompareFunction::GreaterEqual => D3D11_COMPARISON_GREATER_EQUAL,
        CompareFunction::Always => D3D11_COMPARISON_ALWAYS,
    }
}

fn d3d11_stencil_op(op: StencilOp) -> D3D11_STENCIL_OP {
    match op {
        StencilOp::Keep => D3D11_STENCIL_OP_KEEP,
        StencilOp::Zero => D3D11_STENCIL_OP_ZERO,
        StencilOp::Replace => D3D11_STENCIL_OP_REPLACE,
        StencilOp::IncrementClamp => D3D11_STENCIL_OP_INCR_SAT,
        StencilOp::DecrementClamp => D3D11_STENCIL_OP_DECR_SAT,
        StencilOp::Invert => D3D11_STENCIL_OP_INVERT,
        StencilOp::IncrementWrap => D3D11_STENCIL_OP_INCR,
        StencilOp::DecrementWrap => D3D11_STENCIL_OP_DECR,
    }
}

fn d3d11_stencil_face(stencil: StencilState) -> D3D11_DEPTH_STENCILOP_DESC {
    D3D11_DEPTH_STENCILOP_DESC {
        StencilFailOp: d3d11_stencil_op(stencil.front.fail_op),
        StencilDepthFailOp: d3d11_stencil_op(stencil.front.depth_fail_op),
        StencilPassOp: d3d11_stencil_op(stencil.front.pass_op),
        StencilFunc: comparison(stencil.front.compare),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_write_mask_maps_channels_to_d3d11_flags() {
        assert_eq!(color_write_mask(ColorWrites::ALL), 15);
        assert_eq!(color_write_mask(ColorWrites::COLOR), 7);
        assert_eq!(color_write_mask(ColorWrites::RED), 1);
        assert_eq!(color_write_mask(ColorWrites::ALPHA), 8);
        assert_eq!(color_write_mask(ColorWrites::RED | ColorWrites::ALPHA), 9);
        assert_eq!(color_write_mask(ColorWrites::empty()), 0);
    }
}
