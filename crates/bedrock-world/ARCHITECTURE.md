# bedrock-world 架构边界

`bedrock-world` 是建立在 raw storage 之上的通用 Minecraft Bedrock 世界格式库。它负责解释、查询、迁移、审计和安全写入世界数据，但不重新实现 LevelDB。

## 与 bedrock-leveldb 的边界

```text
bedrock-leveldb
    raw bytes / snapshots / scans / batches
                ↓
bedrock-world storage adapter
                ↓
Minecraft Bedrock codecs + models
                ↓
query / migration / edit / audit
```

`bedrock-world` 可以理解 Chunk key、NBT、BlockState 等 Minecraft 语义；`bedrock-leveldb` 不可以。

## 公共职责层

### model

只描述语义数据，不包含数据库 I/O 或编辑策略：

- world/chunk/subchunk；
- BlockState/palette；
- biome/heightmap；
- actor/entity/block entity；
- player/item；
- map/village/global records。

### codec

负责二进制与语义模型的双向转换：

- Bedrock little-endian NBT；
- chunk key；
- LegacyTerrain；
- SubChunk v0/v1/v2-v7/v8/v9；
- palette/storage indices；
- Data2D/Data3D；
- actor/entity record；
- `.mcstructure`。

codec 不决定“是否允许修改旧地图”；它只做严格解析/编码和 raw preservation。

### storage

`WorldStorage` 与具体 backend adapter，只提供 raw world record 的 get/get_many/scan/batch/transaction 边界。

### migration

负责显式版本迁移：

- BlockState migration graph；
- 旧 numeric `id:data` → canonical BlockState；
- LegacyTerrain/pre-paletted → canonical chunk；
- legacy actor storage → modern actor storage；
- `chunks.dat` importer；
- authoritative target palette validation。

任何未知转换必须返回 unresolved/unsupported，禁止猜测。

### edit

只接收 canonical/可安全写模型：

- typed block edit；
- block entity edit；
- chunk replacement；
- structure placement；
- entity/player/global record modifications。

所有写入必须受 `WritePolicy` 控制，并尽量保留未修改、未知字段。

### audit

只读检查：

- compatibility/capability scan；
- integrity audit；
- orphan actor/digest；
- malformed NBT/subchunk；
- legacy/future BlockState；
- unknown/future record preservation 状态。

## 目标源码结构

```text
src/
├── model/
│   ├── world.rs
│   ├── chunk.rs
│   ├── block.rs
│   ├── biome.rs
│   ├── actor.rs
│   ├── player.rs
│   └── records.rs
├── codec/
│   ├── nbt/
│   ├── chunk_key.rs
│   ├── subchunk/
│   │   ├── legacy.rs
│   │   ├── v1.rs
│   │   └── paletted.rs
│   ├── terrain.rs
│   ├── actor.rs
│   ├── biome.rs
│   └── mcstructure.rs
├── storage/
│   ├── traits.rs
│   ├── memory.rs
│   ├── leveldb.rs
│   └── pocket_chunks_dat.rs
├── migration/
│   ├── block_state.rs
│   ├── block_state_graph.rs
│   ├── historical_chunk.rs
│   ├── actor.rs
│   └── legacy_import.rs
├── edit/
│   ├── block.rs
│   ├── chunk.rs
│   ├── block_entity.rs
│   ├── actor.rs
│   └── transaction.rs
├── audit/
│   ├── compatibility.rs
│   └── integrity.rs
├── query/
│   ├── chunk.rs
│   ├── selection.rs
│   └── overlays.rs
├── level_dat.rs
├── discover.rs
└── bedrock_world.rs
```

这是迁移目标，不允许为了快速移动而复制实现。旧模块先通过 facade/`#[path]` 兼容，随后逐步迁移内部依赖。

## 依赖方向

允许：

```text
model
  ↑
codec ← storage
  ↑       ↑
migration
  ↑
edit/query/audit
```

