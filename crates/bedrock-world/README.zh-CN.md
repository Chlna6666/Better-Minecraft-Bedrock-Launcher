# bedrock-world

[English](README.md) | [简体中文](README.zh-CN.md)

`bedrock-world` 是构建在 `bedrock-leveldb` 之上的 Minecraft Bedrock 多版本世界库。
本 crate 负责 Bedrock 文件、数据库 key、NBT、区块、玩家、实体、biome、结构以及
跨版本兼容语义；底层 `bedrock-leveldb` 只负责 Mojang LevelDB 的
WAL/SST/MANIFEST、压缩、校验、缓存与任意字节 key/value 访问。

核心原则是：**保留存档中真实存在的持久化表示**。普通读取和写入不会静默升级、
降级、归一化，也不会补造历史版本不存在的数据。跨存储代际转换必须显式执行，
并在写入前完成 preflight。

## 快速开始

普通只读工具应让世界层自动识别真实存储格式：

```rust
use bedrock_world::{BedrockWorld, WorldScanOptions};

fn inspect() -> bedrock_world::Result<()> {
    let world = BedrockWorld::open_auto_blocking("path/to/minecraftWorld")?;
    println!("format={:?}", world.format());

    let versions = world.versions_blocking()?;
    println!("mixed={}", versions.has_mixed_version_storage());
    println!("future={}", versions.has_future_storage());

    let chunks = world.list_chunk_positions_blocking(WorldScanOptions::default())?;
    println!("chunks={}", chunks.len());
    Ok(())
}
```

开启 `async` feature 后可使用 `BedrockWorld::open_auto` 和对应 async wrapper。
只有需要显式格式提示或可写 LevelDB 时，才使用
`BedrockWorld::open_blocking(path, OpenOptions)`。

## 支持的真实存储代际

世界打开逻辑按实际文件与记录识别：

- 当前 Mojang LevelDB 世界；
- 仍使用 `LegacyTerrain` 的早期 LevelDB 世界；
- 使用 `chunks.dat` 的 pre-LevelDB Pocket Edition 世界；
- 与 `chunks.dat` 同期的 Pocket `entities.dat`；
- 同一世界混合多个记录代际的部分升级/过渡世界；
- 未知或未来 chunk tag、数据库 key、SubChunk version，并将其保留为未知证据，
  而不是强行解释成某个已知版本。

`WorldVersions` 记录真实持久化证据。不能只依据
`level.dat.lastOpenedWithVersion` 推断整个世界的存储格式。

## Pocket `chunks.dat` 不等于后来的 `LegacyTerrain`

旧 Pocket terrain 的确认核心长度是 **82,176 bytes**，包含：

- block id；
- block metadata；
- sky light；
- block light；
- 16x16 height map。

后来的 LevelDB `LegacyTerrain` 多出 **1,024 bytes** 的 biome/RGB tail，完整长度为
**83,200 bytes**。该 tail 保存 256 个 `[biome_id, red, green, blue]` 样本。

库会严格区分两者：真实 82,176-byte Pocket 数据读取后仍然是 82,176 bytes，
不会自动补默认 biome id 或 RGB。

`LegacyTerrain::has_biome_samples()` 可以判断是否真实存在 biome tail。
Pocket core 上的方块、metadata、light、height 仍可编辑；如果源记录没有 biome tail，
修改 biome sample 会直接报错，不会隐式扩展数据。

在尝试复制到后期 LevelDB 表示前，应先调用
`check_pocket_chunks_dat_leveldb_import_blocking`。
`import_pocket_chunks_dat_records_blocking` 会在目标发生任何修改前拒绝缺少必要持久化
biome/RGB 信息的有损转换。

## Pocket `entities.dat`

历史 Pocket `entities.dat` 按真实文件格式解析，而不是伪装成 LevelDB actor：

- `ENT\0` magic；
- 小端文件版本；
- 小端 NBT 长度；
- 一个包含 `Entities` / `TileEntities` 的小端 NBT root。

