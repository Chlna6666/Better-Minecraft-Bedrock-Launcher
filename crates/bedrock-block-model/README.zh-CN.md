# bedrock-block-model

[English](README.md)

`bedrock-block-model` 把 Minecraft Bedrock 资源包中的方块视觉数据解析为单一模型描述，
供渲染器和 OBJ 导出器消费。

`vanilla_voxel_shapes()` 提供独立的只读形状数据库，包含从所提供的 Bedrock 1.21.110
行为包文档解码的 218 个体积、684 个盒体。`generated/vanilla_voxel_shapes.json` 保留
标识符、版本和原始 16 单位坐标；API 将盒体转换为方块单位，并预计算保守的 16×16
边界覆盖。未知标识符返回 `None`。这些体积没有 UV、材质或完整 BlockState 映射，
不会替换捕获的视觉模型。覆盖本身不代表不透明：调用方必须确认状态及不透明材质语义，
才能用来剔除面。库不嵌入游戏着色器字节码。

## 覆盖范围

- 旧版原版资源包 `blocks.json`；
- `minecraft:block` 自定义方块（`components` 与 `permutations`）；
- `textures/terrain_texture.json`（不回退到 `item_texture.json`）；
- `minecraft:material_instances`（render method、tint 元数据）；
- `models/**` 下的几何 JSON（`minecraft:geometry` 数组与旧版 `geometry.*` 根）；
- 方块 OBJ 导出的材质库语义、mesh-face 适配、OBJ 文档组装、导出统计与纹理复制目标；
- 世界资源包发现（`world_resource_packs.json`、资源包 manifest UUID、世界/共享资源包目录）；
- `resource_packs/vanilla` 为权威包：它排在 `vanilla_*` 覆盖包之后，因此同名纹理以基础包为准，
  覆盖包只补齐基础包没有的文件。

## 运行时 BedrockSceneCapture 模型库

已移除不完整的内置 `captured_detail_models.bin` 与 `native_models.json`，
不再把 Java 版网格当作基岩版原生几何的自动替代品。

3D 编辑器按选定游戏安装目录读取 BedrockSceneCapture 的 Geometry Pool v2：`manifest.json`、`blocks.json` 和 `geometry_pool` 下的几何、外观、材质表。通过完整 BlockState 精确查找，按 SHA-256 引用懒加载共享资源；不再依赖旧 `meshes/` 完整网格。
纹理仅保存游戏中的资源别名，路径从当前游戏资源包的
`terrain_texture.json` 动态解析；不维护跨版本硬编码贴图表。

可以通过 `BMCBL_CAPTURE_MODEL_DIR` 显式指定模型导出目录。
缺少真实采样时，旧的基岩版程序化形状仍属不保证精度的临时回退，
不能将其标记为原生精准模型。

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