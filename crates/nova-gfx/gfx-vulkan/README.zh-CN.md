# gfx-vulkan

[English documentation](README.md)

`gfx-vulkan` 是 `gfx-core` device trait 的 Vulkan 实现。

本 crate 拥有 Vulkan instance、device、queue、swapchain、resource、
pipeline 和 command encoder 状态。它只在后端内部使用 `raw-window-handle`
创建原生 presentation surface；`gfx-core` 不依赖窗口句柄 crate。

## API

调用方导入需要的 `gfx_core::Gfx*Device` trait，然后在 `VulkanDevice` 上
调用 trait 方法。

```rust
use gfx_core::{ClearColor, DrawStepDesc, GfxPresentationDevice, RenderPassId, SwapchainId};
use gfx_vulkan::VulkanDevice;

fn present(
    device: &mut VulkanDevice,
    swapchain: SwapchainId,
    render_pass: RenderPassId,
    steps: &[DrawStepDesc],
    clear: ClearColor,
) -> gfx_core::Result<()> {
    device.draw_steps_and_present(swapchain, render_pass, steps, clear)
}
```

原生 semaphore 等后端同步细节保留在本 crate 内部，不进入 core 公共 API。
