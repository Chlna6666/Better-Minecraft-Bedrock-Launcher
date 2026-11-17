use super::*;

#[derive(Clone, Copy)]
pub(super) struct Draw {
    pub(super) pipeline: RenderPipelineId,
    pub(super) bind_group: BindGroupId,
    pub(super) target: TextureViewId,
}

pub(super) fn render<D: PresentationDevice>(
    device: &mut D,
    resources: &RendererResources,
    draw: Draw,
    profile_id: Option<(u64, BufferId)>,
) {
    let commands: Vec<_> = (0..if profile_id.is_some() { 32 } else { 1 })
        .map(|_| {
            if let Some((_, arguments)) = profile_id {
                RenderCommand::DrawIndirect(DrawIndirectCommand {
                    pipeline: draw.pipeline,
                    bind_groups: bind_group_list([draw.bind_group]),
                    arguments,
                    offset: 0,
                    scissor: None,
                })
            } else {
                RenderCommand::Draw(DrawCommand {
                    pipeline: draw.pipeline,
                    bind_groups: bind_group_list([draw.bind_group]),
                    vertex_count: 4,
                    instance_count: 1,
                    first_vertex: 0,
                    first_instance: 0,
                    scissor: None,
                })
            }
        })
        .collect();
    let pass = RenderPassCommands {
        color_attachments: std::slice::from_ref(&draw.target),
        render_pass: resources.render_pass,
        commands: &commands,
        color_load_op: LoadOp::Clear(ClearColor {
            red: 0.,
            green: 0.,
            blue: 0.,
            alpha: 0.,
        }),
        clear_region: None,
        depth_attachment: Some(DepthAttachment {
            target: resources.depth_texture_view,
            depth_load_op: LoadOp::Clear(1.),
            stencil_load_op: None,
        }),
        viewport: None,
        resolve_target: None,
    };
    let tag = profile_id.map(|(batch_id, _)| GpuTimestampTag {
        batch_id,
        stage: GpuTimestampStage::DrawIndirect,
    });
    device
        .render_to_textures_on_queue(&[pass], &[tag], OffscreenQueuePreference::OrderedGraphics)
        .expect("shader comparison draw");
}

pub(super) fn profile<D: gfx_core::Device + TextureTransferDevice>(
    device: &mut D,
    resources: &[RendererResources; 2],
    targets: [(TextureId, TextureViewId); 2],
    draws: [Draw; 2],
    label: &str,
) {
    let arguments = device
        .create_buffer(&BufferDescriptor {
            label: Some("shader comparison indirect arguments".into()),
            size: 16,
            usage: BufferUsage::INDIRECT | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::GpuOnly,
        })
        .expect("shader comparison arguments");
    let bytes: Vec<_> = [4u32, 1, 0, 0]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    device
        .write_buffer(arguments, 0, &bytes)
        .expect("shader comparison argument upload");
    let mut samples = Vec::new();
    for round in 0..88 {
        for index in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
            render(
                device,
                &resources[index],
                draws[index],
                Some((round * 2 + index as u64, arguments)),
            );
            if D::BACKEND_KIND == BackendKind::Dx11 {
                // D3D11's completed event query owns the preceding timestamp scope.
                device
                    .checkpoint_ordered_queue(GpuWorkStage::Graphics)
                    .expect("shader comparison completion checkpoint");
            }
            // Explicit readback is confined to this ignored hardware test, never a render hot path.
            device
                .read_texture(targets[index].0)
                .expect("complete shader comparison pass");
            device
                .drain_gpu_work_timestamps(&mut samples)
                .expect("completed GPU shader timestamps");
        }
    }
    let mut times: [Vec<_>; 2] = std::array::from_fn(|_| Vec::new());
    let mut pairs = std::collections::BTreeMap::<_, [Option<i128>; 2]>::new();
    for sample in samples {
        if sample.tag.batch_id >= 16
            && let Some(duration) = sample.duration()
        {
            let index = (sample.tag.batch_id % 2) as usize;
            times[index].push(duration.as_nanos());
            pairs.entry(sample.tag.batch_id / 2).or_default()[index] =
                Some(duration.as_nanos() as i128);
        }
    }
    for times in &mut times {
        times.sort_unstable();
    }
    if times.iter().all(|times| !times.is_empty()) {
        let mut deltas: Vec<_> = pairs
            .values()
            .filter_map(|pair| Some(pair[1]? - pair[0]?))
            .collect();
        deltas.sort_unstable();
        if deltas.is_empty() {
            println!("{label} GPU timing has no completed pairs");
            return;
        }
        println!(
            "{label} GPU median_ns: original={} adapted={} samples={}/{} paired_delta_p10/median/p90={}/{}/{} (32 draws/pass)",
            times[0][times[0].len() / 2],
            times[1][times[1].len() / 2],
            times[0].len(),
            times[1].len(),
            deltas[deltas.len() / 10],
            deltas[deltas.len() / 2],
            deltas[deltas.len() * 9 / 10]
        );
    } else {
        println!("{label} GPU timing unavailable on {:?}", D::BACKEND_KIND);
    }
}

pub(super) fn native_binary(mut binary: ShaderBinary, backend: BackendKind) -> ShaderBinary {
    let ShaderCode::Hlsl(source) = &binary.code else {
        return binary;
    };
    use windows::Win32::Graphics::Direct3D::Fxc::{
        D3DCOMPILE_ENABLE_STRICTNESS, D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile,
    };
    use windows::core::PCSTR;
    let target = match (backend, binary.stage) {
        (BackendKind::Dx11, ShaderStage::Vertex) => b"vs_5_0\0",
        (BackendKind::Dx11, _) => b"ps_5_0\0",
        (_, ShaderStage::Vertex) => b"vs_5_1\0",
        _ => b"ps_5_1\0",
    };
    let entry = std::ffi::CString::new(binary.entry_point.as_str()).expect("shader entry");
    let mut bytecode = None;
    let mut diagnostics = None;
    // SAFETY: all pointers refer to live source/entry/target data for this synchronous test compile.
    unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(entry.as_ptr().cast()),
            PCSTR(target.as_ptr()),
            D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &raw mut bytecode,
            Some(&raw mut diagnostics),
        )
        .expect("native reference shader compile");
    }
    let bytecode = bytecode.expect("compiled reference bytecode");
    // SAFETY: the COM blob keeps its initialized immutable byte buffer live until the copy completes.
    let bytes = unsafe {
        std::slice::from_raw_parts(
            bytecode.GetBufferPointer().cast::<u8>(),
            bytecode.GetBufferSize(),
        )
    };
    binary.code = ShaderCode::DxBytecode(bytes.to_vec());
    binary
}
