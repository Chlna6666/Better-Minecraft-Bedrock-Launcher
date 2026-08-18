# bedrock-world 架构边界

`bedrock-world` 是 Minecraft Bedrock 世界文件读写库。它面向游戏真实持久化数据：`level.dat`、Mojang LevelDB、pre-LevelDB Pocket `chunks.dat` / `entities.dat`、`LegacyTerrain`、SubChunk、Data2D/Data3D、Actor、Player、BlockEntity、SavedItem、Map、Village 与 `.mcstructure`。

核心原则是：**读取保存事实，普通编辑保持原表示，跨版本写出必须选择具体数据目标并完成可表达性证明。**

## 与 bedrock-leveldb 的边界

```text
Minecraft Bedrock world folder
        │
        ├─ level.dat
        ├─ db/
        │    └─ Mojang LevelDB
        ├─ historical chunks.dat
        └─ historical entities.dat
                 │
                 ▼
          bedrock-world
                 │
                 └─ Bedrock world semantics

bedrock-leveldb
        └─ WAL / SST / MANIFEST / checksum / compression / cache / compaction
```

`bedrock-leveldb` 不理解 Chunk key、Dimension、NBT、BlockState、Actor 或 Player。`bedrock-world` 不应重新实现数据库引擎机制。

正式 Bedrock LevelDB 写出默认使用 compression id `4` 的 raw DEFLATE；id `2` 的 zlib framing 仍作为显式兼容策略保留。

## 世界打开与版本证据

只知道地图目录时优先：

```rust
let world = bedrock_world::BedrockWorld::open_auto_blocking("world")?;
let versions = world.versions_blocking()?;
```

`open_auto_blocking()` 区分：

- 标准 Bedrock LevelDB；
- 含 `LegacyTerrain` 的旧 LevelDB；
- pre-LevelDB Pocket `chunks.dat`，并由 world 层叠加 `entities.dat`。

Pocket terrain 有两个必须区分的真实长度：

```text
82,176 bytes  pre-LevelDB Pocket terrain core，没有 biome/RGB tail
83,200 bytes  LevelDB LegacyTerrain，包含 1,024-byte biome/RGB tail
```

库不得把 82,176 bytes 补默认 biome 后伪装成 83,200 bytes。缺失字段必须继续表现为缺失。

`versions_blocking()` 收集实际证据，而不是把 mixed-version 世界强行归纳成一个内部 schema：

- `level.dat` header version；
- `StorageVersion`；
- `lastOpenedWithVersion`；
- `MinimumCompatibleClientVersion`；
- `InventoryVersion`；
- SubChunk V0-V9/未知版本数量；
- `LegacyTerrain`；
- `BlockExtraData`；
- `Data2D` / `Data2DLegacy` / `Data3D`；
- chunk `Entity`；
- `digp` / `actorprefix`。

## 普通读写

普通读写只做：

```text
识别实际 Bedrock 数据
        ↓
严格解析
        ↓
调用者读取/编辑
        ↓
按同一物理记录和同一持久化表示写回
```

不得隐式执行：

- SubChunk V0-V7 → V8/V9；
- `LegacyTerrain` → SubChunk；
- Data2D → Data3D；
- `Entity` → `digp`/`actorprefix`；
- numeric SavedItem → named SavedItem；
- BlockState schema 更新；
- Player 历史字段归一化；
- `level.dat.Player` ↔ `~local_player` 物理迁移；
- `level.dat` 游戏版本号更新。

未修改的 Player、未知 NBT、未来记录应优先 byte-exact 保留。

## 跨版本操作：具体数据目标，不使用通用 Plan

库不再提供 `UpgradePlan` / `DowngradePlan` 一类泛化世界计划。原因是不同 Bedrock 数据对象的版本边界并不一致，把它们抽象成一个统一 action 列表容易把“局部转换”误当成“完整世界版本转换”。

现在的设计是每个真实数据对象自行定义严格的 preflight 和 write：

### SubChunk

- `upgrade_subchunks_blocking(target_game_version, upgrade_data, target_palette)`：目标游戏版本明确，BlockState 升级数据与目标 vanilla palette 必须匹配。
- `write_subchunks_as_legacy_numeric_blocking(target_subchunk_version, numeric_table)`：目标固定数组 SubChunk version 明确，并要求经过正向验证的历史 numeric ID/meta 表。
- 未知 SubChunk version、无 version byte、实验期无法唯一确定目标表示时直接拒绝。

### Biome

- `Data2D/Data2DLegacy → Data3D` 必须显式传入目标 `ChunkVersion::Old/New`，不能默认假设 Caves & Cliffs 后高度。
- `Data2DLegacy` 的 saved RGB 若要进入无 RGB 字段的 Data3D，必须显式确认损失。
- `Data3D → Data2D` 只有在每个 `(x,z)` 列所有垂直 biome 完全一致且 id 可表示时才允许；否则拒绝，不做有损折叠。

### Actor

真实表示只有：

```text
chunk Entity
```

以及：

```text
digp<ChunkKey>
    ↓ Actor storage id
actorprefix<Actor storage id>
```

转换必须精确保持 actor 顺序和 NBT。若 mixed storage 同时存在，两套表示必须完全一致；禁止把两边额外 actor 合并成第三份状态。

### Player / SavedItem

