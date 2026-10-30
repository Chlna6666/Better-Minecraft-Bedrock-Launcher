# gfx-opengl

图形 pass 跳过不变的 uniform/storage 缓冲范围绑定、纹理与采样器。缓存存储在 pass 间复用，但每次 pass
开始都会清空条目；必要的镜像复制仍在每个 draw 前执行。呈现通过 `PresentTimings` 返回主机
命令、swap、信号和收尾耗时。

[English](README.md)

统一缓冲计算接口复用计算 program，并在后续读取前设置内存屏障。仅支持
`GL_ARB_shader_draw_parameters` 时声明非索引间接绘制能力，以保持统一的 first-instance
语义。显式等待队列完成使用 `glFinish`；普通计算派发与绘制提交不等待 GPU 完成。

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
