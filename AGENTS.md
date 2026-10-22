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
- gpui / nova-gfx 边界：[docs/adr/0001-nova-gfx-gpui-boundary.md](docs/adr/0001-nova-gfx-gpui-boundary.md)
- 变更流程与提交规范：[docs/CONTRIBUTING.md](docs/CONTRIBUTING.md)

## 3. 硬性边界

- `nova-gfx`/`gfx-*` 是纯图形库：不得依赖 `gpui`、`egpui`、应用代码，也不得出现 UI 概念
  （damage/retained/atlas/glyph/blur/scene）。
- `gfx-core` 不依赖 `winit`/`raw-window-handle`；native surface 由 backend 的
  `SurfaceDevice::SurfaceTarget` 承担。
- `gpui` 只依赖 `gfx-core` 能力 trait；不得按 backend 调用固有 resize/present/alpha 方法。
- `gpui-3d` 依赖 `gpui` + `gfx-core`，不拥有窗口、swapchain 或后端。
- `egpui` 不拥有 BMCBL 路由、下载、地图等产品语义。
- `bedrock-*` 不依赖 UI/GPU 呈现；`bedrock-voxel` 只产出计划，不直接写世界。
- 不要修改 `crates/easytier`、`crates/raknet`、`vendor/*`（外部/ vendored 代码），除非任务
  明确要求并在 PR 中说明上游版本与补丁原因。

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
