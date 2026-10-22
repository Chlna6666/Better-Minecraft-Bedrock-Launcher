//! Native retirement checks; no shader compilation or application event loop is required.
use super::*;

// Only the two window-lifetime calls are needed by this backend test. Keep them local
// rather than enabling the UI feature set in the production windows dependency.
#[link(name = "user32")]
unsafe extern "system" {
    fn CreateWindowExW(
        extended_style: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: HWND,
        menu: *mut std::ffi::c_void,
        instance: *mut std::ffi::c_void,
        parameter: *mut std::ffi::c_void,
    ) -> HWND;
    fn DestroyWindow(window: HWND) -> i32;
}

struct Window(HWND);
impl Window {
    fn new() -> Self {
        // SAFETY: STATIC is a system class. This hidden window is uniquely owned
        // on the test thread, with no parent, menu, instance or creation parameter.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                windows::core::w!("STATIC").as_ptr(),
                ptr::null(),
                0x8000_0000,
                0,
                0,
                64,
                64,
                HWND::default(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        assert!(!hwnd.0.is_null(), "create test HWND");
        Self(hwnd)
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: test swapchains/surfaces have retired before this owned HWND.
        assert_ne!(unsafe { DestroyWindow(self.0) }, 0, "destroy test HWND");
    }
}

struct QueueBlock(windows::Win32::Graphics::Direct3D12::ID3D12Fence);
impl QueueBlock {
    fn new(device: &mut Dx12Device) -> Self {
        // SAFETY: this fence belongs to the live device; the CPU releases the
        // queued wait in Drop, including on assertion failure.
        let fence = unsafe { device.device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }
            .expect("test queue gate");
        unsafe { device.graphics_queue.Wait(&fence, 1) }.expect("queue gate wait");
        Self(fence)
    }
}
impl Drop for QueueBlock {
    fn drop(&mut self) {
        // SAFETY: CPU signaling does not depend on the blocked GPU queue.
        unsafe { self.0.Signal(1) }.expect("release test queue gate");
    }
}

#[test]
#[ignore = "requires Windows hardware D3D12 and a desktop session"]
fn resize_retirement_excludes_later_work_and_rejects_old_commands() {
    let mut device = Dx12Device::new(&DeviceDescriptor::default()).expect("DX12 device");
    for alpha in [
        CompositeAlphaMode::Opaque,
        CompositeAlphaMode::Premultiplied,
    ] {
        verify_retirement(&mut device, alpha);
    }
}

fn verify_retirement(device: &mut Dx12Device, alpha: CompositeAlphaMode) {
    let window = Window::new();
    let surface = device.surfaces.insert(Dx12Surface {
        label: None,
        hwnd: window.0,
    });
    let chain = device
        .create_swapchain(
            surface,
            SurfaceConfig {
                size: gfx_core::Extent2d::new(64, 64).expect("extent"),
                format: Format::Bgra8Unorm,
                present_mode: PresentMode::Mailbox,
                alpha_mode: alpha,
            },
        )
        .expect("native swapchain");
    verify_native_scaling(device, chain);
    seed_published_composition(device, chain);
    verify_submission_invalidation(device, chain);
    let encoder = device
        .create_command_encoder(&CommandEncoderDescriptor { label: None })
        .expect("encoder");
    device
        .bind_swapchain_generation(encoder, chain)
        .expect("record buffer generation");
    device
        .poll_swapchain_resize(chain)
        .expect("prepare retirement");
    let retirement = device
        .swapchains
        .get(chain)
        .expect("chain")
        .resize_retirement
        .expect("fence");
    device
        .wait_for_fence_value(retirement)
        .expect("retire original buffers");

    verify_later_work(device, chain, retirement);
    verify_resize_generation(device, chain, encoder);
    device
        .destroy_command_encoder(encoder)
        .expect("destroy old command list");
    device.destroy_swapchain(chain).expect("destroy chain");
    device.destroy_surface(surface).expect("destroy surface");
}

fn verify_native_scaling(device: &Dx12Device, chain: SwapchainId) {
    let record = device.swapchains.get(chain).expect("chain");
    // SAFETY: the native swapchain and its HWND stay live while its descriptor is read.
    let descriptor = unsafe { record.swapchain.GetDesc1() }.expect("native descriptor");
    assert_eq!(
        descriptor.Scaling, DXGI_SCALING_STRETCH,
        "a grown HWND must be covered by scaling the last presented frame"
    );
}

fn verify_later_work(device: &mut Dx12Device, chain: SwapchainId, retirement: u64) {
    let gate = QueueBlock::new(device);
    device.signal_frame().expect("later unrelated work");
    assert!(device.has_pending_gpu_work().expect("blocked later work"));
    for _ in 0..8 {
        assert!(
            device
                .prepare_swapchain_resize(chain)
                .expect("window already retired")
        );
        assert_eq!(
            device
                .swapchains
                .get(chain)
                .expect("chain")
                .resize_retirement,
            Some(retirement)
        );
    }
    drop(gate);
}

fn verify_resize_generation(
    device: &mut Dx12Device,
    chain: SwapchainId,
    encoder: CommandEncoderId,
) {
    let previous = device
        .swapchains
        .get(chain)
        .expect("old chain")
        .swapchain
        .clone();
    let before_resize = device.next_fence_value;
    device
        .resize_swapchain(chain, 96, 80)
        .expect("resize prepared buffers");
    assert_eq!(
        device.next_fence_value, before_resize,
        "prepared resize must not enqueue a new device-wide wait fence"
    );
    let resized = device.swapchains.get(chain).expect("resized chain");
    assert_eq!(resized.generation, 1);
    assert_eq!(
        resized.source_extent,
        gfx_core::Extent2d::new(96, 80).unwrap()
    );
    assert_eq!(
        previous, resized.swapchain,
        "size-only resize must retain the native presentation stream"
    );
    if let Some(composition) = &resized.composition {
        assert_eq!(
            composition.active_content.as_ref(),
            Some(&previous),
            "resize must retain the visual's native content binding"
        );
    }
    assert!(resized.resize_retirement.is_none());
    assert!(
        matches!(
            device.submit_without_wait(encoder),
            Err(Error::InvalidInput(_))
        ),
        "commands for old native buffers must never be executed"
    );
}

fn seed_published_composition(device: &mut Dx12Device, chain: SwapchainId) {
    let record = device.swapchains.get_mut(chain).expect("chain");
    if let Some(composition) = &mut record.composition {
        assert!(
            composition.active_content.is_none(),
            "new native content starts unpublished"
        );
        // This ownership test needs no shaders. Seed the state established by first Present;
        // The consumer renderer separately exercises real rendering and publication.
        composition.active_content = Some(record.swapchain.clone());
    }
}

fn verify_submission_invalidation(device: &mut Dx12Device, chain: SwapchainId) {
    device
        .poll_swapchain_resize(chain)
        .expect("old retirement point");
    let encoder = device
        .create_command_encoder(&CommandEncoderDescriptor { label: None })
        .expect("new encoder");
    device
        .bind_swapchain_generation(encoder, chain)
        .expect("new backbuffer commands");
    let submitted = device.submit_deferred(encoder).expect("new reader");
    assert!(
        device
            .swapchains
            .get(chain)
            .expect("chain")
            .resize_retirement
            .is_none(),
        "a new submission must invalidate the earlier retirement point"
    );
    device
        .wait_submission(submitted)
        .expect("new reader retirement");
}
