use gfx_core::{
    DeviceDescriptor, DiagnosticsDevice, Error, Extent2d, Format, MemoryLocation, Origin2d,
    ResourceDevice, TextureDataLayout, TextureDescriptor, TextureDimension, TextureTransferDevice,
    TextureUsage, TextureWrite, TextureWriteDescriptor,
};
use gfx_vulkan::VulkanDevice;

#[test]
fn moderate_and_aggressive_trim_preserve_live_pixels_and_release_empty_blocks() {
    let mut device = match VulkanDevice::new(&DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(reason)) => {
            eprintln!("NOVA_GFX_SKIP=vulkan unavailable: {reason}");
            return;
        }
        Err(error) => panic!("Vulkan initialization failed: {error}"),
    };
    let descriptor = TextureDescriptor {
        label: None,
        size: Extent2d::new(2, 2).expect("extent"),
        mip_level_count: 1,
        format: Format::Rgba8Unorm,
        usage: TextureUsage::COPY_SRC | TextureUsage::COPY_DST | TextureUsage::SAMPLED,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
            sample_count: 1,
        };
    for empty_trim in [
        gfx_core::MemoryTrimLevel::Moderate,
        gfx_core::MemoryTrimLevel::Aggressive,
    ] {
        let texture = device
            .create_texture(&descriptor)
            .expect("texture after trim");
        let pixels = [
            11, 22, 33, 0, 44, 55, 66, 127, 77, 88, 99, 255, 12, 34, 56, 78,
        ];
        device
            .write_texture(
                TextureWriteDescriptor {
                    texture,
                    mip_level: 0,
                    origin: Origin2d::ZERO,
                    size: descriptor.size,
                    layout: TextureDataLayout::new(0, 8, 2).expect("layout"),
                },
                &pixels,
            )
            .expect("upload");
        device
            .trim_memory(gfx_core::MemoryTrimLevel::Moderate)
            .expect("moderate trim with live texture");
        assert_eq!(
            device
                .read_texture(texture)
                .expect("pixels after moderate trim")
                .bytes,
            pixels
        );
        device
            .trim_memory(gfx_core::MemoryTrimLevel::Aggressive)
            .expect("trim with live texture");
        assert_eq!(
            device
                .read_texture(texture)
                .expect("live texture after trim")
                .bytes,
            pixels
        );
        device.destroy_texture(texture).expect("destroy texture");
        device
            .trim_memory(gfx_core::MemoryTrimLevel::Light)
            .expect("light trim");
        let cached = device.resource_stats();
        assert_eq!(cached.allocated_bytes, 0);
        assert!(cached.reserved_bytes > 0);
        device.trim_memory(empty_trim).expect("trim empty blocks");
        let trimmed = device.resource_stats();
        assert_eq!(trimmed.allocated_bytes, 0);
        assert_eq!(trimmed.reserved_bytes, 0);
        eprintln!(
            "VULKAN_EMPTY_BLOCK_TRIM reserved_bytes={} -> {}",
            cached.reserved_bytes, trimmed.reserved_bytes
        );
    }
}

