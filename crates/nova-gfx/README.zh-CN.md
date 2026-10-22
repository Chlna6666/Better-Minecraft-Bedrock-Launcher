# nova-gfx

[English](README.md)

`nova-gfx` 是 BMCBL 的轻量图形基础库，不是 UI 库。`gfx-core` 是唯一公共 API 契约：
后端无关描述符、类型化句柄、错误与设备能力 trait；各 backend crate 为 Vulkan、
Direct3D 11/12、OpenGL 4.5 与 Metal 实现这些 trait。

## crate

- `gfx-core`：后端无关类型与能力 trait；不依赖 UI/窗口句柄。
- `gfx-memory`：GPU 分配封装、上传环、延迟释放与统计。
- `gfx-shader` / `gfx-shader-build`：WGSL 校验与 SPIR-V/HLSL/MSL/GLSL 生成/构建期编译。
- `gfx-dx11` / `gfx-dx12` / `gfx-vulkan` / `gfx-opengl` / `gfx-metal` / `gfx-webgl`：各后端。
- `examples/*`：triangle / atlas / public-api smoke 基线，不是库 API。

## 公共 API

按需使用最窄的 trait：

- `SurfaceDevice`：surface/swapchain 与 surface 生命周期；
- `ResourceDevice`：buffer/texture/view/sampler/bind group；
- `PipelineDevice`：shader、render pass layout、pipeline layout、pipeline；
- `CommandDevice`：显式 command encoder；
- `SubmissionDevice`：deferred submission 与完成跟踪；
- `PresentationDevice`：`render_and_present`、`render_to_textures`、surface readiness 与呈现能力；
- `DiagnosticsDevice`：资源与内存诊断。

`Device` 组合完整契约；辅助函数应尽量使用窄 trait。

## 边界

nova-gfx 是图形封装库，不处理 UI。帧策略（damage、retained layer、帧调度、
partial present 决策、GPU 工作跟踪）属于消费方渲染器。`gfx-core` 只包含图形词汇：
device、queue、resource、pipeline、command、surface 与 presentation primitives；
不得引用 UI 框架、widget 树或渲染器 damage 模型。

## Feature 选择

`gfx-memory` 默认无后端 feature；`gfx-vulkan`/`gfx-dx12`/`gfx-metal` 各自启用所需 memory
feature。消费方显式选择 backend crate，不存在“通用 nova-gfx feature”。

## 文档

- [工作区架构](../../docs/ARCHITECTURE.md)
- [crate 目录](../../docs/CRATES.md)
- [构建与测试](../../docs/BUILD_AND_TEST.md)
