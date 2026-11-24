#![cfg(target_vendor = "apple")]

use gfx_core::{
    DeviceDescriptor, Error, Extent2d, Format, MemoryLocation, Origin2d,
    ResourceDevice, TextureCopy, TextureDataLayout, TextureDescriptor, TextureDimension,
    TextureTransferDevice, TextureUsage, TextureWriteDescriptor,
};
use gfx_metal::MetalDevice;

#[test]
fn private_texture_upload_copy_and_padded_readback() {
    let mut device = match MetalDevice::new(&DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(reason)) => {
            eprintln!("NOVA_GFX_SKIP=metal unavailable: {reason}");
            return;
        }
        Err(error) => panic!("Metal initialization failed: {error}"),
    };
    let size = Extent2d::new(257, 3).expect("extent");
    let desc = TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        format: Format::Rgba8Unorm,
        usage: TextureUsage::COPY_SRC | TextureUsage::COPY_DST,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
        sample_count: 1,
    };
    let source = device.create_texture(&desc).expect("source");
    let destination = device.create_texture(&desc).expect("destination");
    let row = size.width() as usize * 4;
    let source_pitch = row + 12;
    let mut input = vec![0xee; 4 + source_pitch * 3];
    let expected: Vec<u8> = (0..row * 3).map(|i| (i % 251) as u8).collect();
    for y in 0..3 {
        input[4 + y * source_pitch..4 + y * source_pitch + row]
            .copy_from_slice(&expected[y * row..(y + 1) * row]);
    }
    device.write_texture(TextureWriteDescriptor {
        texture: source,
        mip_level: 0,
        origin: Origin2d::ZERO,
        size,
        layout: TextureDataLayout::new(4, source_pitch as u32, 3).expect("layout"),
    }, &input).expect("padded upload");
    device.copy_texture_batch(&[TextureCopy {
        source, destination, source_origin: Origin2d::ZERO,
        destination_origin: Origin2d::ZERO, size,
    }]).expect("native texture copy");
    device.wait_texture_transfers().expect("transfer completion");
    assert!(device.supports_transfer_timestamps());
    if let Some(duration) = device.last_texture_transfer_time() {
        assert!(!duration.is_zero());
        eprintln!("NOVA_GFX_METAL_TRANSFER_NS={}", duration.as_nanos());
    } else {
        eprintln!("NOVA_GFX_METAL_TRANSFER_TIMESTAMP_UNAVAILABLE");
    }
    let readback = device.read_texture(destination).expect("readback");
    assert_eq!(readback.size, size);
    assert_eq!(readback.bytes_per_row, row as u32);
    assert_eq!(readback.bytes, expected);
    let upload_only = device.create_texture(&TextureDescriptor {
        usage: TextureUsage::COPY_DST, ..desc
    }).expect("upload-only texture");
    assert!(matches!(device.read_texture(upload_only), Err(Error::InvalidInput(_))));
    for id in [upload_only, source, destination] {
        device.destroy_texture(id).expect("destroy texture");
    }
}
