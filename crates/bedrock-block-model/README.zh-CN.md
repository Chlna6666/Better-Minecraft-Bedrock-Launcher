# bedrock-block-model

[English](README.md)

`bedrock-block-model` 把 Minecraft Bedrock 资源包中的方块视觉数据解析为单一模型描述，
供渲染器和 OBJ 导出器消费。

## 覆盖范围

- 旧版原版资源包 `blocks.json`；
- `minecraft:block` 自定义方块（`components` 与 `permutations`）；
- `textures/terrain_texture.json`（不回退到 `item_texture.json`）；
- `minecraft:material_instances`（render method、tint 元数据）；
- `models/**` 下的几何 JSON（`minecraft:geometry` 数组与旧版 `geometry.*` 根）；
- 方块 OBJ 导出的材质库语义、mesh-face 适配、OBJ 文档组装、导出统计与纹理复制目标；
- 世界资源包发现（`world_resource_packs.json`、资源包 manifest UUID、世界/共享资源包目录）。

## 边界

- 只解析资源包并产出模型/导出描述；不渲染、不依赖 GPU/UI。
- 不写世界数据；OBJ 导出由调用方决定输出位置。

## 构建与测试

```powershell
cargo check -p bedrock-block-model
cargo test -p bedrock-block-model
```

## 文档

- [crate 目录](../../docs/CRATES.md)
- [架构](../../docs/ARCHITECTURE.md)
