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
