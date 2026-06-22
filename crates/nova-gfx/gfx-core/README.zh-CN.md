# gfx-core

[English documentation](README.md)

`gfx-core` 定义 `nova-gfx` 的后端无关规范 API。

它包含：

- 类型化的 generational resource handle；
- resource、pipeline、command、surface、presentation、diagnostics 描述符；
- `GfxError` 和 `Result`；
- 公共后端能力 trait：
  `GfxBackend`、`GfxSurfaceDevice`、`GfxResourceDevice`、`GfxPipelineDevice`、
  `GfxCommandDevice`、`GfxPresentationDevice`、`GfxDiagnosticsDevice` 和
  `GfxDevice`。

`gfx-core` 不依赖 GPUI、winit、`raw-window-handle`、Vulkan、Direct3D 12
或 Metal。原生窗口目标通过 `GfxSurfaceDevice::SurfaceTarget` 表达，由各
后端 crate 自己选择具体类型。

## 推荐用法

helper 应使用最窄的 trait bound：

```rust
use gfx_core::{BufferDesc, BufferId, GfxResourceDevice, Result};

fn create_upload_buffer<D>(device: &mut D, desc: &BufferDesc) -> Result<BufferId>
where
    D: GfxResourceDevice,
{
    device.create_buffer(desc)
}
```

只有确实需要完整设备能力的代码才使用 `GfxDevice`。

## API 稳定性

`0.1.x` 是早期破坏性 API 线。本 crate 中的 trait 是维护中的公共契约；
后端 crate 应实现这些 trait，而不是暴露重复的 public inherent method。
