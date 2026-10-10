use super::*;

#[test]
#[ignore = "requires a native Vulkan device"]
fn compact_preserves_gpu_only_buffer_bytes_without_new_backing() {
    let mut device = VulkanDevice::new(&DeviceDescriptor::default()).expect("Vulkan device");
    let descriptor = BufferDescriptor {
        label: Some("heap preservation fixture".into()),
        size: 1024 * 1024,
        usage: BufferUsage::STORAGE,
        memory_location: MemoryLocation::GpuOnly,
    };
    let mut buffers = Vec::new();
    for _ in 0..32 {
        buffers.push(device.create_buffer(&descriptor).expect("buffer"));
    }
    // Keep an early block resident so its holes remain available. A lone surviving block
    // cannot be evacuated under the strict no-new-backing policy.
    let destination_anchor = buffers.remove(0);
    let survivor = buffers.pop().expect("survivor");
    let pixels: Vec<u8> = (0..descriptor.size as usize)
        .map(|index| ((index * 17) ^ (index / 1024)) as u8)
        .collect();
    device
        .write_buffer(survivor, 0, &pixels)
        .expect("initial contents");
    for buffer in buffers {
        device.destroy_buffer(buffer).expect("holes");
    }
    let report = device.compact_heap().expect("relocation");
    assert!(report.moved_resources > 0 && report.reserved_after < report.reserved_before);
    assert!(report.reserved_peak <= report.reserved_before);
    assert!(report.moved_bytes <= 8 * 1024 * 1024 && report.moved_resources <= 128);
    let readback = device
        .create_buffer_unregistered(&BufferDescriptor {
            usage: BufferUsage::COPY_DST,
            memory_location: MemoryLocation::GpuToCpu,
            ..descriptor
        })
        .expect("readback");
    device
        .copy_buffer_once(
            device.buffers.get(survivor).expect("stable ID").buffer,
            readback.buffer,
            0,
            0,
            pixels.len() as u64,
        )
        .expect("read relocated buffer");
    assert_eq!(
        readback.allocation.mapped_slice().expect("mapped readback"),
        pixels.as_slice()
    );
    device
        .destroy_buffer_now(readback)
        .expect("readback cleanup");
    device.destroy_buffer(survivor).expect("survivor cleanup");
    device
        .destroy_buffer(destination_anchor)
        .expect("destination anchor cleanup");
    println!("GPU-only buffer contents and stable ID verified: {report:?}");
}
