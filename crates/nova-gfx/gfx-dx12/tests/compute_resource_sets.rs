#![cfg(all(target_os = "windows", feature = "shader-compiler"))]

use gfx_core::{conformance, DeviceDescriptor, Error, ResourceDevice, ShaderStage};
use gfx_dx12::Dx12Device;

#[test]
#[ignore = "requires D3D12 hardware; run with --features shader-compiler -- --ignored"]
fn compute_uav_to_srv_to_storage_image_roundtrip() {
    let mut device = match Dx12Device::new(&DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(reason)) => {
            eprintln!("NOVA_GFX_SKIP=dx12 unavailable: {reason}");
            return;
        }
        Err(error) => panic!("D3D12 initialization failed: {error}"),
    };
    let writer = gfx_shader::WgslModule::parse(
        conformance::COMPUTE_RESOURCE_SET_WRITER_WGSL,
    )
    .expect("writer WGSL")
    .compile_hlsl(ShaderStage::Compute, "write_data")
    .expect("writer HLSL");
    let reader = gfx_shader::WgslModule::parse(
        conformance::COMPUTE_RESOURCE_SET_READER_WGSL,
    )
    .expect("reader WGSL")
    .compile_hlsl(ShaderStage::Compute, "write_image")
    .expect("reader HLSL");
    assert!(device.compute_bind_group_capabilities().storage_textures);
    conformance::compute_resource_set_roundtrip(&mut device, writer, reader)
        .expect("native D3D12 UAV/SRV image visibility and readback");
}


#[test]
#[ignore = "requires D3D12 device with independent DIRECT queue; run with --features shader-compiler -- --ignored"]
fn native_independent_direct_queue_compute_and_gpu_wait() {
    use gfx_core::{
        BufferBinding, BufferDescriptor, BufferUsage, ComputeBufferAccess,
        ComputeBufferBinding, ComputeDispatch, GpuQueueId, GpuWorkStage,
        MemoryLocation, PipelineDevice, ShaderModuleDescriptor, SubmissionDevice,
    };
    let mut device = match Dx12Device::new(&DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(_)) => return,
        Err(error) => panic!("DX12 device initialization failed: {error}"),
    };
    let Some(queue) = device.independent_queue(GpuWorkStage::Compute) else {
        eprintln!("DX12 extra DIRECT queue unavailable; skipping");
        return;
    };
    // Prefer a real typed COMPUTE queue (index 2), but drivers rejecting
    // dedicated Compute still support independently submitted Compute on a
    // second DIRECT queue (index 1). Both must execute real shader commands.
    assert!(queue == (GpuQueueId { family: 0, index: 2 })
        || queue == (GpuQueueId { family: 0, index: 1 }));
    let shader = gfx_shader::WgslModule::parse(
        conformance::COMPUTE_RESOURCE_SET_WRITER_WGSL,
    )
    .expect("writer WGSL")
    .compile_hlsl(ShaderStage::Compute, "write_data")
    .expect("writer HLSL");
    let shader = device.create_shader_module(&ShaderModuleDescriptor {
        label: Some("native DIRECT queue Compute".into()),
        binary: shader,
    }).expect("compute shader");
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("native Compute output".into()),
        size: 16,
        usage: BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::GpuOnly,
    }).expect("storage");
    let bindings = [ComputeBufferBinding {
        binding: 0,
        buffer: BufferBinding { buffer, offset: 0, size: 16, stride: None },
        access: ComputeBufferAccess::ReadWrite,
    }];
    let jobs = [ComputeDispatch {
        shader, bindings: &bindings, workgroups: [1, 1, 1],
    }];
    let compute = device.submit_compute(gfx_core::ComputeSubmit {
        queue,
        dispatches: &jobs,
        timestamp_tag: None,
    })
        .expect("actual independent DIRECT compute dispatch");
    let proof = device.enqueue_gpu_queue_wait(GpuQueueId::PRIMARY, compute)
        .expect("GPU-side DIRECT queue join");
    assert_eq!(proof.producer(), compute);
    let joined = device.checkpoint_gpu_queue(GpuQueueId::PRIMARY)
        .expect("primary queue completion checkpoint");
    // Test-only host waits validate actual GPU fence signaling; the runtime
    // submission and dependency mechanism is asynchronous.
    device.wait_submission(joined).expect("primary GPU fence");
    device.wait_submission(compute).expect("independent producer fence");
    device.destroy_buffer(buffer).expect("destroy output");
    device.destroy_shader_module(shader).expect("destroy shader");
}

