use super::*;
use gfx_core::BufferWriteDescriptor;

#[test]
fn native_batch_preserves_overlap_and_signals_once() {
    let mut device = match Dx12Device::new(&gfx_core::DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(reason)) => {
            eprintln!("NOVA_GFX_SKIP=dx12 unavailable: {reason}");
            return;
        }
        Err(error) => panic!("DX12 initialization failed: {error}"),
    };
    let first = buffer(&mut device, MemoryLocation::GpuOnly);
    let second = buffer(&mut device, MemoryLocation::GpuOnly);
    let before = device.next_fence_value;
    let stats = ResourceDevice::write_buffer_batch(
        &mut device,
        [
            write(first, 0, &[1; 16]),
            write(second, 0, &[4; 16]),
            write(first, 4, &[2; 8]),
            write(first, 8, &[3; 4]),
        ],
    )
    .expect("native batch");
    assert_eq!(
        device.next_fence_value,
        before + 1,
        "one signal for the complete batch"
    );
    assert_eq!(
        stats,
        BufferUploadStats {
            calls: 4,
            bytes: 44
        }
    );
    assert_eq!(
        read(&mut device, first),
        [1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 1, 1, 1, 1]
    );
    assert_eq!(read(&mut device, second), [4; 16]);
    let before = device.next_fence_value;
    assert!(ResourceDevice::write_buffer_batch(&mut device, [write(first, 15, &[9; 2])]).is_err());
    assert_eq!(device.next_fence_value, before);
    assert_eq!(
        ResourceDevice::write_buffer_batch(&mut device, []).expect("empty batch"),
        BufferUploadStats::default()
    );
}

fn write(buffer: BufferId, offset: u64, data: &[u8]) -> BufferWrite<'_> {
    BufferWrite {
        descriptor: BufferWriteDescriptor { buffer, offset },
        data,
    }
}

fn buffer(device: &mut Dx12Device, memory_location: MemoryLocation) -> BufferId {
    device
        .create_buffer(&BufferDescriptor {
            label: Some("buffer batch fixture".into()),
            size: 16,
            usage: BufferUsage::COPY_SRC | BufferUsage::COPY_DST,
            memory_location,
        })
        .expect("fixture buffer")
}

fn read(device: &mut Dx12Device, id: BufferId) -> Vec<u8> {
    let readback = buffer(device, MemoryLocation::GpuToCpu);
    let source = device
        .buffers
        .get(id)
        .expect("source")
        .resource
        .clone()
        .expect("source native");
    let target = device
        .buffers
        .get(readback)
        .expect("readback")
        .resource
        .clone()
        .expect("readback native");
    device
        .copy_buffer_region(BufferRegionCopy {
            source: &source,
            source_state: D3D12_RESOURCE_STATE_GENERIC_READ,
            source_offset: 0,
            destination: &target,
            destination_state: D3D12_RESOURCE_STATE_COPY_DEST,
            destination_final_state: D3D12_RESOURCE_STATE_COPY_DEST,
            destination_offset: 0,
            len: 16,
        })
        .expect("readback copy");
    let mut pointer = ptr::null_mut();
    // SAFETY: The readback copy completed, and the range covers this sixteen-byte buffer.
    unsafe {
        target.Map(
            0,
            Some(&D3D12_RANGE { Begin: 0, End: 16 }),
            Some(&raw mut pointer),
        )
    }
    .expect("readback map");
    // SAFETY: The mapped allocation contains the sixteen copied bytes.
    let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), 16) }.to_vec();
    // SAFETY: Balance the successful Map without declaring CPU writes.
    unsafe { target.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 })) };
    device.destroy_buffer(readback).expect("release readback");
    bytes
}
