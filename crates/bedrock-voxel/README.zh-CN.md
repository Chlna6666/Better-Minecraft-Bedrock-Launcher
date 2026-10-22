# bedrock-voxel

[English](README.md)

`bedrock-voxel` 把图片和 OBJ 模型转换为 Minecraft Bedrock 方块放置计划。它不访问世界存储：
计划保持本地，直到调用方通过 `bedrock-world` 校验并应用。

## 公共接口

- `block_image`：图片到方块的计划；
- `obj` / `obj_voxel`：OBJ 加载、材质与体素化；
- `palette`：默认方块候选；
- `relief`：高度/浮雕转换辅助。

## 边界

- 纯转换：不做世界 IO、不依赖 UI/GPU。
- 计划的应用、确认与回滚由调用方负责。

## 构建与测试

```powershell
cargo check -p bedrock-voxel
cargo test -p bedrock-voxel
```

## 文档

- [架构](../../docs/ARCHITECTURE.md)
- [crate 目录](../../docs/CRATES.md)
