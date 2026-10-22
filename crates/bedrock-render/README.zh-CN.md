# bedrock-render

[English](README.md)

`bedrock-render` 是 Minecraft Bedrock 世界的 CPU 瓦片渲染器。它通过 `bedrock-world` 读取
LevelDB、NBT、区块/子区块、高度与生物群系数据；渲染、调色板、瓦片规划、图片编码、取消、
线程、预览与基准都在本 crate，使世界解析器保持轻量。

## 特性

- 默认特性包含 `async` 与 `webp`；GPU 后端通过 `gpu`、`gpu-dx11`、`gpu-vulkan`、`gpu-dx12`
  显式启用；纯 CPU 消费者可使用 `--no-default-features` 选择图片格式。
- 渲染平面为 X/Z，Y 只作为采样参数；`RenderLayout` 控制每瓦片区块数与每像素方块数。

## 边界

- 不依赖 `gpui` 或任何 UI；不写世界数据。
- 世界语义与存储分别在 `bedrock-world` / `bedrock-leveldb`。

## 构建与测试

```powershell
cargo check -p bedrock-render
cargo test -p bedrock-render
```

## 文档

- [构建与测试](../../docs/BUILD_AND_TEST.md)
- [crate 目录](../../docs/CRATES.md)
