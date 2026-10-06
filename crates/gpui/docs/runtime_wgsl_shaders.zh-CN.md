# 运行时 WGSL Shader

[English](runtime_wgsl_shaders.md)

GPUI 会在构建时校验并嵌入内置渲染器 WGSL。拥有自定义 Nova GPU 渲染的应用和示例也可
以在运行时加载并校验 WGSL，然后再创建 shader module。

运行时 shader 加载适用于模型查看器、可视化工具、游戏视图、自定义材质系统，以
及其他需要 GPUI 内置元素之外 shader 代码的功能。

## 加载 WGSL

如果需要保留经过验证的 shader source，使用 `WgslShaderSource`：

```rust
let source = gpui::WgslShaderSource::from_path("examples/viewer.wgsl")?;
```

通过关联构造器加载文件：

```rust
let shader = gpui::WgslShaderSource::from_path("examples/viewer.wgsl")?;
```

生成的或内嵌的 shader 字符串可以使用 source label：

```rust
let shader = gpui::WgslShaderSource::from_source(
    "generated-material-shader",
    generated_wgsl,
)?;
```

loader 会在应用渲染代码创建后端 shader module 前用 `naga` 校验 WGSL。文件读取错
误会包含路径；解析和校验错误会包含传入的 label 或路径，以及格式化后的 WGSL 诊断
信息。

## 接入 Renderer Extension

运行时 WGSL 属于实现自定义 GPU 绘制的 crate。GPUI `RendererExtension` 可以在 extension
device 上创建 pipeline 和 buffer，再向宿主 render pass 添加 draw step：

1. 使用 `WgslShaderSource` 加载并校验 WGSL。
2. 按选中的后端 cross-compile 或转换 shader。
3. 在 extension renderer 中构建 bind groups、pipelines、buffers 和 textures。
4. 从 `RendererExtensionRenderer::render` 返回有序的 `RenderStepDescriptor`。
5. GPUI 应用元素 scissor，并在 extension 的 scene 顺序位置提交这些步骤。

pipeline 的 color target 必须匹配 `RendererExtensionContext::color_format`。extension callback
在 renderer owner 上同步执行；不得执行阻塞 IO 或回调应用 UI 状态。GPUI 拥有窗口和 render
pass 生命周期，extension 实现拥有自己的 GPU 资源。

## 错误处理

把 shader 加载视为可能失败的应用初始化：

- 文件系统错误应带上 source path 后返回或显示。
- parse 与 validation diagnostic 应反馈给用户或开发日志。
- shader 或 surface format 改变时，重建依赖的 pipeline。
- 除非 shader 属于框架渲染器，否则不要把运行时 shader 错误放进 GPUI renderer
  internals。

## 3D 示例

旧 `hatsune_miku_viewer` 示例依赖已移除的 GPUI mesh 和 surface 路径。可复用的 3D 示例现位于
`gpui-3d` crate，覆盖 viewport API；`WgslShaderSource` 仍是通用 shader 校验工具。

```powershell
cargo run -p gpui-3d --example scene
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-dx12
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-vulkan
```

参阅 [`gpui-3d` 架构指南](../../../docs/GPUI_3D.md)，了解 3D/API 边界和当前后端验证状态。
