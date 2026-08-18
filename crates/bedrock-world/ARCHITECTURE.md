# bedrock-world 架构边界

`bedrock-world` 是 Minecraft Bedrock 世界文件读写库。它直接面向 Bedrock 实际存在的数据：`level.dat`、LevelDB 世界记录、`LegacyTerrain`、SubChunk、Data2D/Data3D、`Entity`、`digp`、`actorprefix`、Player、BlockEntity、SavedItem、Map、Village 与 `.mcstructure`。

它不是“世界升级器”。普通读取、编辑和写回与版本升级/降级完全分离。

## 与 bedrock-leveldb 的边界

```text
Minecraft Bedrock world folder
        │
        ├─ level.dat
        ├─ db/
        │    └─ Mojang LevelDB
        └─ historical chunks.dat
                 │
                 ▼
          bedrock-world
                 │
                 └─ Bedrock world semantics

bedrock-leveldb 只负责 Mojang 修改版 LevelDB 的数据库机制。
```

`bedrock-world` 可以理解 Chunk key、Dimension、SubChunk、BlockState、Actor、Player、Biome 等 Minecraft 语义；`bedrock-leveldb` 不应包含这些 Minecraft 规则。

## 开发者入口

只知道地图文件夹路径时，优先使用：

```rust
let world = bedrock_world::BedrockWorld::open_auto_blocking("world")?;
let versions = world.versions_blocking()?;
```

`open_auto_blocking()` 自动识别当前目录使用的世界数据：

- Bedrock LevelDB；
- 包含 `LegacyTerrain` 的旧 LevelDB 世界；
- pre-LevelDB Pocket Edition `chunks.dat`。

`versions_blocking()` 从实际文件和数据库记录收集版本证据，不把整个世界强行归纳成一个假的单一版本。它包含：

- `level.dat` header version；
- `StorageVersion`；
- `lastOpenedWithVersion`；
- `MinimumCompatibleClientVersion`；
- `InventoryVersion`；
- SubChunk V0-V9/未知版本的实际记录数量；
- `LegacyTerrain`；
- `Data2D` / `Data2DLegacy` / `Data3D`；
- chunk `Entity`；
- `digp` / `actorprefix`。

因此 mixed-version 世界是正常输入，不是异常状态。

## 普通读写

普通 API 的职责只有：

```text
识别实际 Bedrock 数据
        ↓
严格解析
        ↓
调用者读取/编辑
        ↓
按该数据本身能够表达的形式写回
```

普通读写不得隐式执行：

- SubChunk V0-V7 → V8/V9；
- `LegacyTerrain` → SubChunk；
- Data2D → Data3D；
- `Entity` → `digp`/`actorprefix`；
- numeric SavedItem → named SavedItem；
- BlockState 版本更新；
- Player 历史字段归一化；
- `level.dat` 游戏版本号更新。

如果某个特定编辑 API 只实现 V8/V9，它应明确拒绝其他 SubChunk，而不是调用升级流程。

## 升级与降级

升级和降级是两套独立操作。

```rust
let upgrade = world.upgrade_plan_blocking(target_version)?;
let downgrade = world.downgrade_plan_blocking(target_version)?;
```

### upgrade

升级按目标 Bedrock 版本检查可能需要的实际数据变化，例如：

- `LegacyTerrain` → 新地形记录；
- SubChunk → 目标 SubChunk version；
- Data2D/Data2DLegacy → Data3D；
- `Entity` → `digp` + `actorprefix`；
- historical SavedItem / BlockState；
- 最后更新 `level.dat` 中对应的版本数据。

### downgrade

降级不允许“把升级逻辑倒着执行”。它独立检查旧版本能否表达当前数据，例如：

- V9/V8 → 更早 SubChunk；
- SubChunk → `LegacyTerrain`；
- Data3D → Data2D；
- `digp` + `actorprefix` → chunk `Entity`；
- named/BlockState SavedItem → historical item representation；
- 目标版本 block palette / numeric ID+meta 是否存在。

`DowngradePlan` 必须单独报告潜在数据损失，例如 3D biome 降到 2D、目标版本不存在的 BlockState、需要 historical numeric ID/meta 等。

任何实际升级/降级执行都必须先完成完整计划和目标数据验证；不能半途修改世界后才发现目标不可表达。

## Chunk 与 SubChunk

SubChunk 使用实际持久化 version byte：

```text
V0
V1
V2
V3
V4
V5
V6
V7
V8
V9
Unknown(u8)
```

读取自动从 payload 识别版本。

同版本普通写回应尽量保持原表示。调用者明确选择另一个版本时，才进入对应的升级或降级操作。

未知 V10+ 等未来版本：

```text
读取 version byte
        ↓
保留 raw payload
        ↓
标记 UnsupportedFuture
        ↓
禁止不了解该数据的结构化重写
```

不能把未知版本当成 V9，也不能只改 version byte。

