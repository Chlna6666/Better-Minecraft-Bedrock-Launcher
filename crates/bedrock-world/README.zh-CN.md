# bedrock-world

[English](README.md)

`bedrock-world` 是构建在 `bedrock-leveldb` 之上的多版本 Minecraft Bedrock 世界库。
它拥有 Bedrock 文件/key/NBT/区块/玩家/实体/生物群系/结构/版本兼容语义；底层 crate 只负责
Mojang LevelDB 的 WAL/SST/MANIFEST 机制。

核心规则：**保留实际持久化的表示**。普通读写不会静默升级、降级、规范化或发明历史字段；
会改变 generation 的操作必须显式请求并预检。

`SubChunk::estimated_cpu_bytes()` 估算解码后的调色板与索引存储，包括压缩索引和嵌套方块状态。
该估算不包含分配器及映射容器的管理开销，不等同于进程 RSS。

## 边界

- 不依赖 UI/GPU；不负责渲染（渲染在 `bedrock-render`）。
- 不直接实现 LevelDB 存储机制（在 `bedrock-leveldb`）。
- 写入/升级操作必须先预检并保留可回滚路径。

## 构建与测试

```powershell
cargo check -p bedrock-world
cargo test -p bedrock-world
```

支持的版本与地形格式由 crate 测试覆盖；具体 API 行为以 rustdoc 为准。

## 文档

- [构建与测试](../../docs/BUILD_AND_TEST.md)
- [crate 目录](../../docs/CRATES.md)
