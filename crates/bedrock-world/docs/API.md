# `bedrock-world` API

本文件记录当前公开接口和语义边界。公开 API 以 Rust 文档为准；这里不保留已经删除的旧接口别名。

## 打开世界

只知道世界目录时优先使用自动识别：

```rust
use bedrock_world::{World, Result};

fn open_world(path: &str) -> Result<()> {
    let world = World::open(path, OpenOptions::default())?;
    println!("{:?}", world.format());
    Ok(())
}
```

需要控制只读、格式或扫描策略时使用 `OpenOptions`。

## LevelDB 语义

启用 `bedrock-leveldb` feature 后，公开具体后端为 `BedrockLevelDbStorage`。公开 storage 抽象还包括 `WorldStorage`、`PartitionedWorldStorage`、`MemoryStorage`、扫描控制/结果以及 `StorageBatch`。

`bedrock-world` 只负责 Minecraft Bedrock 的 key/value 语义；WAL、SST、MANIFEST、压缩、校验和、缓存、compaction 等数据库引擎机制属于 `bedrock-leveldb`。

正式 Bedrock LevelDB 表写出默认使用 Mojang/Bedrock compression id `4` 的 raw DEFLATE。id `2` 的 zlib framing 只作为显式底层兼容策略存在。

## 世界版本证据

```rust
let versions = world.versions()?;
```

版本证据来自实际 world data，而不是由单一版本字符串推导。报告可包含：

- `level.dat` header / `StorageVersion`；
- `lastOpenedWithVersion` / `MinimumCompatibleClientVersion`；
- 实际 LevelChunk version；
- SubChunk V0-V9 与未知 version byte；
- `LegacyTerrain`；
- `Data2D` / `Data2DLegacy` / `Data3D`；
- chunk `Entity`；
- `digp` / `actorprefix`；
- 未知或未来 record。

mixed-version 世界会保留这些证据，不被强制折叠成一个内部 schema。

## 方块状态

`BlockState` 保留方块 permutation 的完整 `states` 复合标签。完整支持的边界是无损、
按 Mojang 原键访问，而不是只枚举当前版本里已知的少数属性：

```rust
for (name, value) in block_state.state_entries() {
    println!("{name} = {value:?}");
}

let custom_direction = block_state.state_integer("direction")?;
let future_state = block_state.state("minecraft:future_state");
```

因此未知、未来或附加包方块的方向、上下半部、连接、年龄等状态不会在解析时丢失。
`state_boolean`、`state_integer` 和 `state_string` 会验证 NBT 类型；类型不符返回错误，
不会静默使用默认值。

常见原版方块另提供 Minecraft 语义化视图：`horizontal_direction()`、
`facing_direction()`、`block_face()`、`vertical_half()`、`corner()`、
`door_states()`、`trapdoor_states()`、`stair_states()`、`slab_states()` 和
`redstone_states()`。这些视图同时返回方向、上下/开合/铰链/角形/信号等构成该
permutation 的状态；缺失必需字段或数值越界会报错。调用方不应通过方块名称猜测
朝向，也不应把门、活板门和楼梯的不同数字方向编码混用。

## Biome

磁盘层始终保留真实数值 ID：

```rust
let id: Option<u32> = world.biome_id(chunk, x, z, y)?;
```

`Data2D`、`Data2DLegacy`、`Data3D` 的解析和写回不依赖 biome 名称表。未知、未来版本或第三方世界中的 ID 仍按原数值保留。

`bedrock-world` 不维护或嵌入 `Biome ID -> 名称/属性` registry，也不会根据游戏版本猜测数值 ID 的语义。名称显示、属性查询、按 biome 名称编辑以及草地/树叶/水体着色规则属于上层应用或 `bedrock-render`；需要这些能力时应由调用方提供明确的数据源。

核心 world API 只保证持久化事实的保真读写。只修改与 biome 无关的数据时，原始 biome 数值必须原样保留。

## 高度图与 biome 写回

高度图是 biome record 的组成部分，不能把它当成独立无上下文数组写入。

安全写回必须遵守：

