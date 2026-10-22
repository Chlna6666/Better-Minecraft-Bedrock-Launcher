# gfx-opengl

[English](README.md)

`gfx-opengl` 是 nova-gfx 的 OpenGL 后端。原生后端要求 desktop OpenGL 4.5 core；
Windows 使用 WGL，Linux 使用 EGL（X11 或 Wayland）。context 与 GPU 资源归渲染线程所有；
Linux display connection 必须比 device 活得更久。

WGSL 在构建期翻译为 GLSL 4.50；驱动在 pipeline 创建时编译/链接 GLSL。资源反射把逻辑
binding 映射到紧凑的 native UBO/SSBO 槽与 combined texture/sampler uniform。clip control
保持 top-left origin 与 0..1 depth。

支持 buffer、BGRA/RGBA/depth texture、mip view、sampler、indexed draw、uniform range、
depth/dual-source blending、GPU fence、readback 与 native window presentation。初始化失败
返回错误，让调用方尝试其他已编译后端。

## 边界

- 纯图形后端；不依赖 UI 框架/应用代码。
- 呈现与 surface 生命周期通过 `gfx-core` trait 暴露。

## 构建与测试

```powershell
cargo check -p gfx-opengl
cargo test -p gfx-opengl -- --ignored --nocapture --test-threads=1
```

## 文档

- [nova-gfx](../README.zh-CN.md)