Player 物理记录保持区分：

- `level.dat.Player`；
- `~local_player`；
- 原始 `player_<suffix>` key，suffix 不假设为 XUID，也不要求 UTF-8。

SavedItem 的实际持久化代际：

- Classic：MCPE <= 1.5 numeric `TAG_Short id` + `Damage`；
- Medieval：MCPE 1.6-1.8 string name + metadata；
- Modern：MCPE 1.9+ string name，并可包含 persisted `Block` BlockState。

world scope 只提供 representability/preflight。不能只把 Player inventory 降级后原地写回仍标记为新版本的源世界。实际转换发生在 owned `PlayerData` / item 对象上，再交给完整具体目标版本 writer/export。

已确认的 MCPE 0.6.1 Player writer 是这种“具体目标写出”的例子。

### BlockEntity

BlockEntity 没有一个所有实体统一共享的 schema version。公共接口提供 `BlockEntityRewriter`，调用者用明确版本证据实现具体规则；没有 target 绑定的内置便利转换不作为公共兼容接口。

## SubChunk

读取以真实 payload version byte 为准：

```text
V0 V1 V2 V3 V4 V5 V6 V7 V8 V9 Unknown(u8)
```

未来版本处理：

```text
读取 version byte
        ↓
保留 raw payload
        ↓
标记 UnsupportedFuture
        ↓
禁止不了解该表示的结构化重写
```

绝不能把未知版本当 V9，也不能只改 version byte。

## BlockState 与历史 numeric 映射

BlockState 的 `version` 是游戏真实保存的数据版本，不是库内部 schema id。

升级必须使用权威历史规则正向处理；反向写历史 numeric ID/meta 必须通过“候选历史值 → 正向升级 → 与当前 semantic BlockState 精确相等”的方式证明，不能把正向 rename 表直接反转后猜结果。

## 完整性与兼容性

兼容性回答“当前保存的数据能否安全读取/按自身表示写回”，不回答“是否应该升级”：

```text
Exact
ReadCompatible
UnsupportedFuture
Corrupt
```

whole-world compatibility scan 还会报告：

- malformed / duplicate `digp`；
- dangling actor reference；
- orphan `actorprefix`；
- 同一 actor id 被多个 chunk 引用；
- Pocket 82,176-byte terrain core 与完整 83,200-byte `LegacyTerrain` 数量；
- malformed terrain length；
- unknown storage keys / unknown chunk records；
- 实际 SubChunk version 分布。

结构性 actor 引用错误和非法 terrain 长度属于 `Corrupt`；孤儿 actor payload、Pocket 缺 biome tail 等可读取但不能假装完整写回的数据属于 `ReadCompatible`。

## 原子性

所有跨记录写操作先完成完整 preflight，再构造一个原子 batch/transaction：

- Actor `digp` + `actorprefix`；
- `entities.dat` 导入；
- SubChunk 多记录升级；
- biome 整世界转换；
- chunk 删除及其 actor references。

禁止“按 N 个 batch 分批提交，再假设后续不会失败”的伪事务语义。

跨 `level.dat` 与 LevelDB 的真正单文件系统事务不可实现；因此这类跨容器操作不能伪装成原子世界转换。

## 源码组织

目录按 Minecraft Bedrock 实际概念拆分，不建立 `manager/service/migration-all` 之类总桶：

```text
src/
├─ bedrock_world.rs
├─ database/
│  ├─ storage_v2.rs
│  └─ pocket_chunks.rs
├─ world/
│  ├─ bedrock_world.rs
│  ├─ level_dat.rs
│  ├─ legacy_terrain.rs
│  ├─ biome_upgrade.rs
│  ├─ biome_downgrade.rs
│  ├─ subchunk_upgrade.rs
│  ├─ subchunk_numeric.rs
│  ├─ pocket_chunks_dat.rs
│  ├─ pocket_entities_dat.rs
│  └─ pocket_world_storage.rs
├─ chunk/
├─ block/
├─ biome/
├─ entity/
├─ player/
├─ item/
├─ integrity/
├─ level/
├─ map/
└─ query/
```

## 数据保留原则

不能为了“能保存”而：

- 给 Pocket terrain 编造 biome/RGB；
- 把未知 SubChunk version 直接改成当前 version；
- 对未知 block metadata 回退到 0；
- 删除不认识的 NBT 字段；
- 把两个 mixed actor 表示合并成新状态；
- 将未知未来记录重新编码成已知格式；
- 在降级时静默丢弃目标版本无法表达的数据；
- 只转换某个 Player/Item/Chunk 子系统后宣称整个世界已降到目标游戏版本。

## 性能原则

`bedrock-world` 不应抵消 `bedrock-leveldb` 的性能优势：

- key scan 尽量一次完成并复用；
- `get_many` / exact-get 批处理；
- raw record 能不复制就不复制；
- packed palette indices 延迟展开；
- surface query 不为每个 SubChunk 无条件展开 4096 indices；
- worker-local reduction，避免共享热锁；
- bounded pipeline，避免大世界扫描无界排队；
- 编辑按 Chunk 聚合后原子提交；
- 未修改记录 byte-exact 保留；
- LevelDB block cache 分离 data/index/file 容量；
- native table 写出使用 Bedrock raw-DEFLATE 路径。
