# GPU Mesh 3D

[English](gpu_mesh_3d.md)

GPUI 可以把 renderer-owned 3D mesh primitives 作为普通 scene 的一部分绘制。这不同
于自定义 GPU surfaces：mesh primitives 提交给 GPUI renderer，而 GPU surfaces 让
应用拥有完整 render pipeline。

## 使用场景

当 UI rendering 需要适合 GPUI scene batching 和 renderer resource model 的有限 3D
primitive 时，使用 GPUI mesh primitives。当应用需要完全控制 shaders、materials、
passes 或 GPU resources 时，使用自定义 GPU surface。

## Renderer Behavior

Nova renderer 会把 mesh buffers 和 shader modules 保存在 retained resources 中。
当 windows idle 或 memory pressure 需要清理时，这些资源可以被 trim。

Mesh paint order 仍属于 scene ordering。不要把 application stateful model loaders
直接构建进 renderer internals。

## Guidelines

- geometry 跨帧复用时保持 mesh ids 稳定。
- 只有 vertex data 改变时才重建 mesh buffers。
- 诊断 mesh-heavy scenes 时使用 metrics。
- 完整 model viewers 和 material systems 优先使用 GPU surfaces。
