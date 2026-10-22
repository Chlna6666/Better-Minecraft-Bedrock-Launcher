# gpui-macros

[English](README.md)

`gpui-macros` 是 BMCBL 维护的 GPUI 过程宏镜像（`IntoElement`、`Render`、Action、
App/Visual Context、inspector reflection 与基准 harness）。由 `gpui` 使用，应用不应直接依赖。

## 边界

- 只包含宏实现；运行时行为在 `gpui`。
- 行为与 `Cargo.toml` 记录的上游 revision 保持一致。

## 构建与测试

```powershell
cargo check -p gpui-macros
cargo test -p gpui
```

## 文档

- [gpui](../gpui/README.zh-CN.md)
- [crate 目录](../../docs/CRATES.md)
