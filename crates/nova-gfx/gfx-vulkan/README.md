# gfx-vulkan

Graphics command recording skips unchanged pipelines, scissors, and descriptor sets within a pass.
Descriptor reuse requires the same pipeline layout. Draw descriptors share one flat temporary
array rather than allocating a separate descriptor-set vector for each draw.

Buffer retirement batches share one cleanup fence/queue submission. Handles are validated before
invalidation, and native buffers remain alive until the cleanup fence completes.

[中文文档](README.zh-CN.md)

`gfx-vulkan` is the Vulkan implementation of the `gfx-core` device traits.

The shared buffer compute API is available when the selected graphics queue supports compute.
Compute batches share one command buffer/fence and reuse compatible descriptor sets for
identical bindings. Buffer uploads recycle fence-completed upload command pools.
Non-indexed indirect drawing requires `drawIndirectFirstInstance`. Compute pipelines are cached;
descriptor pools stay alive through submission fences. Compute and draw commands use the same
ordered queue with memory barriers, without CPU readback or per-dispatch completion waits.

Device-local buffer writes use ordered uploads with destination barriers and fence-retained
staging, without waiting on the CPU. CPU-visible mapped writes still require caller-owned
resource slots that are no longer being read by earlier submissions.

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
