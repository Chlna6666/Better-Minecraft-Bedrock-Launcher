# gfx-dx12

图形 pass 跳过不变的 pipeline、scissor 与根描述符表；资源键包含缓冲范围和纹理视图句柄，
根签名变化时清空描述符表缓存。
逐 draw 录制借用 pipeline 和 layout 对象，避免重复克隆。

[English](README.md)

计算批次共用一份命令列表和完成信号，并在批次内复用相同的缓冲描述符。
缓冲上传复用 fence 已完成的命令对象及保持映射的上传环页面。
间接参数转换复用已完成 fence 对应的暂存缓冲，并限制缓存数量；显式内存裁剪会清除此缓存，
保留的暂存字节计入资源统计。

缓冲计算与非索引间接绘制使用统一的 `ResourceDevice` 接口。计算 pipeline 可复用；descriptor
分配和间接参数暂存缓冲保留到提交 fence 完成。通用绘制记录在 GPU 上转换为 root constants
与原生绘制参数。计算与绘制在有序 graphics queue 上执行，无需 CPU 回读或每次派发等待完成。

设备本地缓冲写入使用有序批量上传，暂存资源保留到 fence 完成，CPU 不逐批等待完成。
CPU 可见的映射写入仍要求调用方使用已不再被前序提交读取的资源槽位。

`gfx-dx12` 是 `gfx-core` 设备 trait 的 Direct3D 12 实现。Windows 上创建原生 D3D12 设备、
资源、pipeline、command encoder 与 DXGI swapchain；非 Windows 目标仍实现 trait，但以
`Error::Unavailable` 的 stub 返回，便于跨平台 crate 编译非目标路径。

`raw-window-handle` 只在本 backend 内用于 native surface 创建；`gfx-core` 不依赖它。

## Shader 编译

生产调用方应提供预编译的 `ShaderCode::DxBytecode`。运行时 HLSL/FXC 编译默认关闭；
`shader-compiler` feature 显式启用 FXC 绑定，面向 shader 工具/示例，而不是应用启动。

## 边界

- 纯图形后端；不依赖 UI 框架/应用代码。
- 呈现与 surface 生命周期通过 `gfx-core` trait 暴露。

## 构建与测试

```powershell
cargo check -p gfx-dx12
cargo test -p gfx-dx12 -- --ignored --nocapture --test-threads=1
```

## 文档

- [nova-gfx](../README.zh-CN.md)
