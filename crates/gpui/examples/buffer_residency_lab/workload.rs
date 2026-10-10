use super::{Options, native, report};
use anyhow::{Context as _, Result, ensure};
use gfx_core::*;
use serde_json::{Value, json};
use std::time::Instant;

const SIDE: u32 = 256;
const PIXELS: usize = SIDE as usize * SIDE as usize;

struct Target {
    buffer: BufferId,
    texture: TextureId,
    view: TextureViewId,
    pass: RenderPassId,
    steps: Vec<DrawStepDescriptor>,
}

pub(super) fn run<D: Device + TextureTransferDevice>(
    device: &mut D,
    name: &str,
    options: &Options,
) -> Result<Value> {
    let architecture = device.memory_architecture()?;
    let baseline = report::memory(device);
    let mut payload = vec![0; options.bytes];
    fill(&mut payload, 0, 0);
    let started = Instant::now();
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("residency static stream".into()),
        size: options.bytes as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: options.memory,
    })?;
    let create_us = started.elapsed().as_secs_f64() * 1e6;
    let started = Instant::now();
    let initial_upload = write(device, buffer, 0, &payload)?;
    let initial_upload_us = started.elapsed().as_secs_f64() * 1e6;
    let target = target(device, buffer, options.bytes, options.draws)?;
    draw(device, &target).context("initial shader draw/completion")?;
    verify(device, target.texture, &payload).context("initial checksum readback")?;
    let populated = report::memory(device);
    let (samples, updates, checks) = measure(device, &target, &mut payload, options)?;
    let final_memory = report::memory(device);
    Ok(json!({
        "schema": 1, "status": "passed", "backend": format!("{:?}", D::BACKEND_KIND),
        "adapter": name, "memory_architecture": format!("{architecture:?}"),
        "requested_adapter": options.adapter,
        "build": {"debug_assertions": cfg!(debug_assertions), "os": std::env::consts::OS, "arch": std::env::consts::ARCH},
        "requested_memory": format!("{:?}", options.memory), "path": report::path(D::BACKEND_KIND, options.memory),
        "timing_scope": "CPU wall clock; completion includes recording, driver submission and fence wait, not GPU timestamps",
        "workload": {"buffer_bytes": options.bytes, "samples": options.samples, "warmup": options.warmup,
            "draws_per_sample": options.draws, "update_every": options.update_every,
            "dirty_bytes_per_update": dirty_size(options.bytes), "target": [SIDE, SIDE],
            "shader_logical_bytes_per_draw": options.bytes.max(PIXELS * 4),
            "note": "logical loads include GPU cache hits; they do not establish physical memory bandwidth"},
        "create_us": create_us, "initial_upload_us": initial_upload_us,
        "initial_upload_backend_calls": initial_upload.calls,
        "baseline_memory": baseline, "populated_memory": populated, "final_memory": final_memory,
        "updates_including_warmup": updates, "pixel_checks": checks + 1,
        "summaries": report::summaries(&samples), "samples": samples,
        "production_policy_changed": false,
    }))
}

fn write<D: ResourceDevice>(
    device: &mut D,
    buffer: BufferId,
    offset: usize,
    bytes: &[u8],
) -> Result<BufferUploadStats> {
    Ok(device.write_buffer_batch([BufferWrite {
        descriptor: BufferWriteDescriptor {
            buffer,
            offset: offset as u64,
        },
        data: bytes,
    }])?)
}

fn dirty_size(bytes: usize) -> usize {
    (bytes / 16).max(65536)
}

fn measure<D: Device + TextureTransferDevice>(
    device: &mut D,
    target: &Target,
    payload: &mut [u8],
    options: &Options,
) -> Result<(Vec<Value>, usize, usize)> {
    let mut samples = Vec::with_capacity(options.samples);
    let mut updates = 0;
    let mut checks = 0;
    for index in 0..options.warmup + options.samples {
        let update = options.update_every != 0 && index % options.update_every == 0;
        let mut upload_us = 0.0;
        let mut upload_bytes = 0;
        if update {
            (upload_us, upload_bytes) =
                update_payload(device, target.buffer, payload, &mut updates)?;
        }
        let timings = draw(device, target).context("sample shader draw/completion")?;
        // Readback is outside measured draw/upload samples, including after every dirty write.
        if update || index + 1 == options.warmup + options.samples {
            verify(device, target.texture, payload).context("sample checksum readback")?;
            checks += 1;
        }
        if index >= options.warmup {
            samples.push(json!({"upload_us": upload_us, "upload_bytes": upload_bytes,
                "offscreen_call_us": timings[0], "completion_signal_us": timings[1], "completion_wait_us": timings[2],
                "completion_us": timings.iter().sum::<f64>(),
                "update_and_completion_us": upload_us + timings.iter().sum::<f64>(), "dirty": update}));
        }
    }
    Ok((samples, updates, checks))
}

