use super::*;
use gfx_core::BufferWriteDescriptor;

#[test]
fn native_batch_preserves_overlap_and_retires_one_submission() {
    let mut device = match VulkanDevice::new(&DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(reason)) => {
            eprintln!("NOVA_GFX_SKIP=vulkan unavailable: {reason}");
            return;
        }
        Err(error) => panic!("Vulkan initialization failed: {error}"),
    };
    let first = buffer(&mut device, MemoryLocation::GpuOnly);
    let second = buffer(&mut device, MemoryLocation::GpuOnly);
    let before = device.next_upload_fence_value;
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
    assert_eq!(device.next_upload_fence_value, before + 1);
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
    let before = device.next_upload_fence_value;
    assert!(ResourceDevice::write_buffer_batch(&mut device, [write(first, 15, &[9; 2])]).is_err());
    assert_eq!(device.next_upload_fence_value, before);
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

fn buffer(device: &mut VulkanDevice, memory_location: MemoryLocation) -> BufferId {
    device
        .create_buffer(&BufferDescriptor {
            label: Some("buffer batch fixture".into()),
            size: 16,
            usage: BufferUsage::COPY_SRC | BufferUsage::COPY_DST,
            memory_location,
        })
        .expect("fixture buffer")
}

fn read(device: &mut VulkanDevice, id: BufferId) -> Vec<u8> {
    let readback = buffer(device, MemoryLocation::GpuToCpu);
    let source = device.buffers.get(id).expect("source").buffer;
    let target = device.buffers.get(readback).expect("readback").buffer;
    device
        .copy_buffer_once(source, target, 0, 0, 16)
        .expect("readback copy");
    let bytes = device
        .buffers
        .get(readback)
        .expect("readback")
        .allocation
        .mapped_slice()
        .expect("mapped readback")[..16]
        .to_vec();
    device.destroy_buffer(readback).expect("release readback");
    bytes
}