未修改文档可返回原始 bytes；编辑时保留未知 root 字段和 trailing bytes。

显式 import 会把 entity 写成旧 chunk `Entity` 记录，把 tile entity 写成
`BlockEntity`，不会直接跳到现代 `digp/actorprefix`。位置和冲突检查在目标 batch
提交前完成。

## LevelDB 语义

启用 `backend-bedrock-leveldb` 后，公开具体后端为 `BedrockLevelDbStorage`。
公开 storage 抽象还包括 `WorldStorage`、`PartitionedWorldStorage`、`MemoryStorage`、
扫描控制/结果以及 `StorageBatch`。

Mojang/Bedrock native table 默认使用 **raw DEFLATE compression id `0x04`** 写出。
底层 `bedrock-leveldb` 仍可读取并显式选择标准 zlib `0x02`、Snappy 或无压缩。

旧的 synthetic 公共 `PocketChunksDatStorage` 已撤销。Pocket 世界应由 world layer 打开，
因为 pre-LevelDB terrain 与后期 LevelDB `LegacyTerrain` 并不是 byte-equivalent 表示。

## 玩家数据

玩家数据可能存在于：

- `level.dat.Player`；
- `~local_player`；
- `player_<id>` LevelDB key。

`PlayerId` 只作为文本型 convenience API。任意非 UTF-8 的 `player_*` suffix 通过 raw
player-key API 原样保留，不经过 `String::from_utf8_lossy`，因此 decode → encode 不会改变
原始 key。

历史 `level.dat.Player` 处理是显式的。不能因为调用者选择了 legacy player id，
就把整个 `level.dat` root 当成玩家 NBT。

历史 saved-item 写入按具体目标版本族执行，并要求确切 mapping。缺失或歧义的历史 item / 
BlockState 映射必须在目标修改前拒绝。

## 区块、SubChunk、biome 与渲染

交互式工具应只请求实际需要的数据：

- `list_render_chunk_positions_blocking`；
- `list_chunk_positions_in_region_blocking`；
- `query_chunk_data_blocking`；
- `query_chunk_data_many_blocking`；
- `query_chunk_region_blocking`；
- `parse_chunk_blocking` 用于完整结构化检查。

`ChunkDataRequest` 可以组合 surface columns、固定 layer、cave slice、完整 3D indices、
height map、biome 和 block entity。渲染热路径使用 exact batch read，避免全世界扫描和
不必要的 4096-index materialization。

过渡世界可能同时存在 `LegacyTerrain` 与 `SubChunkPrefix`。库保留两者；渲染时应优先
使用真实 SubChunk 方块数据，仅在确实缺少现代记录时使用 legacy terrain 作为来源/fallback。

未知 SubChunk version byte 会作为兼容性证据保留，不能静默按已知版本解析。

## Actor 与 BlockEntity

旧 chunk inline `Entity` 与现代 `digp -> actorprefix` 是两套不同存储代际。
读取支持两者；普通写入不会静默把一种迁移为另一种。

兼容性/完整性扫描会报告：

- `digp` dangling reference；
- orphan `actorprefix`；
- 同一 actor 被多个 chunk digest 持有；
- 损坏的 actor digest payload。

结构性错误可以直接把对应 chunk 标记为 `Corrupt`，而不是仅记录一个普通 parse warning。
现代 actor 写入会在同一事务内维护 digest 与 actor record。

BlockEntity payload 使用连续小端 NBT roots。具体 rewrite 会校验 chunk 坐标，并保留无关数据。

## `level.dat`、NBT、map、global 与 structure

启动器只需要元数据时，应优先使用文件级 `level.dat` API，不需要打开 LevelDB。
`LevelDatDocument` 会保留 header 信息与非致命读取 warning。

Bedrock 小端 NBT 支持 owned parse、borrowed/event 遍历以及 consecutive roots。

typed helper 覆盖 map、village、常见 global record、hardcoded spawn area、biome、actor、
block entity、player、chunk record 与 `.mcstructure` 导入/导出/放置。多记录修改在 commit
前执行目标验证。

