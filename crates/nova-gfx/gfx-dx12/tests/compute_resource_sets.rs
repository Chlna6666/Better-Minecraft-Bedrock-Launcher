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
    // The dedicated COMPUTE-only queue is intentionally not advertised for
    // this DIRECT command-list API. Queue index 1 is an independent DIRECT
    // queue which supports real Compute commands and native GPU waits.
    assert_eq!(queue, GpuQueueId { family: 0, index: 1 });
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
    let compute = device.submit_compute_batch_on_queue(queue, &jobs)
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
