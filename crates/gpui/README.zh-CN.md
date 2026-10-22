# GPUI

[English](README.md)

GPUI 是 Rust 桌面应用的 GPU 加速 UI 框架，混合即时模式与保留模式，提供应用状态、
窗口、Entity 视图、声明式元素、输入分发、平台集成与渲染后端。

本仓库维护的是 BMCBL 使用的独立 fork，pre-1.0，作为 `egpui` 桌面应用框架的 GUI 核心；
它不是 Zed 官方发行版。

## 渲染方向

- 当前渲染路径为 nova-gfx；`RendererBackend` 可在启动时选择 DX11/DX12/Vulkan/OpenGL/Metal。
- Windows 平台使用 winit 事件循环；Linux 支持 X11/Wayland；macOS 仅维护代码路径。
- 合成默认事件驱动，连续渲染只用于显式 `RenderPolicy::Continuous`。
- 自定义 GPU 内容使用场景图元、图片/SVG 元素或 `RendererExtension`；通用 3D 场景与
  viewport 位于 `gpui-3d`。

## 快速开始

```toml
[dependencies]
gpui = { path = "crates/gpui", default-features = false, features = [
    "mimalloc-collect",
    "nova-gfx",
    "nova-gfx-dx12",
] }
```

```rust
use gpui::prelude::*;
use gpui::{div, App, Application, Context, IntoElement, Render, Window};

struct Hello;

impl Render for Hello {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child("Hello from GPUI")
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.open_window(Default::default(), |_, cx| cx.new(|_| Hello))
            .expect("open window");
        cx.activate(true);
    });
}
```

## 核心概念

- `App`：全局上下文，管理窗口、Entity、菜单、键位、资产与平台服务。
- `Context<T>`：在创建、更新、渲染或事件回调中提供的 Entity 上下文。
- `Window`：显式传入需要输入、焦点、绘制、帧请求或平台状态的代码。
- `Entity<T>`：GPUI 管理的状态；通过 `Entity::update` 或 `Context<T>` 更新，状态变化
  后调用 `cx.notify()`。
- `Render` 构建元素树；`RenderOnce` 是渲染时消费的轻量组件。
- `cx.spawn` / `window.spawn` 运行前台异步任务；阻塞或 IO 工作应放到后台执行器。

新代码不要使用旧接口名：`Model<T>`、`View<T>`、`AppContext`、`ModelContext<T>`、
`WindowContext`、`ViewContext<T>`。

## 边界

- GPUI 是通用 UI 核心，不包含 BMCBL 路由、下载、地图等业务。
- 渲染后端通过 `gfx-core` 能力 trait 访问；不得按 backend 调用固有方法。
- 3D 场景准备与 viewport 资源在 `gpui-3d`；GPUI 只负责窗口、输入与 extension 调度。

## 构建与测试

```powershell
cargo check -p gpui --no-default-features --features nova-gfx-dx12
$env:RUST_MIN_STACK = '67108864'
cargo test -p gpui --no-default-features --features nova-gfx-dx12 --lib platform::nova::
```

硬件相关测试默认 `#[ignore]`，需真实窗口/GPU。完整矩阵见
[构建与测试](../../docs/BUILD_AND_TEST.md)。

## 文档

- [工作区文档索引](../../docs/README.md)
- [架构](../../docs/ARCHITECTURE.md)
- [crate 目录](../../docs/CRATES.md)
- [命名规范](../../docs/NAMING.md)
- [gpui / nova-gfx 边界 ADR](../../docs/adr/0001-nova-gfx-gpui-boundary.md)
