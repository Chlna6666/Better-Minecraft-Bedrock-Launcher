# gfx-metal

[English documentation](README.md)

`gfx-metal` 是 `gfx-core` device trait 的 Metal 实现。

在 Apple 目标上，它创建 Metal device、resource、pipeline、command
encoder 和 `CAMetalLayer` swapchain。在非 Apple 目标上，它实现最小 core
trait stub，并返回 `GfxError::Unavailable`，让非目标平台构建可通过。

当前 `draw_steps_to_texture` 会明确返回 `GfxError::Unavailable`；Metal
offscreen 路径尚未在本次实现中补齐。