#[test]
fn texture_write_round_trips_offset_and_row_padding() {
    let mut device = match VulkanDevice::new(&DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(reason)) => {
            eprintln!("NOVA_GFX_SKIP=vulkan unavailable: {reason}");
            return;
        }
        Err(error) => panic!("Vulkan initialization failed: {error}"),
    };
    eprintln!("NOVA_GFX_ADAPTER=vulkan:{}", device.adapter_name());
    let size = Extent2d::new(2, 2).expect("fixture extent should be valid");
    let texture = device
        .create_texture(&TextureDescriptor {
            label: Some("Vulkan readback contract".to_string()),
            size,
            mip_level_count: 2,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::COPY_SRC | TextureUsage::COPY_DST | TextureUsage::SAMPLED,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
            sample_count: 1,
        })
        .expect("texture creation should succeed");
    let source = [
        0xee, 0xee, 0xee, 0xee, 1, 2, 3, 4, 5, 6, 7, 8, 0xaa, 0xaa, 0xaa, 0xaa, 9, 10, 11, 12, 13,
        14, 15, 16,
    ];
    let base_write = TextureWrite {
        descriptor: TextureWriteDescriptor {
            texture,
            mip_level: 0,
            layout: TextureDataLayout::new(4, 12, 2).expect("fixture layout should be valid"),
            origin: Origin2d::ZERO,
            size,
        },
        data: &source,
    };
    let mip = [17, 18, 19, 20];
    let mip_write = TextureWrite {
        descriptor: TextureWriteDescriptor {
            texture,
            mip_level: 1,
            layout: TextureDataLayout::new(0, 4, 1).expect("mip layout should be valid"),
            origin: Origin2d::ZERO,
            size: Extent2d::new(1, 1).expect("mip extent should be valid"),
        },
        data: &mip,
    };
    device
        .write_texture_batch([base_write, mip_write])
        .expect("base and mip uploads should succeed in one batch");

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
    let mut device = match VulkanDevice::new(&DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(reason)) => {
            eprintln!("NOVA_GFX_SKIP=vulkan unavailable: {reason}");
            return;
        }
        Err(error) => panic!("Vulkan initialization failed: {error}"),
    };
    let baseline = device.resource_stats();
    assert_eq!(
        baseline.reserved_bytes, 0,
        "allocator construction must not allocate blocks"
    );
    let size = Extent2d::new(256, 256).expect("fixture extent should be valid");
    let mut textures = Vec::with_capacity(64);
    for index in 0..64 {
        textures.push(
            device
                .create_texture(&TextureDescriptor {
                    label: Some(format!("Vulkan managed allocation fixture {index}")),
                    size,
                    mip_level_count: 1,
                    format: Format::Rgba8Unorm,
                    usage: TextureUsage::COPY_DST | TextureUsage::SAMPLED,
                    memory_location: MemoryLocation::GpuOnly,
                    dimension: TextureDimension::D2,
            sample_count: 1,
        })
                .expect("managed texture creation should succeed"),
        );
        if index == 0 {
            assert!(
                device.resource_stats().reserved_bytes <= 8 * 1024 * 1024,
                "one small texture must not allocate the old 256 MiB pool"
            );
        }
    }

    let populated = device.resource_stats();
    assert_eq!(
        populated.memory_accounting,
        gfx_core::MemoryAccounting::Allocator
    );
    assert_eq!(populated.textures, baseline.textures + 64);
    if let Some(budget) = device.memory_budget().expect("supported budget query") {
        assert!(budget.local.is_some() || budget.non_local.is_some());
        eprintln!("NOVA_GFX_MEMORY_BUDGET={budget:?}");
    }
    assert!(populated.allocated_bytes > baseline.allocated_bytes);
    assert!(populated.reserved_bytes >= populated.allocated_bytes);
    assert!(
        populated.reserved_bytes < 64 * 1024 * 1024,
        "16 MiB of textures should grow the pool in demand-sized blocks"
    );
    eprintln!(
        "VULKAN_DEMAND_SIZED_MEMORY allocated={} reserved={}",
        populated.allocated_bytes, populated.reserved_bytes
    );
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


#[test]
#[ignore = "requires Vulkan hardware with a second graphics-family queue"]
fn secondary_gpu_buffer_copy_has_native_queue_fence_and_reverse_gpu_wait() {
    use gfx_core::{
        BufferDescriptor, BufferUsage, GpuBufferCopy, GpuQueueId, GpuWorkStage,
        SubmissionDevice,
    };
    let mut device = match gfx_vulkan::VulkanDevice::new(&DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(_)) => return,
        Err(error) => panic!("Vulkan initialization: {error}"),
    };
    let Some(secondary) = device.independent_queue(GpuWorkStage::Transfer) else {
        eprintln!("Vulkan has no second graphics-family queue; skipping");
        return;
    };
    let src = device.create_buffer(&BufferDescriptor {
        label: Some("native Copy source".into()),
        size: 64,
        usage: BufferUsage::COPY_SRC | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::GpuOnly,
    }).expect("source");
    let dst = device.create_buffer(&BufferDescriptor {
        label: Some("native Copy destination".into()),
        size: 64,
        usage: BufferUsage::COPY_DST | BufferUsage::STORAGE,
        memory_location: MemoryLocation::GpuOnly,
    }).expect("destination");
    device.write_buffer(src, 0, &[0x37; 64]).expect("upload");
    let producer = device.submit_buffer_copies_on_queue(secondary, &[GpuBufferCopy {
        source: src, source_offset: 0,
        destination: dst, destination_offset: 0,
        bytes: 64,
    }]).expect("vkCmdCopyBuffer executed on secondary graphics-family queue");
    let proof = device.enqueue_gpu_queue_wait(GpuQueueId::PRIMARY, producer)
        .expect("native secondary to primary GPU wait");
    assert_eq!(proof.producer(), producer);
    let primary = device.checkpoint_gpu_queue(GpuQueueId::PRIMARY)
        .expect("native primary join fence");
    // Host waits only in ignored hardware conformance tests, not in runtime.
    device.wait_submission(primary).expect("primary GPU queue completion");
    device.wait_submission(producer).expect("producer physical queue completion");
    device.destroy_buffer(dst).expect("destroy destination");
    device.destroy_buffer(src).expect("destroy source");
}
