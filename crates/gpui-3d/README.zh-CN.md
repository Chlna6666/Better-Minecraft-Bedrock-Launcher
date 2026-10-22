# gpui-3d

[English](README.md)

`gpui-3d` 提供可复用的 3D 场景、几何、相机、材质、动画、查询与 GPUI scene-view 层。
应用把领域数据转换成本 crate 的类型；GPUI 负责通用 `RendererExtension` 调度与 Nova 集成，
`gpui-3d` 负责 3D 场景准备与 scene-view 资源；BMCBL 专属的世界/预览策略留在 `src/ui`。

## 渲染路径

- **GPUI scene view**：通过 `RendererExtension` 在宿主合成 pass 内绘制。
- **Offscreen renderer**：`OffscreenRenderer` 渲染方向光阴影、线性 PBR 场景、可选 IBL/天空、
  显示参考后处理到离屏目标，支持 readback、resize、trim、调试视图、延迟完成轮询与逐帧统计。
  使用调用方提供的 Nova device，不创建第二个窗口渲染器、swapchain 或缓存所有者。

`load_path` / `load_bytes` 支持 glTF 2.0 与 GLB（PBR 材质、mip 纹理、节点层级、灯光、
相机、变换动画）。

## 边界

- 依赖 `gpui` + `gfx-core`；不拥有窗口、swapchain、后端或应用策略。
- 不提供播放时钟；动画采样使用调用方提供的时间。
- 离屏渲染能力与后端状态随实现推进，具体未实现项以 crate rustdoc 为准。

## 构建与测试

```powershell
cargo check -p gpui-3d
cargo check -p gpui-3d --tests
```

`test_assets/` 是有许可证与哈希固定的测试模型资产，不是文档或运行时依赖。

## 文档

- [工作区架构](../../docs/ARCHITECTURE.md)
- [crate 目录](../../docs/CRATES.md)
- [构建与测试](../../docs/BUILD_AND_TEST.md)
- [English README](README.md)
