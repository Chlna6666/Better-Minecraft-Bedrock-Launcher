# bedrock-scene

[English](README.md) | [简体中文](README.zh-CN.md)

从已解码的 Minecraft 基岩版区块和结构生成 CPU 侧 3D 几何。方块状态、方块实体、群系颜色
和资源包模型共同决定每个面的几何与材质。区块边界可以单独缓存，供相邻区块重建时复用。

- `chunk::collect` 采集流式构建所需的方块。将采集结果和已缓存的邻块边界插入用
  `new_stream` 创建的 `SceneBuilder`，再为目标区块生成面或体素描述。
- `chunk::prepare` 和 `SceneBuilder::build_selection` 生成选区的索引网格快照。
  `structure::build` 根据已解码的结构和调用方传入的调色板构建网格。

世界读取、可见区块调度、纹理加载和 GPU 提交均由调用方负责。库使用 `gpui_3d` 的 CPU 侧
数据类型，依赖声明关闭了 GPUI 集成。
