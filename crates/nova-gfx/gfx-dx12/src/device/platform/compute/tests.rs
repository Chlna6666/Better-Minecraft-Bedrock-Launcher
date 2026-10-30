use super::*;

#[test]
#[ignore = "requires D3D12 hardware; verifies bounded scratch reuse and explicit trimming"]
fn native_indirect_scratch_reuses_completed_buffers_and_preserves_recording_state() {
    let mut device = Dx12Device::new(&DeviceDescriptor::default()).unwrap();
    let scratch = device.compute.acquire_indirect(&device.device, 96).unwrap();
    let address = windows::core::Interface::as_raw(&scratch.buffer);
    assert_eq!(scratch.capacity, 128);
    assert_eq!(scratch.state, D3D12_RESOURCE_STATE_COPY_DEST);
    device.compute.recycle_indirect(scratch, false);
    let scratch = device.compute.acquire_indirect(&device.device, 64).unwrap();
    assert_eq!(windows::core::Interface::as_raw(&scratch.buffer), address);
    assert_eq!(scratch.state, D3D12_RESOURCE_STATE_COPY_DEST);
    device.compute.recycle_indirect(scratch, true);
    let scratch = device.compute.acquire_indirect(&device.device, 96).unwrap();
    assert_eq!(windows::core::Interface::as_raw(&scratch.buffer), address);
    assert_eq!(scratch.state, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT);
    device.compute.recycle_indirect(scratch, false);
    assert_eq!(device.compute.indirect_bytes(), 128);
    let accounting = device.resource_stats().allocated_bytes;
    device.trim_memory(MemoryTrimLevel::Aggressive).unwrap();
    assert_eq!(device.compute.indirect_bytes(), 0);
    assert_eq!(device.resource_stats().allocated_bytes + 128, accounting);
    let resources = (0..DX12_FRAME_COMMAND_POOL_CAPACITY + 2)
        .map(|_| device.compute.acquire_indirect(&device.device, 64).unwrap())
        .collect::<Vec<_>>();
    for resource in resources {
        device.compute.recycle_indirect(resource, false);
    }
    assert_eq!(
        device.compute.indirect_buffers.len(),
        DX12_FRAME_COMMAND_POOL_CAPACITY
    );
}
