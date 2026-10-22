# gfx-memory

[English](README.md)

`gfx-memory` 提供 `nova-gfx` 后端使用的 GPU 内存分配封装：包装 backend allocator、
暴露分配统计，并提供资源上传路径使用的小型 upload-ring allocator。

它不拥有 buffer/image/texture 等图形资源；backend 拥有资源并把 allocation 绑定到资源。

## 模块

- 通用 allocation 描述、错误与统计；
- upload-ring staging 分配；
- 按已完成 GPU fence 值回收的 deferred-free 队列；
- 由 Cargo feature 选择的 backend allocator 包装。

## Feature

`default = []`，按需启用 `vulkan`、`dx12`、`metal`。

## 构建与测试

```powershell
cargo test -p gfx-memory --no-default-features
cargo test -p gfx-memory --no-default-features --features vulkan
cargo test -p gfx-memory --no-default-features --features dx12
```

## 文档

- [nova-gfx](../README.zh-CN.md)
- [crate 目录](../../../docs/CRATES.md)