- `Data2D`：只替换 height map，保留原 256 个 biome ID；
- `Data2DLegacy`：只替换 height map，同时保留原 biome ID 与 RGB；
- `Data3D`：只替换 height map，保留全部 paletted biome storages；
- 目标 chunk 没有可确定的同代 biome record 时拒绝写；
- mixed/冲突表示不能凭调用者传入的 `ChunkVersion` 静默选一份；
- 禁止通过 `vec![0; 256]`、空 Data3D storage 或其它默认数据补造世界里不存在的信息。

## SubChunk

SubChunk 版本使用实际持久化 version byte：

```text
V0 V1 V2 V3 V4 V5 V6 V7 V8 V9 Unknown(u8)
```

未知 version 保留 raw bytes，并禁止不了解格式的结构化写回。

跨版本 SubChunk 写出必须使用具体目标：

- `upgrade_subchunks_blocking(target_game_version, upgrade_data, target_palette)`；
- `downgrade_subchunks(target_subchunk_version, numeric_table)`。

历史 numeric ID/meta 的反向写出必须通过正向升级结果验证，不能把 rename 表直接反转后猜值。

## Actor

旧版：

```text
chunk Entity
```

新版：

```text
digp<ChunkKey>
  -> actor storage id list
actorprefix<Actor storage id>
  -> actor NBT
```

转换要求 actor 顺序和 NBT 一致。mixed storage 同时存在时，两边必须表示同一批 actor；库不会把两套额外实体合并成第三份状态。

## Player

Player 物理记录保持区分：

- `level.dat.Player`；
- `~local_player`；
- `player_<raw suffix>`。

底层数据库 key 不要求 `player_` suffix 为 UTF-8；文本 `PlayerId` 只是便利接口，完整工具可通过 raw player-key API 保留任意 key bytes。

普通读写不自动执行玩家存储位置迁移，也不把“只转换 inventory”伪装成整个 Player/世界的目标版本转换。

## BlockEntity

BlockEntity 没有统一的全局 schema version。公共 `BlockEntityRewriter` 用于由调用者根据明确版本证据实现具体规则；未绑定 target version 的便利转换不作为版本兼容保证。

## pre-LevelDB Pocket 世界

`open()` 可以识别历史 `chunks.dat`，并在 world 层叠加 `entities.dat`。

必须区分：

```text
82,176 bytes  Pocket terrain core，没有 biome/RGB tail
83,200 bytes  LevelDB LegacyTerrain，包含完整 1,024-byte biome/RGB tail
```

库不会把 82,176-byte 数据补默认 biome 后伪装成 83,200-byte `LegacyTerrain`。缺失信息继续表现为缺失；需要这些数据的无损转换会拒绝执行。

## 扫描 API

大量数据查询使用面向 world data 的 `query_*` / `scan_*` API。不要引入渲染层命名，例如已经废弃的 `load_render_chunks_with_stats_blocking`；渲染、mesh、atlas、纹理缓存属于 `bedrock-render` 或应用层，而不是 `bedrock-world` 的公共接口语义。

对于高吞吐 key scan，可使用 `PartitionedWorldStorage::scan_keys_partitioned`；worker 持有本地 reduction 状态，避免共享 `Mutex<HashMap<...>>`。

## 完整性与兼容性

兼容级别：

```text
Exact
ReadCompatible
UnsupportedFuture
Corrupt
```

whole-world compatibility/integrity scan 会检查包括：

- malformed / duplicate `digp`；
- dangling actor reference；
- orphan `actorprefix`；
- 同一 actor id 被多个 chunk 引用；
- Pocket terrain core / complete LegacyTerrain；
- malformed terrain length；
- unknown storage key / chunk record；
- 实际 SubChunk version 分布。

`ReadCompatible` 表示能保真读取但不能假装拥有缺失字段；它不等于“可以自动升级”。

## 原子写入

同一 LevelDB 内跨多个 record 的修改必须先完成全部 preflight，再构造一个 `StorageBatch` / transaction 后一次提交，例如：

- `digp` + `actorprefix`；
- SubChunk 多记录写入；
- biome 整批转换；
- `entities.dat` 显式导入；
- chunk 删除及 actor reference 更新。

跨 `level.dat` 和 LevelDB 无法提供真正的单文件系统事务，因此 API 不应把这类操作描述成原子 world conversion。
