# nova-gfx

[English documentation](README.md)

`nova-gfx` 是 BMCBL 和后续项目使用的轻量图形基础库。`gfx-core` 是
公共规范接口，负责定义后端无关的描述符、类型化句柄、错误类型和设备
trait；Vulkan、Direct3D 12、Metal 后端 crate 负责实现这些 trait。

当前 crate：

- `gfx-core`：后端无关的描述符、句柄、错误和 `Gfx*Device` trait。
  它不依赖 GPUI、winit、raw-window-handle、Vulkan、DX12 或 Metal。
- `gfx-memory`：GPU 内存分配封装、upload-ring 统计、延迟释放队列和
  内存统计。
- `gfx-shader`：通过 Naga 做 WGSL 校验，并生成 SPIR-V、HLSL、MSL。
- `gfx-dx12`：基于 `windows-rs` 的 Windows Direct3D 12 后端。
- `gfx-metal`：基于 `objc2-metal` 的 Apple Metal 后端。
- `gfx-vulkan`：基于 `ash` 的 Vulkan 后端。
- `examples/triangle*`：使用 core trait 的最小窗口三角形示例。
- `examples/atlas-smoke*`：使用 core trait 的 atlas/resource binding smoke test。

## 公共 API

调用方应按需要选择最窄的 core trait：

- `GfxSurfaceDevice`：surface 和 swapchain。
- `GfxResourceDevice`：buffer、texture、view、sampler、resource set。
- `GfxPipelineDevice`：shader、render pass、layout、pipeline。
- `GfxCommandDevice`：显式 command encoder 工作流。
- `GfxPresentationDevice`：`draw_steps_and_present` 和 `draw_steps_to_texture`。
- `GfxDiagnosticsDevice`：资源统计。

只有调用方确实需要完整设备能力时，才使用组合 trait `GfxDevice`。通用
helper 优先使用更窄的 trait bound，避免给后端增加不必要的 API 要求。

## 发布

crates.io 发布顺序和 package 校验清单见 [PUBLISHING.zh-CN.md](PUBLISHING.zh-CN.md)。
发布顺序很重要，因为后端 crate 依赖 `gfx-core`，而 crates.io dry-run 要求
依赖版本已经存在于 registry。

## Feature 选择

`gfx-memory` 默认不启用任何后端 feature。调用方应只选择实际需要的 allocator
后端：

```powershell
rtk cargo test -p gfx-memory --no-default-features
rtk cargo test -p gfx-memory --no-default-features --features vulkan
rtk cargo test -p gfx-memory --no-default-features --features dx12
```

后端 crate 只启用自己的 memory feature：

- `gfx-vulkan` 依赖 `gfx-memory` 的 `features = ["vulkan"]`。
- `gfx-dx12` 依赖 `gfx-memory` 的 `features = ["dx12"]`。
- `gfx-metal` 依赖 `gfx-memory` 的 `features = ["metal"]`。

GPUI 用户必须显式选择具体 nova-gfx 后端：

```toml
gpui = { path = "vendor/gpui", default-features = false, features = ["nova-gfx-vulkan"] }
```

可选 feature 为 `nova-gfx-vulkan`、`nova-gfx-dx12`、`nova-gfx-metal`。旧
`nova-gfx` feature 不再隐式选择后端；这是破坏性编译选择变更，用于避免只用
Vulkan 时仍编译 DX12/Metal 等无关代码。

可以用 dependency tree 校验编译边界：

```powershell
rtk cargo tree -p gfx-vulkan --no-default-features
rtk cargo tree -p gpui --no-default-features --features nova-gfx-vulkan
```

Vulkan tree 不应包含 `gfx-dx12`、`gfx-metal`、D3D12 或 Metal Objective-C
依赖。

## 窗口边界

`gfx-core` 不依赖 `raw-window-handle`。Surface target 通过
`GfxSurfaceDevice` 的后端关联类型表达，由具体后端 crate 决定支持哪种
原生窗口目标。当前 Vulkan、DX12、Metal 后端在自己的 crate 中使用
`raw-window-handle`，因为原生 surface 创建属于平台集成边界，不属于 core
trait 依赖。

## 破坏性 API 变更

与 core trait 重复的后端 public inherent method 不再作为维护 API。调用方
应导入相应的 `gfx_core::Gfx*Device` trait，并通过 trait 方法使用具体后端
device。`draw_and_present` 和 `draw_resources_and_present` 是 core trait
的便利默认方法；生产渲染器通常应直接使用 `draw_steps_and_present`。

## crate 文档

每个库 crate 都有自己的 crates.io README 和 rustdoc 入口：

- `gfx-core`：规范描述符、句柄、错误和 trait。
- `gfx-memory`：后端内存 allocator 封装和 upload-ring 统计。
- `gfx-shader`：WGSL 校验以及 SPIR-V、HLSL、MSL 生成。
- `gfx-vulkan`：core trait 的 Vulkan 实现。
- `gfx-dx12`：core trait 的 Direct3D 12 实现。
- `gfx-metal`：core trait 的 Metal 实现。

英文 README 是 crates.io 默认展示文档；`README.zh-CN.md` 提供中文 API
说明。
