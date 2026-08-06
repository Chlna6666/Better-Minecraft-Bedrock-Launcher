# bedrock-world

[English](README.md) | [简体中文](README.zh-CN.md)

`bedrock-world` 是基于 `bedrock-leveldb` 的 Minecraft Bedrock 世界库。它提供
快速 `level.dat` 访问、小端 NBT、Bedrock DB key 分类、玩家读取、包含旧 LevelDB
terrain record 的 chunk/subchunk 解析、实体和方块实体解析、物品提取、biome summary，
typed map/village/global record 访问，以及 Bedrock `.mcstructure` 文件导入/导出 helper。

性能设计以 benchmark 为依据：热路径避免保留 owned raw value，在不需要 DOM 时使用
借用式/event NBT view；精确读取默认复用 index/data cache，一次性扫描则可以绕过缓存。

本 crate 专注于完整解析行为；`bedrock-dev/bedrock-level` 项目仅作为解析行为参考。

## 推荐 API

- `read_level_dat(path)` 和 `write_level_dat_atomic(path, document)` 是启动器快路径，
  不会打开 LevelDB。
- `BedrockWorld::open(path, OpenOptions)` 创建 lazy 世界句柄，可自动打开 LevelDB
  或只读旧版 `chunks.dat` 后端，不会解析整张地图。
- `OpenOptions::format` 默认是 `WorldFormatHint::Auto`：检测到 `db/CURRENT`
  时打开 LevelDB，`StorageVersion <= 4` 的早期世界会标记为
  `WorldFormat::LevelDbLegacyTerrain`；只有 `chunks.dat` 时打开
  `WorldFormat::PocketChunksDat`。
- `OpenOptions::default()` 是只读模式。任何世界记录写入都必须用
  `OpenOptions { read_only: false, ..OpenOptions::default() }` 重新打开
  world。只读 world 的高层写入会在访问 storage 前返回
  `BedrockWorldErrorKind::ReadOnly`。
- UI 和工具应优先使用分类 API：
  `classify_keys_blocking`、`list_players_blocking`、
  `list_chunk_positions_blocking`、`parse_chunk_blocking`、
  `parse_subchunk_blocking`、`scan_entities_blocking`、
  `scan_block_entities_blocking`、`scan_items_blocking`、`scan_maps_blocking`、
  `scan_villages_blocking`、`scan_globals_blocking`。
- BedrockLevelFormat 记录写入使用 v0.2 typed API，并且必须在可写 world 上执行：
  `write_map_record_blocking`、`delete_map_record_blocking`、
  `write_global_record_blocking`、`delete_global_record_blocking`、
  `put_heightmap_blocking`、`put_biome_storage_blocking`、
  `put_hsa_for_chunk_blocking`、`delete_hsa_for_chunk_blocking`、
  `put_block_entities_blocking`、`edit_block_entity_at_blocking`、
  `delete_block_entity_at_blocking`、`put_actor_blocking`、
  `delete_actor_blocking`、`move_actor_blocking`、
  `delete_chunk_positions_blocking`。默认 `async` feature 下有同名 async wrapper。
- 高层写入会先 serialize，再 parse 回来校验，然后才提交。actor 写入在一个 transaction
  中同步维护 `digp -> actorprefix`；方块实体写入会校验坐标仍属于目标 chunk。
  `PocketChunksDatStorage` 继续只读。
- LevelDB 后端写入使用带同步的 WAL-backed write options。批量编辑工具如果需要明确的
  后端 compact 边界，可以在一批写入后调用 `compact_storage_blocking`。
- `bedrock-world` 只负责 Bedrock key/value 语义。写入后的刷新、失效和展示策略属于
  下游应用或适配 crate。
- async wrapper 使用 `tokio::task::spawn_blocking`，磁盘 I/O 和解析不会阻塞前台 async runtime。
- `WorldScanOptions` 控制线程、取消和进度回调。
- `McStructureFile::read_from_path`、`McStructureFile::from_world_region_blocking`
  和 `McStructureFile::write_to_world_blocking` 支持 Bedrock `.mcstructure`
  导入、导出和放置；放置支持 chunk anchor、Y 偏移、水平旋转/镜像和方块实体，
  会重算受影响的 heightmap 列，并按每 16 个 chunk 分批提交。
