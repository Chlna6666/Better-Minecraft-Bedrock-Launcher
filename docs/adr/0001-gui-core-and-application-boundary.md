# ADR-001: GUI 核心与应用框架边界

- 状态：Accepted
- 日期：2026-07-24
- 决策范围：`crates/gpui`、`crates/egpui`

## 背景

GPUI 提供窗口、元素、输入、渲染、Entity 和 GUI 前台执行器，但不应拥有桌面应用的
业务生命周期、物理异步运行时、资源清单、i18n、打包或更新策略。把这些能力直接加入
GUI 核心会导致平台层、应用层和业务层相互依赖，也会使上游 GPUI 同步难以审计。

## 决策

采用单向分层：

1. `gpui` 是 GUI 核心，只负责窗口、渲染、输入、Entity 和 GUI 单线程执行域。
2. `egpui` 是桌面应用框架，负责 `ApplicationHost`、应用生命周期、服务注册、
   应用异步运行时、关闭收敛以及后续的 manifest、资源、i18n 和平台集成。
3. 具体应用依赖 `egpui`，应用领域服务仍归应用所有。
4. 依赖方向固定为 `application -> egpui -> gpui`；`gpui` 不得反向依赖
   `egpui` 或 BMCBL。
5. Zed/GPUI 的版权、Apache-2.0 许可和上游来源继续由 `crates/gpui/NOTICE`、
   `UPSTREAM.md` 和 crate metadata 记录。

## 结果

- GPUI 上游同步可以独立审计。
- 桌面应用能力可按独立 crate 和 SemVer 演进。
- BMCBL 的下载、归档、Minecraft、页面和资源默认值不进入通用框架。
- 新框架能力必须首先证明与 BMCBL 领域无关，才能从应用层迁移。
