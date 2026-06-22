# gfx-dx12

[English documentation](README.md)

`gfx-dx12` 是 `gfx-core` device trait 的 Direct3D 12 实现。

在 Windows 上，它创建原生 D3D12 device、resource、pipeline、command
encoder 和 DXGI swapchain。在非 Windows 目标上，它仍然实现 core trait，
但以最小 stub 返回 `GfxError::Unavailable`，方便跨平台 crate 编译非目标
路径。

`raw-window-handle` 只在本后端 crate 中用于原生 surface 创建。`gfx-core`
不依赖它。