fn update_payload<D: ResourceDevice>(
    device: &mut D,
    buffer: BufferId,
    payload: &mut [u8],
    updates: &mut usize,
) -> Result<(f64, u64)> {
    let size = dirty_size(payload.len());
    let offset = (*updates * size) % payload.len();
    *updates += 1;
    fill(
        &mut payload[offset..offset + size],
        offset / 4,
        *updates as u32,
    );
    let started = Instant::now();
    let bytes = write(device, buffer, offset, &payload[offset..offset + size])?.bytes;
    Ok((started.elapsed().as_secs_f64() * 1e6, bytes))
}

fn draw<D: Device + TextureTransferDevice>(device: &mut D, target: &Target) -> Result<[f64; 3]> {
    let started = Instant::now();
    device
        .render_step_list_to_texture_compat(
            target.view,
            target.pass,
            RenderStepList::Draw(&target.steps),
            LoadOp::Clear(ClearColor {
                red: 0.0,
                green: 0.0,
                blue: 0.0,
                alpha: 1.0,
            }),
            None,
        )
        .context("offscreen helper")?;
    let call = started.elapsed();
    let started = Instant::now();
    // These existing backend hooks drain all queued work, including offscreen draws.
    if matches!(D::BACKEND_KIND, BackendKind::Dx12 | BackendKind::Vulkan) {
        device.wait_texture_transfers().context("completion wait")?;
        return Ok([
            call.as_secs_f64() * 1e6,
            0.0,
            started.elapsed().as_secs_f64() * 1e6,
        ]);
    }
    // A fresh empty encoder places its fence after offscreen work on the same queue.
    let encoder = device
        .create_command_encoder(&CommandEncoderDescriptor { label: None })
        .context("completion encoder")?;
    let submission = device
        .submit_deferred(encoder)
        .context("completion fence submission")?;
    let submit = started.elapsed();
    let started = Instant::now();
    device
        .wait_submission(submission)
        .context("completion fence wait")?;
    let wait = started.elapsed();
    // DX12/Vulkan consume deferred encoders; DX11/OpenGL retain their live handles.
    if matches!(D::BACKEND_KIND, BackendKind::Dx11 | BackendKind::OpenGl) {
        device.destroy_command_encoder(encoder)?;
    }
    Ok([call, submit, wait].map(|time| time.as_secs_f64() * 1e6))
}

fn target<D: Device>(
    device: &mut D,
    buffer: BufferId,
    bytes: usize,
    draws: usize,
) -> Result<Target> {
    let size = Extent2d::new(SIDE, SIDE)?;
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("residency checksum".into()),
        size,
        mip_level_count: 1,
        format: Format::Rgba8Unorm,
        usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
    })?;
    let view = device.create_texture_view(&TextureViewDescriptor {
        label: None,
        texture,
        format: Format::Rgba8Unorm,
        base_mip_level: 0,
        mip_level_count: 1,
    })?;
    let (set, layout) = storage_binding(device, buffer, bytes)?;
    let pass = device.create_render_pass(&RenderPassDescriptor {
        label: None,
        color_attachment: ColorAttachmentDescriptor {
            format: Format::Rgba8Unorm,
        },
        depth_attachment: None,
    })?;
    let pipeline = pipeline(device, bytes, pass, layout)?;
    let steps = vec![
        DrawStepDescriptor {
            pipeline,
            resource_sets: resource_set_list([set]),
            vertex_count: 3,
            first_vertex: 0,
            instance_count: 1,
            first_instance: 0,
            scissor: None
        };
        draws
    ];
    Ok(Target {
        buffer,
        texture,
        view,
        pass,
        steps,
    })
}

fn storage_binding<D: Device>(
    device: &mut D,
    buffer: BufferId,
    bytes: usize,
) -> Result<(ResourceSetId, PipelineLayoutId)> {
    let layout = device.create_resource_set_layout(&ResourceSetLayoutDescriptor {
        label: None,
        entries: vec![ResourceSetLayoutEntry {
            binding: 0,
            binding_type: ResourceBindingType::StorageBuffer,
            stages: ShaderStages::FRAGMENT,
        }],
    })?;
    let set = device.create_resource_set(&ResourceSetDescriptor {
        label: None,
        layout,
        bindings: vec![ResourceBinding {
            binding: 0,
            resource: BindingResource::Buffer(BufferBinding {
                buffer,
                offset: 0,
                size: bytes as u64,
                // Scalar words do not require a structured binding stride.
                stride: None,
            }),
        }],
    })?;
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: None,
        resource_set_layouts: vec![layout],
    })?;
    Ok((set, pipeline_layout))
}

