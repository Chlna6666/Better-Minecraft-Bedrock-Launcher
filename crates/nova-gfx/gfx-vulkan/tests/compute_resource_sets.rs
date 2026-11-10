use gfx_core::{conformance, DeviceDescriptor, Error, ResourceDevice, ShaderStage};
use gfx_vulkan::VulkanDevice;

#[test]
#[ignore = "requires native Vulkan compute-capable hardware and driver"]
fn compute_uav_to_srv_to_storage_image_roundtrip() {
    let mut device = match VulkanDevice::new(&DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(reason)) => {
            eprintln!("NOVA_GFX_SKIP=vulkan unavailable: {reason}");
            return;
        }
        Err(error) => panic!("Vulkan initialization failed: {error}"),
    };
    let writer = gfx_shader::WgslModule::parse(
        conformance::COMPUTE_RESOURCE_SET_WRITER_WGSL,
    )
    .expect("writer WGSL")
    .compile_spirv(ShaderStage::Compute, "write_data")
    .expect("writer SPIR-V");
    let reader = gfx_shader::WgslModule::parse(
        conformance::COMPUTE_RESOURCE_SET_READER_WGSL,
    )
    .expect("reader WGSL")
    .compile_spirv(ShaderStage::Compute, "write_image")
    .expect("reader SPIR-V");
    assert!(device.compute_bind_group_capabilities().storage_textures);
    conformance::compute_resource_set_roundtrip(&mut device, writer, reader)
        .expect("native Vulkan UAV/SRV image visibility and readback");
}


/// Exercise real buffer Compute on either the second graphics-family queue
/// or a dedicated COMPUTE-only queue family. The latter uses CONCURRENT
/// buffer sharing and queue-family-typed command pools, not invalid
/// graphics-family command buffers or EXCLUSIVE image ownership transfers.
/// Native semaphores order the producer before the graphics checkpoint.
#[test]
#[ignore = "requires native Vulkan hardware with a same-family secondary or compute-only queue"]
fn native_compute_submission_and_reverse_gpu_queue_wait() {
    use gfx_core::{
        BufferBinding, BufferDescriptor, BufferUsage, ComputeBufferAccess,
        ComputeBufferBinding, ComputeDispatch, GpuQueueId, GpuWorkStage,
        MemoryLocation, PipelineDevice, ShaderModuleDescriptor, SubmissionDevice,
    };
    let mut device = match VulkanDevice::new(&DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(_)) => return,
        Err(error) => panic!("Vulkan device initialization: {error}"),
    };
    let Some(secondary) = device.independent_queue(GpuWorkStage::Compute) else {
        eprintln!("Vulkan device has no independent buffer-compute queue; skipping");
        return;
    };
    let shader = gfx_shader::WgslModule::parse(
        conformance::COMPUTE_RESOURCE_SET_WRITER_WGSL,
    )
    .expect("GPU writer shader")
    .compile_spirv(ShaderStage::Compute, "write_data")
    .expect("SPIR-V writer");
    let shader = device.create_shader_module(&ShaderModuleDescriptor {
        label: Some("independent native compute producer".into()),
        binary: shader,
    }).expect("shader module");
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("native queue compute storage".into()),
        size: 16,
        usage: BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::GpuOnly,
    }).expect("native storage buffer");
    let bindings = [ComputeBufferBinding {
        binding: 0,
        buffer: BufferBinding { buffer, offset: 0, size: 16, stride: None },
        access: ComputeBufferAccess::ReadWrite,
    }];
    let passes = [ComputeDispatch {
        shader, bindings: &bindings, workgroups: [1, 1, 1],
    }];
    let tag = gfx_core::GpuTimestampTag {
        batch_id: 0xB123,
        stage: gfx_core::GpuTimestampStage::GeneratedMesh,
    };
    let producer = device.submit_compute_batch_profiled_on_queue(secondary, &passes, tag)
        .expect("actual secondary or dedicated Queue Family shader execution");
    let proof = device.enqueue_gpu_queue_wait(GpuQueueId::PRIMARY, producer)
        .expect("native queue-1 -> primary GPU semaphore");
    assert_eq!(proof.producer(), producer);
    assert_eq!(proof.target_queue(), GpuQueueId::PRIMARY);
    let primary = device.checkpoint_gpu_queue(GpuQueueId::PRIMARY)
        .expect("primary completion checkpoint after native wait");
    // A test-only CPU wait proves the native synchronization without
    // contaminating runtime frame-submission hot paths.
    device.wait_submission(primary).expect("primary wait finished");
    device.wait_submission(producer).expect("native compute producer finished");
    let mut completed = Vec::new();
    ResourceDevice::drain_gpu_work_timestamps(&mut device, &mut completed)
        .expect("nonblocking completed timestamp readback");
    if let Some(sample) = completed.iter().find(|sample| sample.tag == tag) {
        assert!(sample.frequency_hz > 0);
        assert!(sample.duration().is_some());
    } else {
        // Some compute-only queue families expose timestampValidBits == 0:
        // missing GPU timing is correct, not an inferred zero-duration sample.
        eprintln!("NOVA_GFX_SKIP=compute family exposes no native GPU timestamp clock");
    }
    device.destroy_buffer(buffer).expect("destroy storage");
    device.destroy_shader_module(shader).expect("destroy shader");
}