#[test]
#[ignore = "requires a physical DX12 dedicated COMPUTE queue; --features shader-compiler -- --ignored"]
fn native_typed_compute_accepts_upload_heap_cbv_without_graphics_only_state() {
    use gfx_core::{
        BufferBinding, BufferDescriptor, BufferUsage, ComputeBufferAccess,
        ComputeBufferBinding, ComputeDispatch, GpuQueueId, MemoryLocation,
        PipelineDevice, ShaderModuleDescriptor, SubmissionDevice,
    };
    let mut device = match Dx12Device::new(&DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(_)) => return,
        Err(error) => panic!("DX12 init failed: {error}"),
    };
    let dedicated = GpuQueueId { family: 0, index: 2 };
    if device.independent_queue(gfx_core::GpuWorkStage::Compute) != Some(dedicated) {
        eprintln!("DX12 dedicated COMPUTE not available");
        return;
    }
    let module = gfx_shader::WgslModule::parse(
        "struct Params { value: u32, pad0: u32, pad1: u32, pad2: u32, }
         @group(0) @binding(0) var<uniform> params: Params;
         @group(0) @binding(1) var<storage, read_write> output: array<u32>;
         @compute @workgroup_size(1)
         fn cs() { output[0] = params.value; }",
    ).expect("CBV shader");
    let shader = device.create_shader_module(&ShaderModuleDescriptor {
        label: Some("typed COMPUTE upload heap CBV".into()),
        binary: module.compile_hlsl(ShaderStage::Compute, "cs").expect("HLSL"),
    }).expect("shader");
    let constants = device.create_buffer(&BufferDescriptor {
        label: Some("upload-heap constant data".into()),
        size: 256,
        usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    }).expect("CPU to GPU CBV");
    let output = device.create_buffer(&BufferDescriptor {
        label: Some("GPU-only compute result".into()),
        size: 16,
        usage: BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE,
        memory_location: MemoryLocation::GpuOnly,
    }).expect("storage output");
    device.write_buffer(constants, 0, &123u32.to_le_bytes()).expect("mapped constant write");
    let bindings = [
        ComputeBufferBinding {
            binding: 0,
            buffer: BufferBinding { buffer: constants, offset: 0, size: 16, stride: None },
            access: ComputeBufferAccess::Uniform,
        },
        ComputeBufferBinding {
            binding: 1,
            buffer: BufferBinding { buffer: output, offset: 0, size: 16, stride: None },
            access: ComputeBufferAccess::ReadWrite,
        },
    ];
    let jobs = [ComputeDispatch { shader, bindings: &bindings, workgroups: [1, 1, 1] }];
    let tag = gfx_core::GpuTimestampTag {
        batch_id: 0xD123,
        stage: gfx_core::GpuTimestampStage::GeneratedMesh,
    };
    let producer = device.submit_compute(gfx_core::ComputeSubmit {
        queue: dedicated,
        dispatches: &jobs,
        timestamp_tag: Some(tag),
    })
        .expect("typed Compute with native GPU timestamp must keep upload CBV GENERIC_READ");
    let joined = device.checkpoint_gpu_queue(GpuQueueId::PRIMARY)
        .expect("primary join checkpoint");
    // This is an explicitly ignored HARDWARE test: CPU fence waits are for
    // correctness validation, never the normal SceneView streaming path.
    device.wait_submission(producer).expect("dedicated producer completion");
    device.wait_submission(joined).expect("graphics consumer join completion");
    let mut completed = Vec::new();
    ResourceDevice::drain_gpu_work_timestamps(&mut device, &mut completed)
        .expect("nonblocking completed timestamp drain");
    if let Some(sample) = completed.iter().find(|sample| sample.tag == tag) {
        assert!(sample.duration().is_some());
        assert!(sample.frequency_hz > 0);
    } else {
        eprintln!("NOVA_GFX_SKIP=compute-only queue has no valid timestamp clock/query");
    }
    device.destroy_buffer(output).expect("output retirement");
    device.destroy_buffer(constants).expect("CBV retirement");
    device.destroy_shader_module(shader).expect("shader retirement");
}
