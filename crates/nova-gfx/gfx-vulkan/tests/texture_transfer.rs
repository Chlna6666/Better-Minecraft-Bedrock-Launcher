use gfx_core::{
    DeviceDesc, Extent2d, Format, GfxDiagnosticsDevice, GfxError, GfxResourceDevice,
    GfxTextureTransferDevice, MemoryLocation, Origin2d, TextureDataLayout, TextureDesc,
    TextureDimension, TextureUsage, TextureWriteDesc,
};
use gfx_vulkan::VulkanDevice;

#[test]
fn texture_write_round_trips_offset_and_row_padding() {
    let mut device = match VulkanDevice::new(&DeviceDesc::default()) {
        Ok(device) => device,
        Err(GfxError::Unavailable(reason)) => {
            eprintln!("NOVA_GFX_SKIP=vulkan unavailable: {reason}");
            return;
        }
        Err(error) => panic!("Vulkan initialization failed: {error}"),
    };
    eprintln!("NOVA_GFX_ADAPTER=vulkan:{}", device.adapter_name());
    let size = Extent2d::new(2, 2).expect("fixture extent should be valid");
    let texture = device
        .create_texture(&TextureDesc {
            label: Some("Vulkan readback contract".to_string()),
            size,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::COPY_SRC | TextureUsage::COPY_DST | TextureUsage::SAMPLED,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        })
        .expect("texture creation should succeed");
    let source = [
        0xee, 0xee, 0xee, 0xee, 1, 2, 3, 4, 5, 6, 7, 8, 0xaa, 0xaa, 0xaa, 0xaa, 9, 10, 11, 12, 13,
        14, 15, 16,
    ];
    device
        .write_texture(
            TextureWriteDesc {
                texture,
                layout: TextureDataLayout::new(4, 12, 2).expect("fixture layout should be valid"),
                origin: Origin2d::ZERO,
                size,
            },
            &source,
        )
        .expect("texture upload should succeed");

    let readback = device
        .read_texture(texture)
        .expect("texture readback should succeed");

    assert_eq!(readback.bytes_per_row, 8);
    assert_eq!(
        readback.bytes,
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
    );
    if device.texture_transfer_timestamps_supported() {
        assert!(device.last_texture_transfer_time().is_some());
    }
}

#[test]
fn managed_texture_memory_reports_live_and_reserved_bytes() {
    let mut device = match VulkanDevice::new(&DeviceDesc::default()) {
        Ok(device) => device,
        Err(GfxError::Unavailable(reason)) => {
            eprintln!("NOVA_GFX_SKIP=vulkan unavailable: {reason}");
            return;
        }
        Err(error) => panic!("Vulkan initialization failed: {error}"),
    };
    let baseline = device.resource_stats();
    let size = Extent2d::new(64, 64).expect("fixture extent should be valid");
    let mut textures = Vec::with_capacity(64);
    for index in 0..64 {
        textures.push(
            device
                .create_texture(&TextureDesc {
                    label: Some(format!("Vulkan managed allocation fixture {index}")),
                    size,
                    format: Format::Rgba8Unorm,
                    usage: TextureUsage::COPY_DST | TextureUsage::SAMPLED,
                    memory_location: MemoryLocation::GpuOnly,
                    dimension: TextureDimension::D2,
                })
                .expect("managed texture creation should succeed"),
        );
    }

    let populated = device.resource_stats();
    assert!(populated.allocated_bytes > baseline.allocated_bytes);
    assert!(populated.reserved_bytes >= populated.allocated_bytes);
    assert!(populated.reserved_memory_utilization().is_some());

    for texture in textures {
        device
            .destroy_texture(texture)
            .expect("managed texture destruction should succeed");
    }
    device
        .wait_texture_transfers()
        .expect("deferred frees should retire");

    let released = device.resource_stats();
    assert_eq!(released.allocated_bytes, baseline.allocated_bytes);
    assert!(released.reserved_bytes >= baseline.reserved_bytes);
    assert_eq!(
        released.unused_reserved_bytes(),
        released
            .reserved_bytes
            .saturating_sub(released.allocated_bytes)
    );
}
