# AGENTS.md

本文件给在此仓库工作的 AI/自动化代理与开发者提供最短、可验证的工作约束。更完整的说明见
[docs/README.md](docs/README.md)、[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)、
[docs/BUILD_AND_TEST.md](docs/BUILD_AND_TEST.md) 与 [docs/NAMING.md](docs/NAMING.md)。

## 1. 项目是什么

BMCBL 是用 Rust + GPUI 编写的 Minecraft Bedrock 原生桌面启动器。主平台 Windows；Linux 处于
测试阶段；macOS 不支持。UI、业务、下载、配置、插件和 Bedrock 解析都在本工作区中实现。

## 2. 先读文档，再改代码

按任务类型选择：

- 结构与 crate 职责：[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)、
  [docs/CRATES.md](docs/CRATES.md)
- 构建、feature、测试矩阵：[docs/BUILD_AND_TEST.md](docs/BUILD_AND_TEST.md)
- 命名规范：[docs/NAMING.md](docs/NAMING.md)
- 变更流程与提交规范：[docs/CONTRIBUTING.md](docs/CONTRIBUTING.md)

## 3. 硬性边界

- **不可偏离的分层**：`nova-gfx`（`gfx-core`、`gfx-*`）是类似 wgpu 的轻量、通用图形 API；
  `gpui-3d` 是可用于游戏的专业 3D 渲染库；`gpui` 是**可选的 UI 侧载消费者**，
  不是 3D 命令编码器、3D 场景调度器或图形驱动。
- `nova-gfx`/`gfx-*` 只定义通用 Device/Buffer/Texture/3D Texture/Pipeline/BindGroup、
  Graphics/Compute/Transfer、ResourceBarrier、CommandEncoder、Queue、Submission/Fence、
  时间戳与设备限制；不得依赖 `gpui`、`gpui-3d`、`egpui`、应用代码，也不得引入
  地图/方块/材质/光照/场景策略和 UI 的 damage/retained/atlas/glyph/blur 概念。
  只允许平台 backend 实现通用图形语义，不允许为 BMCBL 特化公开 API。
- `gfx-core` 不依赖 `winit`/`raw-window-handle`；native surface 由 backend 的
  `SurfaceDevice::SurfaceTarget` 承担。
- `gpui-3d` **拥有游戏级 Scene、GPU Resident 数据、剔除、构网、材质/光照、RenderGraph、
  DrawIndirect、原生离屏 Pass 调度及其资源生命周期策略**。纯 3D 引擎模块只依赖
  `gfx-core` 和无 UI 的库；不得引用 `gpui::`、`RendererExtension*`、UI 布局和 BMCBL
  业务代码。当前 crate 尚包含 GPUI 适配代码，应持续将其限定在
  `scene_view` 等 sidecar/adapter 模块，并由默认启用的 `gpui-sidecar` Cargo feature
  控制；`cargo check -p gpui-3d --no-default-features --lib` 应不再引入 `gpui`。
  不能以已有历史依赖为由继续增加核心耦合。
- `gpui` 负责窗口/swapchain、输入/布局、GPU Owner 设备访问协调、纹理 Sidecar 消费、
  Present 与 Fence 生命周期跟踪。不得接收后重新编码、排序或合并 3D 的 DrawIndirect；
  只允许在设备所有者线程调用通用 `nova-gfx` 原生提交入口，并跟踪 `SubmissionId`。
  3D producer 与 GUI consumer 必须保持 GPU 侧同步，不在热路径插入 CPU Fence wait。
- `gpui` 只依赖 `gfx-core` 能力 trait；不得按 backend 调用固有 resize/present/alpha 方法。
- 各层 API 命名必须通用且基于所有权：`nova-gfx` 使用资源/提交语义；
  `gpui-3d` 使用场景/渲染语义；GPUI 只使用 Sidecar/Consumer 语义。
  不得通过重命名私有函数伪造公共库能力，不得把公共 API 限定成 Minecraft 专用功能。
- `egpui` 不拥有 BMCBL 路由、下载、地图等产品语义。
- `bedrock-*` 不依赖 UI/GPU 呈现；`bedrock-voxel` 只产出计划，不直接写世界。
- 不要修改 `crates/easytier`、`crates/raknet`、`vendor/*`（外部/ vendored 代码），除非任务
  明确要求并在 PR 中说明上游版本与补丁原因。

## 3.1 3D 引擎和 UI Sidecar 的验收规则

- 修改 3D 提交链路时，列出两条独立路径：
  `gpui-3d → nova-gfx → GPU`（场景/Compute/Indirect/原生离屏生产）和
  `gpui-3d → GPUI → Present`（纹理消费/窗口/UI）。严禁让 GPUI 2D 合成器成为
  3D Pass 的实际命令编码器；一张最终 3D 结果纹理的 UI 合成不等于“伪 3D”。
