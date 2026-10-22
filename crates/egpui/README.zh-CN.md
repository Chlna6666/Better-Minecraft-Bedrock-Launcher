# egpui

[English](README.md)

`egpui` 是建立在独立维护的 `gpui` GUI 核心之上的桌面应用层。

它提供：

- `ApplicationHost` 生命周期、headless 模式、服务注册与基于 deadline 的关闭；
- 应用持有的 Tokio/Rayon 运行时（`RuntimeProvider`）；
- `TaskScope` 结构化取消与显式 `TaskOutcome` 终态；
- 有界 `UiHandle`、`UiStreamBridge<T>` 与合并快照通道，把后台结果送回 GPUI 前台；
- `UiTaskBridge`：高频进度合并 + 一次无损终态更新；
- Fluent i18n 目录、BCP 47 fallback、复数、RTL 元数据与 locale 快照；
- 通用持久任务记录与原子 checkpoint；
- `DurableWorkflowCoordinator`：handler 注册、重启恢复与终态持久化。

## 边界

- 不包含 BMCBL 路由、下载/归档语义或任务管理策略；这些留在应用中。
- i18n 层与业务无关：消息 key、目录文件与文案由应用持有。
- GPUI 前台工作仍使用 GPUI 的 `App`/`AsyncApp` 执行域；`egpui` 不要求调用方构造或检测
  Tokio runtime。
- 下载、解压等大任务通过 `TaskScope` 调度，`UiTaskBridge` 只传进度/终态，不执行 IO。

## 构建与测试

```powershell
cargo check -p egpui
cargo test -p egpui
```

## 文档

- [架构](../../docs/ARCHITECTURE.md)
- [crate 目录](../../docs/CRATES.md)
- [English README](README.md)
