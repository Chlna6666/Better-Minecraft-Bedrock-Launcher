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
  扩展可申请 GPU 完成回调；中等内存整理时，宿主仅在 DX11、DX12 或 Vulkan 确认图形队列完成后通知。
  `ExtensionDevice` 也提供可选的有序缓冲计算；扩展需先查询能力，再派发计算或追加 GPU 生成的间接绘制命令。

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

## Debug 热重载

可选 `hot-reload` feature 使用 Subsecond 0.7.10。安装匹配的 Dioxus CLI（`dx` 0.7.10）后，
在仓库根目录启动 BMCBL：

```powershell
dx serve --platform desktop --renderer native --bin BMCBL --features hot-reload --hot-patch
```

Entity 视图、类型擦除的 `AnyView` 和 `RenderOnce` 组件在下一次刷新时进入补丁后的渲染函数，
保留 Entity 状态；应用补丁会使窗口保留的渲染结果失效。自定义 `Element` 生命周期方法、
持久状态内存布局的变化可能仍需重启。运行时仅在 Windows、Linux、macOS 的 Debug 构建中启用，
默认关闭。

## 显示器跟踪

`App::observe_displays` 报告显示器连接、断开和属性变化，包括边界与标称刷新间隔。
`Window::refresh_interval(cx)` 读取 App 缓存；`Window::observe_window_display` 及对应的
`Context` 接口触发时，显示器元数据已更新。原生跨屏通知会延迟并合并处理，允许它们在窗口
更新期间到达。可选 profiler 读取该缓存，避免每帧查询显示器。

Windows 即使没有 GPUI 窗口，也会保留一个隐藏的顶层通知窗口；进程内显示器 ID 不随枚举顺序
变化。Wayland 跟踪当前输出模式、后续模式更新和输出移除。macOS 使用屏幕参数通知，并在
可用时读取 ProMotion 的最高刷新率。X11 仍报告 X screen root，不提供标称刷新间隔。

`window_metrics_snapshot` 提供由时间相关渲染扩展驱动、成功提交的合成帧各阶段宿主耗时 p95。
场景准备包含扩展准备；提交预备、保留资源准备、帧上传与后端呈现单独统计。
逐帧“其他”值覆盖上述互不重叠阶段之外的工作。这些数值不代表 GPU 执行时间或屏幕扫描显示时间。

## 核心概念

- `App`：全局上下文，管理窗口、Entity、菜单、键位、资产与平台服务。
- `Context<T>`：在创建、更新、渲染或事件回调中提供的 Entity 上下文。
- `Window`：显式传入需要输入、焦点、绘制、帧请求或平台状态的代码。
- `Entity<T>`：GPUI 管理的状态；通过 `Entity::update` 或 `Context<T>` 更新，状态变化
  后调用 `cx.notify()`。
- `Render` 构建元素树；`RenderOnce` 是渲染时消费的轻量组件。
- 布局使用 Taffy 0.15。每个窗口通过 `Rc` 共享 `WindowTextSystem`，窗口布局缓存和字体 run
  临时缓冲只在前台线程使用；应用级 `TextSystem` 保留后台字体工作需要的同步。
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
