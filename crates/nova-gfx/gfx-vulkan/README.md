# gfx-vulkan

[中文文档](README.zh-CN.md)

`gfx-vulkan` is the Vulkan implementation of the `gfx-core` device traits.

The crate owns Vulkan instance, device, queue, swapchain, resource, pipeline,
and command encoder state. It uses `raw-window-handle` only inside this backend
crate to create native presentation surfaces; `gfx-core` remains independent of
window-handle crates.

## API

Import the relevant `gfx_core::Gfx*Device` trait and call trait methods on
`VulkanDevice`.

```rust
use gfx_core::{
    ClearColor, LoadOp, PresentationDevice, PresentFrameDescriptor, RenderCommand,
    RenderPassId, SwapchainId,
};
use gfx_vulkan::VulkanDevice;

fn present(
    device: &mut VulkanDevice,
    swapchain: SwapchainId,
    render_pass: RenderPassId,
    commands: &[RenderCommand],
) -> gfx_core::Result<()> {
    let _frame = device.render_and_present(&PresentFrameDescriptor {
        swapchain,
        render_pass,
        commands,
        color_load_op: LoadOp::Clear(ClearColor::default()),
        depth_attachment: None,
        present_region: None,
    })?;
    Ok(())
}
```

Backend-specific synchronization details such as native semaphores stay inside
this crate and are not part of the core public API.
