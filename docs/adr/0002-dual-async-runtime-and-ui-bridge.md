# ADR-002: 双异步模型、Runtime Provider 与 UI Bridge

- 状态：Accepted
- 日期：2026-07-24
- 决策范围：`crates/egpui`

## 背景

GPUI 的 `cx.spawn` 在 GUI 前台线程轮询 future，其任务生命周期通常跟随 View 或
应用上下文。网络、文件 IO、CPU 计算和持久工作流需要独立调度、并发预算、结构化取消
和可观测的关闭语义，不能把 GUI executor 当作通用应用 runtime。

## 决策

框架明确区分两个执行域：

| 执行域 | 所有者 | 允许的数据 | 生命周期 |
| --- | --- | --- | --- |
| GUI 前台 | `gpui` | `App`、`Window`、`Entity`、`Context<T>` | GUI/窗口 |
| 应用后台 | `egpui::ApplicationRuntime` | `Send + 'static` 纯数据 | application/task scope |

具体规则：

1. `RuntimeProvider` 隔离物理运行时；默认 provider 使用一个 Tokio IO runtime 和
   一个 Rayon CPU pool。
2. `TaskScope` 提供父子取消域；`AppTask<T>` 明确返回 completed、cancelled 或
   failed 终态。
3. 阻塞任务和 CPU 任务只能协作取消。调用方取消等待不表示底层同步操作已经停止。
4. `UiHandle` 使用有界通道把 `Send + 'static` 闭包投递到 GPUI 前台；后台线程
   永远不能持有 `AsyncApp`。
5. `UiStreamBridge<T>` 对连续纯数据使用有界队列，队列满时显式返回 backpressure
   错误，不静默丢弃。
6. 应用退出先停止接收新后台任务并取消 scope；GPUI quit callback 不阻塞等待。
   `Application::run` 返回后，host 在配置的 deadline 内等待 Tokio/Rayon 收敛。

## 结果

- GUI 动画和输入调度不会承担持久业务任务。
- 窗口关闭不会自动取消 application scope。
- 后台到 GUI 的所有状态传播都有显式边界和背压。
- Tokio/Rayon 是默认实现而不是 `gpui` 的公开依赖或不可替换契约。
