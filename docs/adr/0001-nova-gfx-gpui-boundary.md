# ADR 0001: nova-gfx 与 gpui 的模块边界

- 状态: 已接受
- 日期: 2026-10-21
- 范围: `crates/nova-gfx`、`crates/gpui`、`crates/gpui-3d`
- 参考: [wgpu](https://wgpu.rs/) / [gfx-rs/wgpu](https://github.com/gfx-rs/wgpu)

## 背景

`nova-gfx` 是图形封装库，`gpui` 是 UI 框架，`gpui-3d` 是建立在两者之上的 3D 渲染扩展。
重构前存在以下边界问题：

1. `gfx-core` 内含只服务 GPUI 的兼容层：`Backend*` 别名 trait、`*_compat`、
   `render_step_list_*`、`*_deferred_*`、`*_measured` 等 API。
2. UI 渲染器抽象混入图形库：`ExtensionDevice`（renderer extension 的 `dyn` 资源接口）
   以及以 draw step 为中心的命名。
3. 反向依赖穿透：gpui 直接调用 backend 固有方法完成 resize、swapchain 重建、alpha 解析、
   frame-ready 查询，图形库没有提供完整契约。
4. GPUI 示例、README、日志 target 出现在 nova-gfx。
5. 呈现 API 参数过多、存在布尔模式参数和长句方法名。

这些问题的本质是：**UI 帧策略（damage、retained layer、frame scheduling、deferred
tracking、blur/atlas 决策）与图形 API 混在同一层**。

## 决策

### 1. 分层契约

| 层 | 职责 | 禁止 |
| --- | --- | --- |
| `gfx-core` | 后端无关描述符、句柄、错误、能力 trait、surface/present 原语 | 引用 UI 框架、窗口事件、damage/retained/atlas 概念 |
| `gfx-<backend>` | 原生 API 实现、acquire/present 同步、backend 诊断 | 引用 gpui；暴露与 core trait 重复的固有 API |
| `gpui/nova` | 帧策略、Scene/DirtyRegion、retained layers、atlas、blur、呈现调度 | 直接调用 backend 固有方法；把 UI 概念写进 gfx-core |
| `gpui-3d` | 基于 gpui + gfx-core 的 3D 场景与扩展 | 修改 nova-gfx 契约 |

依赖方向固定为 `gpui -> nova-gfx -> gfx-core`，禁止反向。`gfx-core` 不依赖 `winit`/
`raw-window-handle`/任何 UI crate；native surface target 由各 backend 的
`SurfaceDevice::SurfaceTarget` 承担。

### 2. 呈现 API

`PresentationDevice` 只保留与 UI 无关的帧原语：

- `render_and_present(&PresentFrameDescriptor)`：一帧 record + submit + present，返回
  可选 `PresentedFrame`。
- `render_to_textures(&[RenderPassCommands])`：有序离屏 pass，可返回 deferred submission。
- `request_frame_ready`、`can_present_now`、`surface_capabilities`、`set_content_stretch`：
  surface 呈现设施，不包含调度策略。

`SurfaceDevice` 补齐 surface 生命周期：`resize_swapchain`、`recreate_swapchain`、
`swapchain_extent`、`resolve_composite_alpha`。gpui 不再需要知道具体 backend 的固有方法。

### 3. 所有权

- `ExtensionDevice` 属于 gpui（renderer extension 的宿主边界），不进入 gfx-core。
- atlas、blur、path mask、retained 资源、damage 合并、提交跟踪归 gpui 的 nova 渲染器。
- acquire/present 同步、swapchain 状态与 backend 诊断归各 gfx backend。

### 4. 命名

图形词汇与 wgpu 对齐：`BindGroup`/`BindGroupLayout`、`RenderCommand`、`RenderPassCommands`、
`PresentFrameDescriptor`、`SurfaceCapabilities` 等；详细规则见 [NAMING](../NAMING.md)。

## 影响

- 破坏性变更：旧兼容别名 trait、draw-step 呈现路径、`*_compat`/`*_measured` 变体被删除；
  呈现与 surface 生命周期收敛到 descriptor/trait；旧调用点已在 gpui 与 gpui-3d 中迁移。
- `ExtensionDevice` 在 gpui 中重新导出，路径保持稳定；gpui-3d 通过 `gpui::ExtensionDevice`
  使用。
- 图形库不再包含 GPUI 示例、文档或日志 target；示例迁移到 gpui 或通用 nova 示例。

## 备选方案

- **保留兼容层**：会让 UI 策略长期留在图形库，双 API 维护成本高，否决。
- **把呈现编排全部移入 gpui**：需要 gpui 直接管理每个 backend 的 swapchain/sync，破坏
  backend 封装，否决；改为由 trait 提供通用原语。
- **一次性重命名为 wgpu 全量 API**：超出边界问题范围，采纳渐进式命名（公共 API 先对齐，
  内部命名随模块重构收敛）。

## 后续

- 新增图形能力先进入 `gfx-core` trait，再由 backend 实现；不得在 gpui 中按 backend 分支。
- 新增文档遵循 [docs/README](../README.md) 与 [NAMING](../NAMING.md)。
