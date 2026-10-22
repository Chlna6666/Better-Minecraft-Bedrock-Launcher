# 贡献指南

本文面向修改 BMCBL 工作区代码与文档的开发者。先读 [ARCHITECTURE](ARCHITECTURE.md)、
[NAMING](NAMING.md) 和 [ADR 0001](adr/0001-nova-gfx-gpui-boundary.md)，再开始改动。

## 1. 变更前：声明范围

每个变更在 PR/提交说明中写清楚：

- **IN SCOPE**：要改的模块、文件和公共接口；
- **OUT OF SCOPE**：明确不改的部分；
- **影响面**：受影响的 crate、feature、平台与用户功能；
- **验证方式**：实际执行的命令与结果。

跨层变更（应用/框架/UI 核心/图形/领域）需要先确认边界：

- `nova-gfx` 只做图形封装，不得引入 UI 框架、窗口策略、damage/retained/atlas 等概念。
- `gpui` 只依赖 `gfx-core` 能力 trait，不按 backend 调固有方法。
- `egpui` 不拥有 BMCBL 业务语义。
- `bedrock-*` 不依赖 UI/GPU 呈现。

## 2. 代码规范

- Edition 2024；`unsafe_code = "warn"`；Clippy `all`/`pedantic` 为 warn。
- 库代码避免 `unwrap()`/`expect()` 处理可恢复错误，使用 `?` 和明确错误类型。
- 每个 `unsafe` 块必须有 `// SAFETY:` 注释，说明指针/生命周期/并发前提。
- 公共 API 必须有 rustdoc 与 `# Errors`/`# Panics`；优先窄 trait bound 和参数对象。
- 命名遵循 [NAMING](NAMING.md)：动词 + 对象、谓词 `is_/has_/can_`、禁止 `_compat`/
  `_legacy`/长句链接。
- 渲染热路径避免分配、锁和日志刷屏；性能改动需要可重复的测量方法。

## 3. 测试要求

- 修改行为必须新增/更新测试；优先单元测试，硬件相关测试保持 `#[ignore]` 并标注前提。
- 至少运行与改动 crate 对应的 `cargo check`；涉及多后端时运行 feature 矩阵。
- 修改 `gfx-*` 时运行该 backend 的测试（含硬件 gate 如可用）。
- 修改 `gpui`/`gpui-3d` 时至少运行 `platform::nova::` 或对应模块测试。
- 修改文档时运行 `pwsh -File scripts/check-docs.ps1`。

完整命令见 [BUILD_AND_TEST](BUILD_AND_TEST.md)。

## 4. 文档规范

- 中文是项目文档第一语言；crate `README.md` 为英文，必须有 `README.zh-CN.md`。
- 修改双语文档时必须同步两种语言。
- 不提交：测试输出/benchmark 原始数据/截图/日志、改名历史/迁移映射、带日期的计划或草稿。
- 架构/边界变化先更新 [ARCHITECTURE](ARCHITECTURE.md) 或新增 ADR；
  ADR 只记录决策，不记录逐条改名。
- 文档中的命令、版本、feature、路径必须与当前代码一致。

## 5. 提交与变更记录

仓库使用 Conventional Commits（由 `cog` 校验，见 `cog.toml`）：

```text
<type>(<scope>): <subject>
```

常用 type：`feat`、`fix`、`refactor`、`perf`、`docs`、`test`、`chore`、`ci`。

- 提交信息用中文或英文均可，但同一 PR 保持一致；scope 使用 crate/模块名。
- `CHANGELOG.md` 由 `cog` 根据提交生成；不要手工编辑历史条目。
- 一个 PR 只做一个逻辑变更；格式、重构、行为变更尽量分开。

## 6. 评审清单

- [ ] 范围与影响面已在 PR 中说明；没有顺手修改无关模块。
- [ ] 没有新增跨层反向依赖（尤其 `nova-gfx -> gpui/app`）。
- [ ] 公共 API 命名符合 [NAMING](NAMING.md)，没有新增兼容别名。
- [ ] 新行为有测试；硬件 gate 的前提已说明。
- [ ] 文档与代码一致；中英文同步；没有违规文档类型。
- [ ] 改动文件已 `rustfmt`，相关 `cargo check`/测试命令通过。
- [ ] 没有提交临时文件、测试数据或调试输出。
