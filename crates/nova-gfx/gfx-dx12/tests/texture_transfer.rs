#![cfg(target_os = "windows")]

use gfx_core::{
    DeviceDesc, Extent2d, Format, GfxError, GfxResourceDevice, GfxTextureTransferDevice,
    MemoryLocation, Origin2d, TextureDataLayout, TextureDesc, TextureDimension, TextureUsage,
    TextureWriteDesc,
};
use gfx_dx12::Dx12Device;

#[test]
fn texture_write_round_trips_offset_and_row_padding() {
    let mut device = match Dx12Device::new(&DeviceDesc::default()) {
        Ok(device) => device,
        Err(GfxError::Unavailable(reason)) => {
            eprintln!("NOVA_GFX_SKIP=dx12 unavailable: {reason}");
            return;
        }
        Err(error) => panic!("DX12 initialization failed: {error}"),
    };
    eprintln!("NOVA_GFX_ADAPTER=dx12:{}", device.adapter_name());
    let size = Extent2d::new(2, 2).expect("fixture extent should be valid");
    let texture = device
        .create_texture(&TextureDesc {
            label: Some("DX12 readback contract".to_string()),
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
