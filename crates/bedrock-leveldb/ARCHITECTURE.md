# bedrock-leveldb 架构边界

`bedrock-leveldb` 是 Minecraft Bedrock 使用的 Mojang 修改版 LevelDB 的独立高性能存储引擎。它只处理数据库物理格式和原始字节，不解释 Minecraft 世界语义。

## 职责

允许在本 crate 中实现：

- `CURRENT`、MANIFEST/VersionEdit、WAL/log record、SST/`.ldb` table；
- internal key、sequence number、memtable、immutable memtable；
- table block、index block、restart point、filter/Bloom；
- Mojang 使用的 compression tag，包括 none、Snappy、zlib、raw-deflate 等已验证格式；
- checksum、corruption detection、repair；
- point get、multi-get、prefix/range scan、snapshot、iterator；
- write batch、WAL durability、flush、compaction；
- block/index/filter cache、mmap、buffer pool、零拷贝/borrowed view；
- 并行扫描、持久线程池、批处理和内存复用；
- 纯 raw key/value 的统计和诊断。

## 明确禁止

本 crate 不得出现以下 Minecraft 世界语义：

- Chunk/ChunkPos/Dimension/SubChunk/LegacyTerrain；
- BlockState、Biome、Actor、Entity、BlockEntity、Player；
- `level.dat`、Bedrock NBT 游戏字段；
- `digp`/`actorprefix` 的业务含义；
- Minecraft 版本迁移、方块升级、地图编辑策略。

历史 Minecraft 版本只影响 raw key/value 的内容。只要 Mojang LevelDB 物理格式可读，本 crate 就应无损返回原始字节；具体字节代表什么由 `bedrock-world` 负责。

## 公共 API 分层

新代码优先使用：

```text
bedrock_leveldb
├── engine   Db / Snapshot / Iterator / Stats / Repair / Cache / Compaction
├── access   ReadOptions / Scan / borrowed EntryRef/KeyRef/ValueRef
└── format   WriteBatch / CompressionPolicy / ChecksumMode / WriteOptions
```

crate root 的旧 re-export 仅作为 0.6 迁移期兼容层，后续逐步收紧。

## 目标内部目录

```text
src/
├── engine/
│   ├── db.rs
│   ├── snapshot.rs
│   ├── iterator.rs
│   ├── scan.rs
│   ├── cache.rs
│   ├── flush.rs
│   ├── compaction.rs
│   └── repair.rs
├── format/
│   ├── coding.rs
│   ├── batch.rs
│   ├── wal.rs
│   ├── manifest.rs
│   ├── table/
│   │   ├── reader.rs
│   │   ├── writer.rs
│   │   ├── block.rs
│   │   ├── index.rs
│   │   └── filter.rs
│   └── compression.rs
├── io/
│   ├── file.rs
│   ├── mmap.rs
│   └── buffer_pool.rs
├── options.rs
├── error.rs
└── bedrock_leveldb.rs
```

物理拆分应逐步进行，不能为了目录漂亮复制状态机或产生第二套 table/WAL codec。

## 依赖方向

```text
coding/compression
       ↓
physical format (wal/table/manifest)
       ↓
engine (db/scan/cache/compaction/repair)
       ↓
public facade
```

禁止 format 层依赖 engine；禁止任何层依赖 `bedrock-world`。

## 性能原则

- 热路径优先借用切片/`Bytes`，避免无意义 `Vec`/`String`；
- multi-get 应排序并按 table/block 聚合；
- scan 使用 bounded pipeline 和 worker-local reduction；
- 压缩/解压缓冲区允许池化复用；
- 不为一个 key/value 反复 canonicalize 或复制；
- compaction 与前台读写隔离，避免全局大锁；
- 所有缓存必须有明确容量和统计；
- 优化必须有 benchmark 或至少可重复的统计依据。

## 兼容原则

- 对未知 raw key/value 完全透明；
- 对未知 table compression tag 返回明确 Unsupported/Corruption，绝不猜算法；
- 读取历史数据库不能自动重写；
- repair 操作必须显式调用；
- pre-1.0 breaking API 仍要通过版本号和 CHANGELOG 明确记录。

## 测试要求

至少长期覆盖：

- WAL replay / torn record / checksum failure；
- MANIFEST recovery；
- multi-table lookup；
- compression tag 0/1/2/4；
- snapshot consistency；
- last-write-wins batch；
- flush + reopen；
- compaction + reopen；
- prefix/range scan ordering；
- damaged database repair；
- mmap/non-mmap 等价性（feature 可用时）。