- `WorldPipelineOptions` 进一步控制有界 pipeline 的队列深度、chunk batch 大小、
  subchunk decode worker 预算和进度间隔。字段为 0 时使用自动策略。
- 渲染专用 API 现在有独立快路径：
  `list_render_chunk_positions_blocking`、
  `list_render_chunk_positions_in_region_blocking`、
  `load_render_chunk_blocking`、`load_render_chunks_blocking` 和
  `load_render_region_blocking`。这些接口只读取渲染 chunk 所需记录，并支持有界并行。
- `RenderChunkData` 现在包含 `legacy_terrain: Option<LegacyTerrain>`、
  结构化 `legacy_biomes` 和兼容字段 `legacy_biome_colors`。`LegacyTerrain`
  的 biome 样本按 `[biome_id, red, green, blue]` 解码；兼容 color 统一为
  `0x00RRGGBB`。Exact surface 采样会优先使用保存的 legacy RGB，
  不让冲突的旧 Data2D/Data3D biome id 覆盖它，并在 stats 中记录
  `legacy_biome_preferred_columns`。渲染 exact batch 会请求 `LegacyTerrain` 记录，因此 0.16
  时代的 LevelDB 世界即使没有 `Data2D`/`SubChunkPrefix`，也能被判定为可渲染
  chunk。
- `RenderChunkLoadOptions::request` 选择单一渲染加载契约：`ExactSurface`
  会从真实方块列自顶向下计算 `RenderChunkData::column_samples`，
  包含真实视觉表面方块、relief/支撑方块、可选薄层覆盖物、水体上下文、biome 和来源；`RawHeightMap`
  仅保留原始 heightmap 诊断路径；`Layer`/`Biome` 用于固定切片读取。
- 同时包含 `LegacyTerrain` 和 `SubChunkPrefix` 的过渡 chunk 会保留两类记录；
  渲染器应优先使用 subchunk 方块数据，再把 legacy terrain/biome color 当作 fallback。
- `parse_world_blocking(WorldParseOptions)` 是显式高级/离线接口，不是启动器默认路径。
- 公开 fallible API 返回 `bedrock_world::Result<T>`。应用侧应匹配
  `BedrockWorldError::kind()` 这样的稳定分类，而不是解析面向人的错误字符串。

更完整的 API、测试和 benchmark 说明见 [`docs/API.md`](docs/API.md)、
[`docs/TESTING.md`](docs/TESTING.md) 与
[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md)。

```rust
use bedrock_world::{
    read_level_dat, BedrockWorld, OpenOptions, WorldScanOptions, WorldThreadingOptions,
};

async fn inspect_world() -> bedrock_world::Result<()> {
    let level = read_level_dat("path/to/minecraftWorld/level.dat")?;
    println!("level.dat version={}", level.header.version);

    let world = BedrockWorld::open("path/to/minecraftWorld", OpenOptions::default()).await?;
    let players = world.list_players().await?;
    println!("players={}", players.len());

    let key_counts = world
        .classify_keys(WorldScanOptions {
            threading: WorldThreadingOptions::Auto,
            ..WorldScanOptions::default()
        })
        .await?;
    println!("key categories={}", key_counts.len());

    Ok(())
}
```

## 解析策略

| 策略 | Raw entries | Raw values | Subchunk indices | Actor resolution | 适用场景 |
| --- | ---: | ---: | ---: | --- | --- |
| `WorldParseOptions::summary()` | 否 | 否 | 只保留 counts | 引用到的 actor | UI summary、大型扫描 |
| `WorldParseOptions::structured()` | 选定 parsed entries | 否 | 只保留 counts | 引用到的 actor | 检查工具 |
| `WorldParseOptions::full_raw()` / `full()` | 是 | 是 | 完整 4096 indices | 全部 actor | 调试/离线分析 |

`WorldParseCategories` 控制是否解析 chunks、players、entities、block entities、items、
maps、villages、globals 和 key counts。summary 模式保留统计和结构化摘要，但避免 raw value 驻留。

## 性能模型

- 启动器只需要 `level.dat` 时应使用 `read_level_dat` 和 `write_level_dat_atomic`；
  这条路径只访问 `level.dat` 文件。
- `BedrockWorld::open` 是 lazy 的，DB 访问委托给 `bedrock-leveldb`。
- key 分类使用 key-only scan，不保留 value。
- 视口渲染应先使用 `list_render_chunk_positions_in_region_blocking` 或 async wrapper。
  它会用 key-only prefix scan 探测当前区域，跳过没有渲染记录的 chunk。
