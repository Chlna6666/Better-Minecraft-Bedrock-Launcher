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
- 世界资源包发现（`world_resource_packs.json`、资源包 manifest UUID、世界/共享资源包目录）；
- `resource_packs/vanilla` 为权威包：它排在 `vanilla_*` 覆盖包之后，因此同名纹理以基础包为准，
  覆盖包只补齐基础包没有的文件。

## 内嵌原生模型目录（由游戏目录生成）

`generated/native_models.json` 由真实 Bedrock 版本目录生成并内嵌进 crate（`src/native_models.rs`），
3D 编辑器运行时无需再扫描 `Minecraft.Windows.exe`。目录对 `blocks.json` 中的 **每一个方块 id**
（26.40 约 1280 个，含行为包数据驱动方块）都有一条记录：

- `render_source: "voxel_shape"`：来自游戏自带 `data/behavior_packs/*/shapes/**` 的原生形状盒子
  （栅栏、铁砧、箱子、堆肥桶等）；
- `render_source: "pack_geometry"`：来自资源包 `models/blocks/*.geo.json` 的方块模型；
- `render_source: "cube"`：其余方块，仍保留材质槽、引擎几何模板 id（如
  `minecraft:geometry.template_fence_gate`）与原生 occlusion 形状 id；渲染回退到 family/Java 形状，
  因为原版渲染模板的 cuboid/UV 由引擎代码生成，不在随包数据中。

`model_shape_for_block_state` 优先使用内嵌原生目录；环境变量 `BMCBL_BLOCK_MODEL_SOURCE` 可取
`native`（默认）、`native-all`（额外把 occlusion 形状当渲染形状）、`legacy`。

`generated/item_texture_map.json` 是物品 id → 图标名映射（染料旧名、`wooden_*`/`golden_*` 工具、
唱片等），由 `ObjTextureResolver::item_texture_for` 使用。

游戏更新后重新生成：

```powershell
python crates/bedrock-block-model/tools/generate_native_models.py --game-dir "<版本目录>"
python crates/bedrock-block-model/tools/generate_item_texture_map.py --game-dir "<版本目录>"
```

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