# GPU Surfaces

[English](gpu_surfaces.md)

旧的应用自管 surface API 已经移除。自定义 GPU 内容应通过 GPUI scene primitive
进入渲染流程，目前使用 `Window::paint_gpu_mesh_3d` 提交 retained 3D mesh 内容。
具体 Nova GFX 后端资源和 present 路径由平台 renderer 持有。

如果应用需要新的自定义 GPU 能力，应先在 `nova-gfx` 中补齐，再暴露为 GPUI scene
primitive 或 adapter。GPUI 应用代码不应依赖后端 device、queue、command buffer 或
swapchain handle。