更精确地说：

- model 不依赖 codec/storage/migration/edit/audit；
- codec 可依赖 model，但不依赖 edit/audit；
- storage 不依赖 Minecraft 迁移/编辑策略；
- migration 可依赖 model+codec；
- edit 可依赖 model+codec+storage+migration policy；
- audit 可依赖 model+codec+storage，但不得修改世界。

## 历史兼容原则

库必须支持 mixed world，而不是假设一个 `StorageVersion` 能代表整个数据库。

每条记录/区块根据实际 codec 分类：

```text
Exact
ReadCompatible
MigrationRequired
UnsupportedFuture
Corrupt
```

### 已知历史格式

至少持续覆盖：

- pre-LevelDB Pocket `chunks.dat`；
- LevelDB `LegacyTerrain`；
- pre-paletted SubChunk v0、v2-v7；
- fixed-array/palette transition v1；
- paletted v8/v9；
- legacy inline Entity；
- modern `digp` + `actorprefix`；
- mixed actor storage；
- current Data2D/Data3D/biome formats；
- synthetic future/unknown record preservation。

### 未知未来格式

未知版本不能被当作当前版本：

```text
parse unknown
    ↓
preserve raw bytes
    ↓
mark UnsupportedFuture
    ↓
normal destructive write = refused
```

## 写入策略

`WritePolicy` 必须有清晰语义：

- `Preserve`：仅对可精确 round-trip 的格式执行结构化写入；
- `Migrate`：只有存在显式 migration pipeline 且目标验证通过时迁移；
- `Refuse`：拒绝修改。

不能为了“能保存”而：

- 把 legacy version 直接改成 current；
- 对未知 block metadata 回退 0；
- 对未知 BlockState 自行计算网络 hash；
- 强制覆盖 FinalizedState；
- 清除不认识的 NBT 字段；
- 重写已有 `RandomSeed`。

## 种子所有权

已有地图 `level.dat.RandomSeed` 是地图自身数据：

- 已存在：只能读取和沿用；
- 缺失：显式初始化一次；
- 类型错误/读取失败：报告错误，不回退配置 seed；
- normalize/save 不得偷偷替换 seed。

这样避免旧区块使用原种子、新区块使用另一种子形成地形接缝。

## canonical BlockState

所有上层组件必须共享同一个语义 identity：

```text
identifier + state-name-sorted semantic states
```

NBT compound 原始 insertion order 和 storage version 不参与语义相等判断；storage version 用于 migration capability，而不是 identity。

任何写入目标版本前必须通过 authoritative palette validator。

## 性能原则

`bedrock-world` 不应抵消 `bedrock-leveldb` 的性能优势：

- 一次 key scan 建索引，多消费者复用；
- `get_many` / exact-get 批处理；
- borrowed/raw record 能不复制就不复制；
- palette packed indices 延迟展开；
- surface query 不为每个 subchunk 分配 4096 `u16`；
- worker-local reduction，避免共享 `Mutex<HashMap>` 热锁；
- bounded pipeline，避免大世界扫描无界排队；
- typed edit 按 chunk 聚合后一次 transaction commit；
- 未修改记录原样保留。

## 大文件拆分策略

当前 `chunk.rs`、`world.rs` 已过大。拆分顺序必须低风险优先：

1. 测试/fixture 与生产代码分离；
2. 独立数据模型与纯 codec helper；
3. query/scan/pipeline；
4. transaction/edit staging；
5. 最后才拆核心 chunk decoder/orchestrator。

每一步保持单一实现，禁止复制旧函数再逐步“同步”。

## 版本与兼容

0.x 阶段允许 breaking change，但必须：

- minor 版本提升；
- CHANGELOG 明确迁移方式；
- facade 至少保留一个迁移周期时优先；
- BMCBL/Calcite 等内部消费者在库自身 CI 稳定后再更新 pin。
