#![cfg(target_os = "windows")]

use gfx_core::{
    BufferDescriptor, BufferUsage, DeviceDescriptor, DiagnosticsDevice, Error, Extent2d, Format,
    MemoryAccounting, MemoryLocation, Origin2d, ResourceDevice, TextureDataLayout,
    TextureDescriptor, TextureDimension, TextureTransferDevice, TextureUsage, TextureWrite,
    TextureWriteDescriptor,
};
use gfx_dx12::Dx12Device;

#[test]
fn texture_write_round_trips_offset_and_row_padding() {
    let mut device = match Dx12Device::new(&DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(reason)) => {
            eprintln!("NOVA_GFX_SKIP=dx12 unavailable: {reason}");
            return;
        }
        Err(error) => panic!("DX12 initialization failed: {error}"),
    };
    eprintln!("NOVA_GFX_ADAPTER=dx12:{}", device.adapter_name());
    let baseline = device.resource_stats();
    let _buffer = device
        .create_buffer(&BufferDescriptor {
            label: Some("memory profile fixture".into()),
            size: 256,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        })
        .expect("buffer creation should succeed");
    let size = Extent2d::new(2, 2).expect("fixture extent should be valid");
    let texture = device
        .create_texture(&TextureDescriptor {
            label: Some("DX12 readback contract".to_string()),
            size,
            mip_level_count: 2,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::COPY_SRC | TextureUsage::COPY_DST | TextureUsage::SAMPLED,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
            sample_count: 1,
        })
        .expect("texture creation should succeed");
    let populated = device.resource_stats();
    assert_eq!(populated.memory_accounting, MemoryAccounting::ResourceSizes);
    assert_eq!(populated.buffers, baseline.buffers + 1);
    assert_eq!(populated.textures, baseline.textures + 1);
    assert!(populated.allocated_bytes >= baseline.allocated_bytes + 256 + 20);
    let budget = device
        .memory_budget()
        .expect("supported driver budget query should succeed");
    if let Some(budget) = budget {
        assert!(budget.local.is_some() || budget.non_local.is_some());
        eprintln!("NOVA_GFX_MEMORY_BUDGET={budget:?}");
    }
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
    if device.supports_transfer_timestamps() {
        assert!(device.last_texture_transfer_time().is_some());
    }
}


/// Native COPY hardware acceptance, private timeline and cross-queue waits.
/// Only the validation path blocks the CPU; production never calls wait here.
#[test]
#[ignore = "requires D3D12 dedicated COPY queue and a hardware driver"]
fn dedicated_copy_queue_buffer_transfer_waits_for_native_gpu_completion() {
    use gfx_core::{GpuBufferCopy, GpuQueueId, GpuWorkStage, SubmissionDevice};
    let mut device = match Dx12Device::new(&DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(_)) => return,
        Err(error) => panic!("D3D12 device initialization failed: {error}"),
    };
    let Some(copy_queue) = device.independent_queue(GpuWorkStage::Transfer) else {
        eprintln!("DX12 native COPY queue unavailable; skipping");
        return;
    };
    let upload = device.create_buffer(&BufferDescriptor {
        label: Some("COPY source".into()),
        size: 64,
        usage: BufferUsage::COPY_SRC,
        memory_location: MemoryLocation::CpuToGpu,
    }).expect("upload buffer");
    let destination = device.create_buffer(&BufferDescriptor {
        label: Some("COPY destination".into()),
        size: 64,
        usage: BufferUsage::COPY_DST | BufferUsage::STORAGE,
        memory_location: MemoryLocation::GpuOnly,
    }).expect("device buffer");
    device.write_buffer(upload, 0, &[0x31; 64]).expect("upload contents");
    let producer = device.submit_buffer_copies_on_queue(copy_queue, &[GpuBufferCopy {
        source: upload, source_offset: 0,
        destination, destination_offset: 0, bytes: 64,
    }]).expect("native typed COPY command execution");
    let join = device.enqueue_gpu_queue_wait(GpuQueueId::PRIMARY, producer)
        .expect("GPU-side graphics wait on COPY producer");
    assert_eq!(join.producer(), producer);
    let primary = device.checkpoint_gpu_queue(GpuQueueId::PRIMARY)
        .expect("primary queue fence after COPY wait");
    device.wait_submission(primary).expect("native primary GPU join");
    device.wait_submission(producer).expect("native COPY completion");
    device.destroy_buffer(destination).expect("destroy copied buffer");
    device.destroy_buffer(upload).expect("destroy upload staging");
}
