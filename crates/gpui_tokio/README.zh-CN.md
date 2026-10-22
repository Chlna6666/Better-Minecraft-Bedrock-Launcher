# gpui_tokio

[English](README.md)

`gpui_tokio` 负责 Tokio 与 GPUI 的集成：把 Tokio runtime handle 安装到 GPUI `App`，
并通过 `TokioContext` 暴露访问器，让前台/后台任务安全使用 Tokio 而不阻塞 UI 线程。

## 公共接口

- `init(cx)`：创建并安装默认 runtime；
- `init_from_handle(cx, handle)`：安装调用方持有的 runtime；
- `Tokio` / `TokioContext`：任务辅助代码使用的 runtime 访问器。

## 边界

- 禁止在 render 路径阻塞；长任务放到 Tokio runtime 或后台执行器，结果再送回 GPUI。
- runtime 所有权属于应用（BMCBL 由 `egpui` 持有）。

## 构建与测试

```powershell
cargo check -p gpui_tokio
cargo test -p gpui_tokio
```

## 文档

- [架构](../../docs/ARCHITECTURE.md)
- [构建与测试](../../docs/BUILD_AND_TEST.md)