## 写入规则

`OpenOptions::default()` 为只读。需要编辑 LevelDB 世界时必须显式以 writable 方式打开：

```rust
let world = bedrock_world::BedrockWorld::open_blocking(
    "path/to/minecraftWorld",
    bedrock_world::OpenOptions {
        read_only: false,
        ..Default::default()
    },
)?;
```

pre-LevelDB Pocket world handle 始终保持只读。把 Pocket 世界转换到另一代存储格式，
必须走单独的显式 import/conversion 流程。

高层写入在 commit 前校验序列化后的表示。未知/未来记录默认保留，除非调用者明确选择
一个目标格式已经完全证明的破坏性操作。

## 历史兼容 corpus

synthetic unit test 不能等同于真实历史世界兼容证明。可以通过以下环境变量挂载
真实或脱敏后的历史世界集合：

```text
BEDROCK_WORLD_FIXTURE_ROOT=/path/to/world-corpus
BEDROCK_WORLD_REQUIRE_HISTORICAL_FIXTURES=1
```

当前 world matrix 的命名 fixture 从 `bedrock-0.6.1` 覆盖到 `bedrock-1.26`，并包含
`future-unknown`。开启 `REQUIRE` 后，任何缺失或不完整 fixture 都会直接导致测试失败。

底层 Mojang LevelDB corpus 使用：

```text
BEDROCK_LEVELDB_FIXTURE_ROOT=/path/to/leveldb-corpus
BEDROCK_LEVELDB_REQUIRE_HISTORICAL_FIXTURES=1
```

`tests/fixtures/sample-bedrock-world` 仍只是可选的大型本地性能 fixture。私有性能 fixture
被 skip 不代表历史兼容测试通过。

完整测试约束见 [`docs/TESTING.md`](docs/TESTING.md)。

## 完整性模型

| 范围 | 当前行为 |
| --- | --- |
| `level.dat` header + 小端 NBT | 已实现，并保留未知字段 |
| Mojang LevelDB raw key/value | 通过 `bedrock-leveldb` 实现 |
| Bedrock native raw-DEFLATE table 写出 | 已实现，compression id `0x04` |
| chunk key 分类 | 已知代际分类；未知 key/tag 保留 |
| Pocket 82,176-byte terrain | 已实现，不补造 biome tail |
| LevelDB 83,200-byte `LegacyTerrain` | 已实现 biome/RGB samples |
| legacy / paletted SubChunk | 已知持久化版本已实现；未知 version 保留 |
| Data2D/Data3D biome + height | 已实现 |
| 玩家记录族 | 已实现；任意 raw `player_*` key 独立保真 |
| legacy inline / modern actor | 已实现，并提供完整性诊断 |
| map/global/HSA/block-entity/actor 写入 | typed + validation |
| 历史版本转换 | 仅在 source/target 表示被证明时支持 |
| 未知/未来格式 | 保留并报告，不猜测 |

**“能够读取”不等于“能够无损写成任意历史版本”。** 对这个区别，以 compatibility report
和显式 preflight API 为准。

## 性能模型

- 只需要 `level.dat` 时不打开数据库；
- render 使用 exact `get_many`，不扫描无关记录；
- 分类与边界发现使用 key-only scan；
- 大型离线扫描使用有界 table-parallel reduction；
- 外层已有 worker pool 时避免嵌套线程池过量并行；
- 不需要 owned structured form 时优先保留 raw bytes / borrowed view。

benchmark 与大型 fixture 说明见 [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md)。

## 错误处理

公开 fallible API 返回 `bedrock_world::Result<T>`。应用侧应匹配
`BedrockWorldError::kind()`，不要解析展示字符串。重要分类包括 read-only、validation、
unsupported format、corrupt world、cancelled 与 LevelDB error。

更详细说明见 [`docs/API.md`](docs/API.md)、[`docs/TESTING.md`](docs/TESTING.md) 和
[`ARCHITECTURE.md`](ARCHITECTURE.md)。
