# bedrock-world

[English](README.md)

`bedrock-world` 是可独立复用的多版本 Minecraft Bedrock 地图数据中间层。
它拥有 Bedrock 文件/key/NBT/区块/玩家/实体/生物群系/结构/版本兼容语义；底层 crate 只负责
Mojang LevelDB 的 WAL/SST/MANIFEST 机制。

解析和内存编辑不需要数据库连接。`WorldStorage` 负责原始记录存取，`World` 与事务
将领域操作接到内存、自定义存储或可选的 `bedrock-leveldb` 后端。

NBT/SNBT 值模型与编解码由 [bedrock-nbt](../bedrock-nbt/README.zh-CN.md) 提供。
本库管理世界字段语义和文件头，公开 NBT 命名空间直接暴露底层编解码及其错误类型。

核心规则：**保留实际持久化的表示**。普通读写不会静默升级、降级、规范化或发明历史字段；
会改变 generation 的操作必须显式请求并预检。

`SubChunk::estimated_cpu_bytes()` 估算解码后的调色板与索引存储，包括压缩索引和嵌套方块状态。
该估算不包含分配器及映射容器的管理开销，不等同于进程 RSS。

## 边界

- 不依赖 UI/GPU；不负责渲染（渲染在 `bedrock-render`）。
- 不直接实现 LevelDB 存储机制（在 `bedrock-leveldb`）。
- 写入/升级操作必须先预检并保留可回滚路径。

## 数据能力与接口

| 数据领域 | 当前接口与边界 |
| --- | --- |
| `level.dat`、世界时间与规则 | `level::parse/encode/read/write`；`LevelDatDocument::has_field/field/field_mut/set_field/remove_field` 保留缺失状态、NBT 类型和未知字段；时钟通过 `clock/set_clock` 访问 |
| 玩家、背包、装备 | `player::PlayerData` 可从 NBT/字节构造、内存编辑及编码；`World::player/save_player` 连接存储 |
| 区块、子区块与方块状态 | `chunk`、`block` 拥有格式与数据模型；`editor` 负责有校验的方块编辑、放置和事务计划 |
| 群系、高度图 | `biome`、`chunk::ChunkHeightMap` 及 `World` 查询；历史缺失数据不会自动补成现代格式 |
| 普通实体与方块实体 | `entity`、`block` 模型及 `World` 编辑接口；现代实体写入维护 `digp`/`actorprefix` 关联 |
| 计划刻、随机刻 | `chunk::ticks::ChunkTicks::from_records/encode_pending/encode_random` 不访问存储；`has_pending/has_random` 区分缺失与空记录，`World::chunk_ticks/write_chunk_ticks` 校验来源并批量提交 |
| 事件、计分板、传送门、常加载区域 | `global::parse/encode` 与 `global::Record` 保留完整 NBT；`World::has_global/global/save_global/remove_global` 提供存在性、查询与快照保护的编辑。领域特定的全部字段尚未逐一类型化 |
| 硬编码生成区（HSA） | `World::hardcoded_spawn_areas_snapshot` 精确读取一个区块；事务 `update_hardcoded_spawn_areas/remove_hardcoded_spawn_areas` 校验来源后更新或删除 |
| 地图物品、村庄 | `map_item`、`village` 模型和 `World` 读写；保留未知 NBT 字段 |
| 结构与历史存档 | `structure`、`pocket` 提供对应格式入口；转换必须显式请求 |
| 未实现的二进制记录 | 保留原始字节；`AabbVolumes`（`0x77`）是结构包围盒记录，不是区块版本，也不交给 NBT 解析器 |

`global::Record::new` 表示目标键必须不存在；`global::parse` 保留来源，包括长度为零的
现有记录。`save_global/remove_global` 拒绝过期快照。按键 `delete_global` 是显式无快照
删除操作。`level.dat` 属于独立文件，不参与键值事务；内存编辑后由调用方决定何时编码或写盘。

这些是存档数据编辑接口，不是游戏事件执行器或游戏刻模拟器。NBT 可编辑不代表该领域
已经具备所有类型化语义，也不代表应用 GUI 已暴露全部功能。地图编辑器会提交修改后的 HSA、
方块实体、高度图、Data3D 群系、全局记录和刻队列文档；普通群系保存不会转换历史 2D 数据。

## 指令集与性能

默认 `simd` feature 启用运行时指令集检测。当前子区块 4-bit 索引有 x86/x86_64 AVX2
路径；其他位宽、CPU 或截断输入使用标量路径。NBT/SNBT 的 SIMD 由 `bedrock-nbt`
提供。这不是所有格式的通用加速保证；吞吐量和峰值内存需要用实际数据及基准测量。

仅使用内存模型和编解码时，可关闭默认后端 feature：

```toml
bedrock-world = { version = "0.7", default-features = false }
```

## 构建与测试

```powershell
cargo check -p bedrock-world
cargo test -p bedrock-world
```

支持的版本与地形格式由 crate 测试覆盖；具体 API 行为以 rustdoc 为准。

## 文档

- [构建与测试](../../docs/BUILD_AND_TEST.md)
- [crate 目录](../../docs/CRATES.md)
