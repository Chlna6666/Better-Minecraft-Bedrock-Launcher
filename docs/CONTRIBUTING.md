# 贡献指南

本文补充变更范围、验证、文档和提交流程。按任务需要打开 [系统架构](ARCHITECTURE.md)、
[命名规范](NAMING.md) 或 [构建与测试](BUILD_AND_TEST.md)，不要求每次改动都通读这些文档。

## 1. 变更范围

涉及跨 crate、公共 API、数据格式、持久化、并发/生命周期或多个用户功能的变更，在
PR/提交说明中交代目标、影响范围（crate、平台、feature）和验证方式。小型局部修复可由
代码差异清楚表达，无需额外撰写形式化范围声明。

跨层改动遵守以下边界：

- nova-gfx 只提供通用图形能力，不依赖 UI 框架、窗口策略或应用语义。
- gpui 只依赖 gfx-core 能力 trait，不按 backend 调用固有方法。
- egpui 不拥有 BMCBL 产品语义。
- bedrock-* 不依赖 UI/GPU 呈现。

## 2. 代码规范

- Edition 2024；unsafe_code = warn；Clippy all/pedantic 为 warn。
- 库代码避免用 unwrap()/expect() 处理可恢复错误；优先使用 ? 和明确错误类型。
- 每个 unsafe 块写 // SAFETY: 注释，说明指针、生命周期和并发前提。
- 公共 API 按需提供 rustdoc、# Errors 和 # Panics；优先窄 trait bound 与合适的参数类型。
- Rust 命名按 [命名规范](NAMING.md) 执行。
- 渲染热路径避免不必要的分配、锁和日志；性能改动需有可重复的测量方法。

## 3. 验证

根据改动和风险选择最小相关检查，不默认运行完整工作区测试：

- 行为变化优先验证受影响逻辑；适用时补充或更新针对性测试。
- 普通 Rust 逻辑按需运行受影响 crate 的 cargo check 或相关测试。
- 涉及多个 backend/feature 时选择相应矩阵；硬件测试保持 #[ignore] 并写明前提，只在
  设备和窗口可用时运行。
- 并发、GPU 资源生命周期、持久化等高风险改动，需要覆盖关键失败/完成路径；静态检查
  不能代替必要的运行时证据。
- 只改文档时使用相关文档检查，不运行 Rust 编译。

具体命令见 [构建与测试](BUILD_AND_TEST.md)。

## 4. 文档

- 中文是项目文档第一语言；crate README.md 为英文，必须提供 README.zh-CN.md。
- 修改带英文镜像的文档时同步更新另一语言。
- 不提交测试输出、benchmark 原始数据、截图、日志、改名历史/迁移映射或带日期计划。
- 架构/边界变化更新 [系统架构](ARCHITECTURE.md) 或新增 ADR；ADR 记录决策，不记录逐条改名。
- 命令、版本、feature 和路径必须与代码一致。

## 5. 提交与变更记录

仓库使用 Conventional Commits（由 cog 校验，见 cog.toml）：

    <type>(<scope>): <subject>

常用 type：feat、fix、refactor、perf、docs、test、chore、ci。

- 提交信息可用中文或英文；同一 PR 保持一致，scope 使用 crate/模块名。
- CHANGELOG.md 由 cog 根据提交生成，不手工编辑历史条目。
- 一个 PR 聚焦一个逻辑变更；格式、重构和行为修改尽量分开。

## 6. 评审检查

- [ ] 变更范围和影响面已清楚表达；没有顺手修改无关模块。
- [ ] 没有新增跨层反向依赖，尤其是 nova-gfx -> gpui/app。
- [ ] 公共 API 命名符合 [命名规范](NAMING.md)，没有无需求的兼容别名。
- [ ] 行为变化有合适的针对性验证；未运行的命令没有写成通过。
- [ ] 文档和代码一致；双语文档已同步；没有违规临时文档。
- [ ] 修改的 Rust 文件格式符合项目要求；实际运行的 cargo check/测试结果已如实记录。
- [ ] 没有提交临时文件、测试数据或调试输出。
