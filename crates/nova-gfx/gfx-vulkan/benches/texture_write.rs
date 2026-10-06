use criterion::{Criterion, Throughput, black_box};
use gfx_core::{
    DeviceDescriptor, Error, Extent2d, Format, MemoryLocation, Origin2d, ResourceDevice,
    TextureDataLayout, TextureDescriptor, TextureDimension, TextureId, TextureTransferDevice,
    TextureUsage, TextureWrite, TextureWriteDescriptor,
};
use gfx_vulkan::VulkanDevice;

const TILE_EDGE: u32 = 16;

fn main() {
    let mut device = match VulkanDevice::new(&DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(reason)) => {
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
        .create_texture(&TextureDescriptor {
            label: Some("Vulkan texture-write benchmark".to_string()),
            size: atlas_size,
            mip_level_count: 1,
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
            .map(|index| TextureWriteDescriptor {
                texture,
                mip_level: 0,
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
    bench_mip_chain(device, &mut criterion);
    criterion.final_summary();
}

fn bench_mip_chain(device: &mut VulkanDevice, criterion: &mut Criterion) {
    let (texture, levels) = create_mip_chain(device);
    let writes = mip_chain_writes(texture, &levels);
    let byte_count = levels
        .iter()
        .map(|(_, _, pixels)| pixels.len())
        .sum::<usize>();
    report_gpu_time(device, &writes, 8);
    let mut group = criterion.benchmark_group("native_texture_write/vulkan/mip-chain");
    group.throughput(Throughput::Bytes(
        u64::try_from(byte_count).expect("mip bytes should fit u64"),
    ));
    group.bench_function("128x128-8-levels", |bencher| {
        bencher.iter(|| {
            device
                .write_texture_batch(writes.iter().copied())
                .expect("mip-chain upload should succeed");
            device
                .wait_texture_transfers()
                .expect("mip-chain wait should succeed");
            black_box(device.last_texture_transfer_time());
        });
    });
    group.finish();
}

fn create_mip_chain(device: &mut VulkanDevice) -> (TextureId, Vec<(u32, u32, Vec<u8>)>) {
    let texture = device
        .create_texture(&TextureDescriptor {
            label: Some("Vulkan mip-chain upload benchmark".to_string()),
            size: Extent2d::new(128, 128).expect("mip-chain extent should be valid"),
            mip_level_count: 8,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::COPY_DST | TextureUsage::SAMPLED,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        })
        .expect("mip-chain texture creation should succeed");
    let levels = (0_u32..8)
        .map(|mip_level| {
            let width = 128 >> mip_level;
            let height = width;
            let byte_count =
                usize::try_from(width * height * 4).expect("mip byte count should fit usize");
            let pixels =
                vec![u8::try_from(mip_level).expect("mip index should fit u8"); byte_count];
            (width, height, pixels)
        })
        .collect::<Vec<_>>();
    (texture, levels)
}

fn mip_chain_writes<'a>(
    texture: TextureId,
    levels: &'a [(u32, u32, Vec<u8>)],
) -> Vec<TextureWrite<'a>> {
    levels
        .iter()
        .enumerate()
        .map(|(mip_level, (width, height, pixels))| TextureWrite {
            descriptor: TextureWriteDescriptor {
                texture,
                mip_level: u32::try_from(mip_level).expect("mip index should fit u32"),
                layout: TextureDataLayout::new(0, width * 4, *height)
                    .expect("mip layout should be valid"),
                origin: Origin2d::ZERO,
                size: Extent2d::new(*width, *height).expect("mip extent should be valid"),
            },
            data: pixels,
        })
        .collect::<Vec<_>>()
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
