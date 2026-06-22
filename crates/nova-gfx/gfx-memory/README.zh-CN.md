# gfx-memory

[English documentation](README.md)

`gfx-memory` 提供 `nova-gfx` 后端使用的 GPU 内存分配封装。

它封装后端 allocator 对象，暴露内存统计，并提供资源上传路径使用的小型
upload-ring allocator。本 crate 不拥有 buffer、image、texture 等图形资源；
这些资源由具体后端 crate 拥有，并由后端把 allocation 绑定到资源上。

crate 已拆成职责明确的模块：

- common allocation 描述符、错误和统计。
- staging upload 使用的 upload-ring allocator。
- 按 GPU completed fence value 驱动的延迟释放队列。
- 由 Cargo feature 选择的后端 allocator wrapper。

## 后端

`gfx-memory` 使用 `default = []`。调用方只启用实际需要的后端 allocator：

```toml
gfx-memory = { version = "0.1", default-features = false, features = ["vulkan"] }
```

- `vulkan` 启用 `ash` 和 `gpu-allocator/vulkan`。
- `dx12` 启用 `windows` 和 `gpu-allocator/d3d12`。
- `metal` 启用 `objc2`、`objc2-metal` 和 `gpu-allocator/metal`。

平台不支持所请求后端时，非目标平台构造函数返回 `GfxError::Unavailable`。

## 生命周期规则

`DeferredFreeQueue<T>` 会在后端观察到关联 GPU fence 已完成前保留 payload。
后端 destroy 路径应立即移除 public handle，把 native payload 按 fence retire，
并且只在 poll 到 fence completed 后释放 native resource。

`UploadRingAllocator` 同样按 completed fence value 跟踪 busy page。submit 后
retire 的 page 不得在后端报告对应 GPU fence 完成前复用。同步 helper upload
路径只有在已经等待 queue 完成后，才可以把临时 fence 标记为 completed。