- chunk 解析使用 prefix scan 和 LevelDB native block cache；样本 chunk 读取不再做全 table 扫描。
- 默认世界扫描使用自动有界并行 table scan。确定性调试可使用 `WorldThreadingOptions::Single`。
- `RenderChunkLoadOptions::threading` 和 `RenderRegionLoadOptions::threading`
  控制 render chunk/region 并行加载。当外层渲染器已经有 worker pool 时，建议使用
  `Single` 避免嵌套过度并行。
- `RenderChunkLoadOptions::priority` 可以设置
  `RenderChunkPriority::DistanceFrom { chunk_x, chunk_z }`，优先加载当前视口中心附近。
  `RenderRegionData::stats` 会记录请求/命中 chunk、decoded subchunk、worker 数、
  queue wait 和总 load 时间。
- 交互式 tile 渲染应使用 `load_render_chunks_with_stats_blocking` 的 exact
  batch 路径读取 `LegacyTerrain`、biome record、subchunk 和 block entity。
  该路径的 `RenderLoadStats::prefix_scans` 应保持为 `0`，并通过
  `legacy_terrain_records`、`legacy_biome_samples`、`legacy_biome_colors`、
  `terrain_source_legacy`、`terrain_source_subchunk`、`legacy_pocket_chunks`、`detected_format`
  判断旧世界和过渡世界加载情况。
- exact render chunk batch 会在输入打乱、重复或被 `RenderChunkPriority`
  重新排序后仍保持每个 `ChunkPos` 与原始记录绑定。如果渲染器出现 chunk 级错乱，
  应先对照这些 exact-batch stats 和渲染器 placement diagnostics，再修改解析坐标公式。
- 长扫描可通过 `CancelFlag` 取消，并通过 `ProgressSink` 汇报进度。

## 旧版世界格式

`bedrock-world` 通过统一的 `WorldStorage` 抽象支持现代和旧版世界：

- `WorldFormat::LevelDb`：当前 Bedrock LevelDB 世界。
- `WorldFormat::LevelDbLegacyTerrain`：旧 LevelDB 世界，chunk 主体为
  `LegacyTerrain` tag `0x30`。
- `WorldFormat::PocketChunksDat`：旧 Pocket Edition 的 `chunks.dat` 世界。
  `PocketChunksDatStorage` 是只读后端，会把 terrain payload 暴露为虚拟
  `LegacyTerrain` record；写入、删除和 batch 写入返回 unsupported。

```rust
let world = bedrock_world::BedrockWorld::open_blocking(
    "path/to/minecraftWorld",
    bedrock_world::OpenOptions::default(),
)?;
println!("detected format: {:?}", world.format());
```

### 迁移：全量 chunk scan 到视口渲染索引

旧版地图查看器通常等待全世界 chunk scan 完成后再渲染：

```rust
let all_chunks = world
    .list_chunk_positions_blocking(WorldScanOptions::default())?;
let visible = all_chunks
    .into_iter()
    .filter(|pos| viewport.contains(*pos))
    .collect::<Vec<_>>();
```

现在优先查询当前视口对应的渲染区域：

```rust
let visible = world.list_render_chunk_positions_in_region_blocking(
    bedrock_world::RenderChunkRegion {
        dimension,
        min_chunk_x,
        min_chunk_z,
        max_chunk_x,
        max_chunk_z,
    },
    WorldScanOptions {
        threading: WorldThreadingOptions::Auto,
        cancel: Some(cancel),
        progress: Some(progress),
        ..WorldScanOptions::default()
    },
)?;
```

随后只加载这些 chunk：

```rust
let chunks = world.load_render_chunks_blocking(
    visible,
    bedrock_world::RenderChunkLoadOptions {
        threading: WorldThreadingOptions::Fixed(4),
        priority: bedrock_world::RenderChunkPriority::DistanceFrom {
            chunk_x: viewport_center_x,
            chunk_z: viewport_center_z,
        },
        ..bedrock_world::RenderChunkLoadOptions::default()
    },
)?;
```

## Fixture 结果

`tests/fixtures/sample-bedrock-world` 是本地可选的大型 Bedrock 世界 fixture，
包含 native `.ldb` table 和 WAL 数据。它被 Git 忽略，因为真实世界体积很大，
并且可能包含玩家数据。缺少该目录时，fixture test 和大型 benchmark 会打印 skip
信息并正常通过。

