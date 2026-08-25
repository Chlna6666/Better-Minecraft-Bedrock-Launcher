use criterion::{Criterion, Throughput, black_box};
use gfx_core::{
    DeviceDesc, Extent2d, Format, GfxError, GfxResourceDevice, GfxTextureTransferDevice,
    MemoryLocation, Origin2d, TextureDataLayout, TextureDesc, TextureDimension, TextureUsage,
    TextureWrite, TextureWriteDesc,
};
use gfx_vulkan::VulkanDevice;

const TILE_EDGE: u32 = 16;

fn main() {
    let mut device = match VulkanDevice::new(&DeviceDesc::default()) {
        Ok(device) => device,
        Err(GfxError::Unavailable(reason)) => {
            eprintln!("NOVA_GFX_SKIP=vulkan unavailable: {reason}");
            return;
        }
        Err(error) => panic!("Vulkan initialization failed: {error}"),
    };
    eprintln!("NOVA_GFX_ADAPTER=vulkan:{}", device.adapter_name());
    run(&mut device);
}

fn run(device: &mut VulkanDevice) {
    let atlas_size = Extent2d::new(128, 128).expect("atlas extent should be valid");
    let texture = device
        .create_texture(&TextureDesc {
            label: Some("Vulkan texture-write benchmark".to_string()),
            size: atlas_size,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::COPY_SRC | TextureUsage::COPY_DST | TextureUsage::SAMPLED,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        })
        .expect("benchmark texture creation should succeed");
    let tile = vec![0x7f_u8; (TILE_EDGE * TILE_EDGE * 4) as usize];
    let layout =
        TextureDataLayout::new(0, TILE_EDGE * 4, TILE_EDGE).expect("tile layout should be valid");
    let size = Extent2d::new(TILE_EDGE, TILE_EDGE).expect("tile extent should be valid");
    let mut criterion = Criterion::default().configure_from_args();
    let mut group = criterion.benchmark_group("native_texture_write/vulkan");
    for count in [1_usize, 8, 64] {
        let descriptors = (0..count)
            .map(|index| TextureWriteDesc {
                texture,
                layout,
                origin: Origin2d {
                    x: (index as u32 % 8) * TILE_EDGE,
                    y: (index as u32 / 8) * TILE_EDGE,
                },
                size,
            })
            .collect::<Vec<_>>();
        let writes = descriptors
            .iter()
            .copied()
            .map(|descriptor| TextureWrite {
                descriptor,
                data: &tile,
            })
            .collect::<Vec<_>>();
        report_gpu_time(device, &writes, count);
        group.throughput(Throughput::Bytes((tile.len() * count) as u64));
        group.bench_function(count.to_string(), |bencher| {
            bencher.iter(|| {
                device
                    .write_texture_batch(writes.iter().copied())
                    .expect("benchmark upload should succeed");
                device
                    .wait_texture_transfers()
                    .expect("benchmark transfer wait should succeed");
                black_box(device.last_texture_transfer_time());
            });
        });
    }
    group.finish();
    criterion.final_summary();
}

fn report_gpu_time(device: &mut VulkanDevice, writes: &[TextureWrite<'_>], count: usize) {
    let mut samples = Vec::with_capacity(32);
    for _ in 0..32 {
        device
            .write_texture_batch(writes.iter().copied())
            .expect("GPU timestamp upload should succeed");
        device
            .wait_texture_transfers()
            .expect("GPU timestamp wait should succeed");
        if let Some(sample) = device.last_texture_transfer_time() {
            samples.push(sample);
        }
    }
    samples.sort_unstable();
    if let Some(median) = samples.get(samples.len() / 2) {
        eprintln!(
            "NOVA_GFX_GPU_MEDIAN=vulkan batch={count} samples={} nanoseconds={}",
            samples.len(),
            median.as_nanos()
        );
    } else {
        eprintln!("NOVA_GFX_GPU_TIMESTAMP_UNAVAILABLE=vulkan batch={count}");
    }
}