- `GpuWorkPlan` 的依赖验证不等于原生 Barrier/状态转换；只有后端确实编码同步并能保证
  提交顺序、Fence 退休和资源可见性，才能宣称支持完整 Graphics→Compute→Graphics。
- 侧载颜色/深度纹理、BindGroup/Placement、Pipeline 的释放必须依据实际
  UI Consumer `SubmissionId` 的 `gpu_submission_completed` 证明，或 GPU Owner
  全局完成证明；`gpu_submission_accepted` 不代表 GPU 已执行完毕。
  GPU Resident Mesh/Draw Slot 仍被多个提交引用时不得因单次 Fence 完成提前回收。
- 上传、Compute Meshing、DrawIndirect 的**原生 GPU 时间戳**必须与
  `sidecar_cpu_prepared`、`gpu_owner_phase`、`offscreen_native_submit`、
  `native_bridge_cpu`、`gpu_owner_submission_wait`、Present 等主机耗时分开记录；
  不把 CPU API 延迟称作 GPU 执行时间，不臆造 FPS 收益。
- 不以降低 3D 质量、减少正确的 Shadow Pass、降帧或每帧 CPU 同步换取假加速；
  多 SceneView、窗口尺寸变化、挂起/恢复、GPU 设备丢失及在途资源复用必须有失败路径。
- 破坏性开发允许，但公共 API 变更必须处理所有调用方和四后端的能力回退；
  未实际执行的编译/硬件测试不能标记为通过。

## 4. 修改流程

1. 先声明 IN/OUT SCOPE、受影响 crate、平台与 feature。
2. 找到最小改动点；不要顺手重构无关模块。
3. 公共 API 变更先确认边界，再更新所有调用方、rustdoc 和文档。
4. 新行为必须有测试；硬件相关测试保持 `#[ignore]` 并注明前提。
5. 文档中英文成对更新；不提交测试输出、benchmark 原始数据、改名历史、带日期计划。

## 5. 验证命令

最少验证：

```powershell
rustfmt --edition 2024 <本次改动文件...>
cargo check -p <受影响 crate>
cargo test -p <受影响 crate>
pwsh -File scripts/check-docs.ps1
```

常用矩阵：

```powershell
cargo check -p gfx-core -p gfx-dx11 -p gfx-dx12 -p gfx-vulkan -p gfx-opengl -p gfx-metal -p gfx-webgl
cargo check -p gpui --no-default-features --features "nova-gfx-dx11 nova-gfx-dx12 nova-gfx-vulkan nova-gfx-opengl"
cargo check -p gpui-3d --tests
cargo check -p bmcbl
$env:RUST_MIN_STACK = '67108864'
cargo test -p gpui --no-default-features --features nova-gfx-dx12 --lib platform::nova::
pwsh -File scripts/check-nova-gfx-boundary.ps1
pwsh -File scripts/check-gpui-3d-boundary.ps1
pwsh -File scripts/check-naming.ps1
```

硬件 gate（需要 GPU/窗口）：

```powershell
cargo test -p gfx-dx11 -- --ignored --nocapture --test-threads=1
cargo test -p gfx-vulkan -- --ignored --nocapture --test-threads=1
```

不要把未实际运行的命令写成“已验证”；不要编造性能数字。

## 6. 代码与命名

- Edition 2024；库代码避免 `unwrap()`/`expect()` 处理可恢复错误。
- 每个 `unsafe` 块必须有 `// SAFETY:` 说明。
- 命名遵循 [docs/NAMING.md](docs/NAMING.md)：动词 + 对象、`is_/has_/can_` 谓词，避免
  `_compat`/`_legacy`/长句链。
- 图形术语以 wgpu 为准：`BindGroup`、`RenderCommand`、`RenderPassCommands`、
  `PresentFrameDescriptor`、`SurfaceCapabilities`。

## 7. 文档规则

- 中文是项目文档第一语言；crate `README.md` 为英文，必须有 `README.zh-CN.md`。
- `docs/` 只放稳定知识：架构、构建/测试、贡献、命名、ADR、crate 目录。
- 禁止新增：测试输出/benchmark 数据/截图/日志、改名历史/迁移映射、带日期的计划或草稿。
- 文档中的命令、版本、feature、路径必须与当前代码一致；修改后运行
  `pwsh -File scripts/check-docs.ps1`。

## 8. Git 与发布

- 未经用户明确要求，不要执行 `git push`、发布或部署操作。
- 提交遵循 Conventional Commits（`cog` 校验）；`CHANGELOG.md` 自动生成，不手工编辑历史。
- 一个 PR 只做一个逻辑变更；格式化、重命名、行为变更尽量分开。
