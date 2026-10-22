# gfx-webgl

[English](README.md)

`gfx-webgl` 是 nova-gfx 的 WebGL 后端占位实现：暴露 backend API surface，但在 WebGL
集成完成前把后端报告为不可用（`Error::Unavailable`）。

## 状态

Stub。不要把它作为生产后端；需要浏览器目标时再补齐实现与测试。

## 构建与测试

```powershell
cargo check -p gfx-webgl
```

## 文档

- [nova-gfx](../README.zh-CN.md)
- [crate 目录](../../../docs/CRATES.md)
