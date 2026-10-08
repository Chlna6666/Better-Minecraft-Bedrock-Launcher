use crate::device::{Dx11Device, backend, required};
use gfx_core::*;
use windows::Win32::Graphics::Direct3D11::*;

pub(crate) enum Shader {
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
        if desc.resource_set_layouts.len() > 1 {
            return Err(Error::Unavailable(
                "D3D11 currently supports resource group zero".into(),
            ));
        }
        for layout in &desc.resource_set_layouts {
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
        Ok(self.passes.insert(desc.clone()))
    }
    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDescriptor,
        _extent: Extent2d,
    ) -> Result<RenderPipelineId> {
        desc.validate()?;
        self.passes.get(desc.render_pass)?;
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
        let (src, dst) = match desc.blend_mode {
            BlendMode::Replace => (D3D11_BLEND_ONE, D3D11_BLEND_ZERO),
            BlendMode::Alpha => (D3D11_BLEND_SRC_ALPHA, D3D11_BLEND_INV_SRC_ALPHA),
            BlendMode::PremultipliedAlpha => (D3D11_BLEND_ONE, D3D11_BLEND_INV_SRC_ALPHA),
            BlendMode::SubpixelDualSource => (D3D11_BLEND_SRC1_COLOR, D3D11_BLEND_INV_SRC1_COLOR),
            BlendMode::AdditiveAlpha => (D3D11_BLEND_ONE, D3D11_BLEND_INV_SRC_ALPHA),
        };
        let mut blend_desc = D3D11_BLEND_DESC::default();
        blend_desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
            BlendEnable: (desc.blend_mode != BlendMode::Replace).into(),
            SrcBlend: src,
            DestBlend: dst,
            BlendOp: D3D11_BLEND_OP_ADD,
            SrcBlendAlpha: D3D11_BLEND_ONE,
            DestBlendAlpha: match desc.blend_mode {
                BlendMode::Replace => D3D11_BLEND_ZERO,
                BlendMode::AdditiveAlpha => D3D11_BLEND_ONE,
                _ => D3D11_BLEND_INV_SRC_ALPHA,
            },
            BlendOpAlpha: D3D11_BLEND_OP_ADD,
            RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
        };
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
                        CullMode: D3D11_CULL_NONE,
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
                        ..Default::default()
                    },
                    Some(&mut depth),
                )
                .map_err(backend)?;
        }
        Ok((required(blend)?, required(raster)?, required(depth)?))
    }
}

fn comparison(value: CompareFunction) -> D3D11_COMPARISON_FUNC {
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