```text
cargo test -p bedrock-world -- --nocapture

unit tests: 36 passed; 0 failed; finished in 0.03s
fixture test: 1 passed; 0 failed; finished in 15.85s

db.entries.count=4571643
db.entries.key_bytes=63624175
db.entries.value_bytes=8398184492
db.chunk.positions.count=237534
db.unknown_keys.first=[]
parsed.sample_chunk.pos=ChunkPos { x: 451, z: -457, dimension: End }
parsed.sample_chunk.records=10
parsed.sample_chunk.subchunks=5
parsed.sample_chunk.subchunk_storages=4
parsed.sample_chunk.palette_states=10
parsed.sample_chunk.block_entities=0
parsed.sample_chunk.biomes.records=1 storages=25
parsed.sample_chunk.errors=[]
players.count=290
```

## Benchmark 结果

最新本地 Criterion 和大型 fixture 结果记录在
[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md)。大型 fixture harness 与 Criterion
分离，因为多百万 entry 扫描不应该在 microbenchmark 中反复运行。

## Features 和 docs.rs

docs.rs 会启用全部 features 构建，因此托管 API 文档会包含 async wrapper 和可选的
`bedrock-leveldb` 后端。

| Feature | 默认 | 含义 |
| --- | --- | --- |
| `async` | 是 | 添加 async wrapper，并把阻塞的文件系统、LevelDB 和 NBT 工作交给 `tokio::task::spawn_blocking` |
| `backend-bedrock-leveldb` | 是 | 通过 `bedrock-leveldb` 打开原生 Bedrock LevelDB 世界 |
| `leveldb-mmap` | 否 | 启用后端并转发 `bedrock-leveldb/mmap` feature |

只需要纯解析、内存 storage、`level.dat` 或 NBT helper 的工具可以关闭默认 features。
crates.io 包包含英文/中文 README、`docs/` 下的指南、changelog、许可证、源码、
测试、fixture 文档和 benchmark。

## 完整度

| 范围 | 状态 |
| --- | --- |
| `level.dat` header、warning、原子写入 | 已实现 |
| Bedrock 小端 NBT 和连续 root | 已实现 |
| DB key 分类 | 已覆盖 chunk、player、actorprefix、digp、map、village、local player alias 和常见 global key |
| 旧 `LegacyTerrain` record | 已实现 83,200 字节 LevelDB 时代 terrain value 解析 |
| 旧 subchunk block array | 已实现 v0 和 v2-v7 pre-paletted `SubChunkPrefix` value |
| Subchunk v1/v8/v9 palette 解析 | 已实现 counts-only 和 full-indices 模式 |
| Data2D/Data3D biome 和 heightmap codec | 已实现 |
| HSA、map、global、actor、block entity 写入 | 已实现，带 roundtrip 校验 |
| `digp -> actorprefix` actor 解析 | 已实现，可配置，并支持事务式现代 actor 写入 |
| 玩家、实体、方块实体、物品栈 | 已提取通用字段 |
| 未知版本专属数据 | 按 retention mode raw 保留或计数 |
| 所有 chunk 版本的完整结构化编辑 | 未实现 |
| 地图像素记录解析 | 已实现 |

早于 LevelDB 的 Bedrock 世界使用 `chunks.dat` / `entities.dat` 之类文件，不是数据库世界；
这类文件导入目前不属于本 crate 的范围。

## 使用建议

- 不要在 UI 中调用 `parse_world_blocking(WorldParseOptions::full_raw())`；它是离线调试路径。
- 启动器元数据使用 `read_level_dat`，安全修改使用 `write_level_dat_atomic`。
- 只需要某一类数据时使用对应分类 API，避免不必要地解析实体、区块和 global records。
- 渲染器应先构造视口 `RenderChunkRegion`，再使用 render-index API。完整
  `list_chunk_positions_blocking` 保留给元数据、搜索和离线导出场景。
- 大批量写入完成后，如需在交还世界给其它进程前主动触发 LevelDB compact，可显式调用
  `compact_storage_blocking`。
- 可选的 `bedrock-leveldb` 后端使用 versioned dependency 供 crates.io 发布，
  并保留本仓库开发用的 `../bedrock-leveldb` 本地路径。
