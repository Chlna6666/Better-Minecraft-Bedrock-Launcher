use super::*;
use gfx_core::BufferWriteDescriptor;

#[test]
#[ignore = "requires Vulkan hardware; verifies grouped retirement and invalid-batch atomicity"]
fn native_buffer_retirement_validates_before_invalidating_handles() {
    let mut device = VulkanDevice::new(&DeviceDescriptor::default()).unwrap();
    let first = buffer(&mut device, MemoryLocation::GpuOnly);
    let second = buffer(&mut device, MemoryLocation::GpuOnly);
    let before = device.deferred_destroys.len();
    assert!(ResourceDevice::destroy_buffer_batch(&mut device, &[first, first]).is_err());
    assert!(device.buffers.get(first).is_ok());
    assert_eq!(device.deferred_destroys.len(), before);
    ResourceDevice::destroy_buffer_batch(&mut device, &[first, second]).unwrap();
    assert!(device.buffers.get(first).is_err());
    assert!(device.buffers.get(second).is_err());
    assert!(device.deferred_destroys.len() <= before + 1);
    let next = buffer(&mut device, MemoryLocation::GpuOnly);
    assert!(ResourceDevice::destroy_buffer_batch(&mut device, &[next, first]).is_err());
    assert!(device.buffers.get(next).is_ok());
    ResourceDevice::destroy_buffer_batch(&mut device, &[next]).unwrap();
    ResourceDevice::destroy_buffer_batch(&mut device, &[]).unwrap();
    device.wait_for_pending_work().unwrap();
    assert!(device.deferred_destroys.is_empty());
}

#[test]
#[ignore = "requires Vulkan hardware; verifies command reuse and explicit memory trimming"]
fn native_buffer_upload_reuses_completed_command_pool() {
    let mut device = VulkanDevice::new(&DeviceDescriptor::default()).unwrap();
    let target = buffer(&mut device, MemoryLocation::GpuOnly);
    ResourceDevice::write_buffer_batch(&mut device, [write(target, 0, &[1; 16])]).unwrap();
    gfx_core::SubmissionDevice::wait_queue_idle(&mut device).unwrap();
    device.poll_cleanup();
    let pool = device
        .upload_command_pool
        .iter()
        .find(|commands| commands.timestamp_queries.is_none())
        .unwrap()
        .command_pool;
    for value in 2..8 {
        ResourceDevice::write_buffer_batch(&mut device, [write(target, 0, &[value; 16])]).unwrap();
        gfx_core::SubmissionDevice::wait_queue_idle(&mut device).unwrap();
        device.poll_cleanup();
        assert!(
            device
                .upload_command_pool
                .iter()
                .any(|commands| commands.command_pool == pool)
        );
    }
    assert_eq!(read(&mut device, target), [7; 16]);
    device
        .trim_memory(gfx_core::MemoryTrimLevel::Aggressive)
        .unwrap();
    assert_eq!(device.upload_ring.stats().reserved_bytes, 0);
    assert!(device.upload_command_pool.is_empty());
}

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
