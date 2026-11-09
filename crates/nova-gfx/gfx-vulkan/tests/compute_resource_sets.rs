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