fn pipeline<D: Device>(
    device: &mut D,
    bytes: usize,
    pass: RenderPassId,
    pipeline_layout: PipelineLayoutId,
) -> Result<RenderPipelineId> {
    let source = shader_source(bytes);
    let vs = device.create_shader_module(&ShaderModuleDescriptor {
        label: None,
        binary: native::shader(&source, D::BACKEND_KIND, ShaderStage::Vertex, "vs")?,
    })?;
    let fs = device.create_shader_module(&ShaderModuleDescriptor {
        label: None,
        binary: native::shader(&source, D::BACKEND_KIND, ShaderStage::Fragment, "fs")?,
    })?;
    Ok(device.create_render_pipeline(
        &RenderPipelineDescriptor {
            label: None,
            vertex_shader: vs,
            vertex_entry_point: "vs".into(),
            fragment_shader: fs,
            fragment_entry_point: "fs".into(),
            vertex_buffers: vec![],
            render_pass: pass,
            pipeline_layout: Some(pipeline_layout),
            color_format: Format::Rgba8Unorm,
            blend_mode: BlendMode::Replace,
            primitive_topology: PrimitiveTopology::TriangleList,
            depth_state: None,
        },
        Extent2d::new(SIDE, SIDE)?,
    )?)
}

fn shader_source(bytes: usize) -> String {
    let words = bytes / 4;
    let reads = (words / PIXELS).max(1);
    format!(
        r"
@group(0) @binding(0) var<storage, read> values: array<u32>;
@vertex fn vs(@builtin(vertex_index) v: u32) -> @builtin(position) vec4<f32> {{
    let p = array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));
    return vec4<f32>(p[v],0.0,1.0);
}}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{
    let pixel = u32(p.y) * {SIDE}u + u32(p.x);
    var sum = 0u;
    for (var i = 0u; i < {reads}u; i = i + 1u) {{
        sum = sum + values[(pixel * {reads}u + i) % {words}u];
    }}
    return vec4<f32>(f32(sum & 255u), f32((sum >> 8u) & 255u), f32((sum >> 16u) & 255u), f32(sum >> 24u)) / 255.0;
}}"
    )
}

fn fill(payload: &mut [u8], start_word: usize, generation: u32) {
    for (index, word) in payload.chunks_exact_mut(4).enumerate() {
        let value = (start_word + index) as u32 ^ generation.wrapping_mul(0x9e3779b9);
        word.copy_from_slice(&value.to_le_bytes());
    }
}

fn verify<D: TextureTransferDevice>(
    device: &mut D,
    texture: TextureId,
    payload: &[u8],
) -> Result<()> {
    let readback = device.read_texture(texture)?;
    ensure!(
        readback.bytes_per_row >= SIDE * 4
            && readback.bytes.len() >= (readback.bytes_per_row * SIDE) as usize,
        "incomplete checksum texture readback"
    );
    let words = payload.len() / 4;
    let reads = (words / PIXELS).max(1);
    for pixel in 0..PIXELS {
        let sum = (0..reads).fold(0u32, |sum, index| {
            let offset = ((pixel * reads + index) % words) * 4;
            sum.wrapping_add(u32::from_le_bytes(
                payload[offset..offset + 4]
                    .try_into()
                    .expect("word range is four bytes"),
            ))
        });
        let expected = [
            (sum & 255) as u8,
            ((sum >> 8) & 255) as u8,
            ((sum >> 16) & 255) as u8,
            (sum >> 24) as u8,
        ];
        let offset =
            pixel / SIDE as usize * readback.bytes_per_row as usize + pixel % SIDE as usize * 4;
        ensure!(
            readback.bytes[offset..offset + 4] == expected,
            "buffer checksum mismatch at pixel {pixel}: {:?} != {expected:?}",
            &readback.bytes[offset..offset + 4]
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checksum_shader_compiles_for_all_windows_backends() {
        for backend in [
            BackendKind::Dx11,
            BackendKind::Dx12,
            BackendKind::Vulkan,
            BackendKind::OpenGl,
        ] {
            for bytes in [65536, 1048576, 8388608] {
                let source = shader_source(bytes);
                for (stage, entry) in [(ShaderStage::Vertex, "vs"), (ShaderStage::Fragment, "fs")] {
                    gfx_shader::compile_wgsl_for_backend(&source, backend, stage, entry)
                        .expect("checksum shader translation");
                }
            }
        }
    }
}
