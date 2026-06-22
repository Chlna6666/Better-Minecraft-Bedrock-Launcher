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
let shader = source.compile(surface.device());
```

一次性编译可以使用 helper：

```rust
let shader = gpui::compile_wgsl_shader_module_from_path(
    surface.device(),
    "examples/viewer.wgsl",
)?;
```

生成的或内嵌的 shader 字符串可以使用 source label：

```rust
let shader = gpui::compile_wgsl_shader_module(
    surface.device(),
    "generated-material-shader",
    generated_wgsl,
)?;
```

loader 会在创建 Nova shader module 之前用 `naga` 校验 WGSL。文件读取错误会包含
路径；解析和校验错误会包含传入的 label 或路径，以及格式化后的 WGSL 诊断信息。

## 与 GPU Surface 集成

运行时 WGSL 通常配合 `Window::paint_gpu_mesh_3d` 使用：

1. 创建由 GPUI 管理的 GPU surface。
2. 使用 surface device 编译 shader。
3. 从同一个 device 构建 bind groups、pipelines、buffers 和 textures。
4. 渲染到 `GpuSurfaceHandle::back_buffer_view`。
5. present 或 swap 渲染后的 buffer，再把 surface 绘制到 GPUI 场景。

自定义 render pipeline 的 color target 必须匹配 surface texture format。

## 错误处理

把 shader 加载视为可能失败的应用初始化：

- 文件系统错误应带上 source path 后返回或显示。
- parse 与 validation diagnostic 应反馈给用户或开发日志。
- shader 或 surface format 改变时，重建依赖的 pipeline。
- 除非 shader 属于框架渲染器，否则不要把运行时 shader 错误放进 GPUI renderer
  internals。

## 示例

`hatsune_miku_viewer` 在 Windows 上演示完整流程：

- 从示例 shader 文件加载 WGSL；
- 用 `tobj` 解析 OBJ 和 MTL；
- 用 `image` 加载材质贴图；
- 将按材质拆分的 submesh 渲染到 GPUI 管理的 GPU surface；
- 支持鼠标拖拽旋转、滚轮缩放和 resize。

设置 `GPUI_HATSUNE_MIKU_DIR` 可指定 OBJ 资源目录。

```powershell
cargo run --example hatsune_miku_viewer
```
