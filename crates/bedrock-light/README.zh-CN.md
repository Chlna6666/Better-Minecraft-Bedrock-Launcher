# bedrock-light

[English](README.md)

独立于存储的 Bedrock 体素光照计算库，负责有界六邻接天空光/方块光传播；不包含瓦片渲染、
UI、GPU 或世界 IO。`bedrock-block-model` 提供状态属性，应用组装单元区域，`gpui-3d`
负责 GPU 着色；`bedrock-render` 保持二维瓦片渲染职责。

`LightVolume::solve` 允许 15 级天空光沿透明单元垂直向下保持，对横向光与方块光执行衰减，
重新求解可移除旧光源。输入应覆盖完整垂直列及至少 15 格水平邻接层，未知单元应不透光。
输出高半字节为天空光、低半字节为方块光；不写入存档光数组，也不承诺覆盖全部原版状态/形状。

```powershell
cargo check -p bedrock-light
cargo test -p bedrock-light
```