## BlockState 与 SavedItem

BlockState 的 `version` 是 Bedrock 实际保存的数据版本，不是库内部版本号。

SavedItem 不拥有一个可通用于所有历史时期的单一 Mojang item version，因此版本识别依赖实际保存形态和世界版本证据：

- numeric `id` + meta；
- string identifier；
- persisted `Block` BlockState；
- mixed representation。

普通 Item/Player 读取不得自动更新这些表示。

升级使用权威历史规则正向处理；降级必须有目标版本可表达性和反向映射数据，不能根据正向规则猜逆映射。

## Player

Player 直接对应实际存储来源：

- `level.dat.Player`；
- `~local_player`；
- `player_<id>`。

Player 数据按真实 NBT 字段提供访问，例如：

- `Pos` / `Motion` / `Rotation` / `DimensionId`；
- Spawn fields；
- `Inventory` / `EnderChestInventory`；
- `Armor` 和 Inventory slots `100..=103`；
- `Offhand` / `OffHandItem`；
- `abilities`；
- `Attributes`；
- `ActiveEffects`；
- `PlayerLevel` / `PlayerLevelProgress`；
- `PlayerGameMode`。

如果同一玩家同时存在多种历史表示，应把它们分别暴露给调用者，不要在读取阶段偷偷选择一个作为“现代真值”。

未修改 Player 应尽量原 bytes 写回，以保留未知字段和未来数据。

## Actor

Actor 的实际持久化形式包括：

```text
chunk Entity
```

以及：

```text
digp<ChunkKey>
    ↓ ActorUniqueID
actorprefix<ActorUniqueID>
```

两种形式都属于 Bedrock 历史数据。普通读取可以识别 mixed actor storage；只有显式 upgrade/downgrade 才改变其存储形式。

## Biome

支持的实际 Bedrock 数据包括：

- `Data2D`；
- `Data2DLegacy`；
- `Data3D`；
- `LegacyTerrain` 内历史 biome sample。

Data2D 和 Data3D 之间的变化属于明确的升级/降级，不是普通 biome write 的默认行为。

## 源码组织原则

目录和文件名优先使用 Minecraft Bedrock 实际概念，不创建含义过宽的总桶。

当前主要结构：

```text
src/
├─ bedrock_world.rs
├─ world/
│  ├─ bedrock_world.rs
│  ├─ level_dat.rs
│  ├─ upgrade.rs
│  ├─ downgrade.rs
│  ├─ discover.rs
│  └─ pocket_chunks_dat.rs
├─ chunk/
│  ├─ subchunk.rs
│  ├─ version.rs
│  └─ ...
├─ block/
├─ biome/
├─ entity/
├─ player/
│  ├─ data.rs
│  ├─ inventory.rs
│  ├─ equipment.rs
│  ├─ abilities.rs
│  ├─ attributes.rs
│  ├─ effects.rs
│  ├─ position.rs
│  ├─ spawn.rs
│  ├─ experience.rs
│  └─ game_mode.rs
├─ item/
├─ level/
├─ map/
├─ database/
└─ ...
```

`world/bedrock_world.rs` 目前仍包含较多方法；应继续按实际 Bedrock 数据拆到 `world/chunk.rs`、`world/player.rs`、`world/actor.rs`、`world/map.rs`、`world/village.rs` 等，而不是再创建 `access`、`manager`、`service` 一类总桶。

## 完整性判断

兼容性只回答“当前保存的数据能否安全读取/按其自身表示写回”，不回答“是否应该升级”。

```text
Exact
ReadCompatible
UnsupportedFuture
Corrupt
```

其中：

- 已实现读写的历史格式可以是 `Exact`；
- `ReadCompatible` 表示可以安全读取，但写入需要保留 raw；
- `UnsupportedFuture` 表示遇到库未知的新格式；
- `Corrupt` 表示数据本身损坏或内部矛盾。

不存在 `MigrationRequired` 这种普通兼容性状态。

## 数据保留原则

不能为了“能保存”而：

- 把历史 SubChunk version 直接改成当前 version；
- 对未知 block metadata 回退到 0；
- 删除不认识的 NBT 字段；
- 覆盖已有 `FinalizedState`；
- 替换已有 `RandomSeed`；
- 将未知未来记录重新编码成已知格式；
- 在降级时静默丢弃目标版本无法表达的数据。

## 性能原则

`bedrock-world` 不应抵消 `bedrock-leveldb` 的性能优势：

- 地图目录只解析需要的文件；
- key scan 尽量一次完成并复用结果；
- `get_many` / exact-get 批处理；
- raw record 能不复制就不复制；
- packed palette indices 延迟展开；
- surface query 不为每个 SubChunk 无条件分配 4096 个索引；
- worker-local reduction，避免共享热锁；
- bounded pipeline，避免大世界扫描无界排队；
- 编辑按 Chunk 聚合再提交；
- 未修改记录尽量 byte-exact 保留。
