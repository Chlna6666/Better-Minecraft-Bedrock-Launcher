# gfx-metal

[English](README.md)

`gfx-metal` 是 `gfx-core` 设备 trait 的 Metal 实现。Apple 目标上创建 Metal device、资源、
pipeline、command encoder 与 `CAMetalLayer` swapchain；非 Apple 目标以 `Error::Unavailable`
stub 实现 trait，保证非目标构建可编译。

离屏 `render_to_texture` 当前返回 `Error::Unavailable`；在实现 Metal 离屏路径前保持显式
失败，不做静默回退。

## 边界

- 纯图形后端；不依赖 UI 框架/应用代码。
- Windows 上只编译 stub；Apple 侧需单独构建验证。

## 构建与测试

```powershell
cargo check -p gfx-metal
```

## 文档

- [nova-gfx](../README.zh-CN.md)
- [构建与测试](../../../docs/BUILD_AND_TEST.md)
