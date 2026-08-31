# bedrock-world Agent Instructions

本文件适用于 `crates/bedrock-world/**`，并在该目录范围内补充仓库根 `AGENTS.md`。
`bedrock-world` 当前处于 dev 阶段：API 重命名与模块重构默认采用破坏性迁移，仓库内调用方必须同批迁移，不保留旧接口兼容层。

## 命名事实源与决策顺序

命名按以下顺序决定：

1. Minecraft Bedrock 已存在的领域名称、持久化名称和协议名称；
2. Rust 标准库与 Rust API Guidelines 的惯用表达；
3. 本 crate 的模块路径、receiver、参数和返回类型已经提供的上下文；
4. 最短且仍然准确、稳定、无歧义的名称。

不要为了“解释完整”把签名、线程模型、实现步骤或整个控制流序列化进名称。名称描述稳定领域语义，不描述当前实现。

## Minecraft Bedrock 领域词

以下名称属于稳定领域概念，除非语义确实不同，否则优先直接使用，不自行改写为通用软件分层词：

- `World`、`Dimension`、`Chunk`、`SubChunk`、`Block`、`BlockState`、`BlockEntity`；
- `Biome`、`Entity`、`Actor`、`Player`、`Item`、`Structure`；
- `LevelDat` / `level.dat`、`LevelDB`、`LegacyTerrain`、`Data2D`、`Data3D`；
- Bedrock 实际记录名和 key 语义，例如 `digp`、`actorprefix`、`SubChunkPrefix`；
- 只有 Minecraft/Bedrock 本身存在版本或表示差异时，才在名称中保留 `Legacy`、`Pocket`、`V8`、`V9` 等限定。

crate 名已经是 `bedrock-world`，因此 crate 内公共根类型和 API 不重复 `BedrockWorld*`。例如优先：

```rust
World
OpenOptions
WorldFormat
BlockState
ChunkPos
```

而不是：

```rust
BedrockWorld
BedrockWorldOpenOptions
BedrockWorldChunkResult
```

当类型在更宽的 BMCBL 根命名空间中会发生真实冲突时，才增加必要限定词；不能仅因为“更明确”就重复父模块或 crate 名。

## Rust API 命名

### 同步与异步

`bedrock-world` 的底层 LevelDB/NBT/SubChunk 操作以同步实现为 canonical API。同步函数和方法使用短语义名，不添加 `_blocking`：

```rust
world.chunk(pos)
world.subchunk(pos, y)
world.block_state(dimension, pos)
world.block_states(dimension, positions)
world.player(id)
world.players()
world.read_level_dat()
world.apply_block_edits(...)
```

只有 `tokio::task::spawn_blocking` 等异步适配器才添加 `_async`：

```rust
world.chunk_async(pos).await
world.players_async().await
```

不得让 async wrapper 占据短名、迫使同步核心使用 `_blocking`。

### Getter、集合与谓词

普通 getter 不使用 `get_`；receiver 已表达对象时直接使用名词：

```rust
world.chunk(pos)
world.block_state(dimension, pos)
world.player(id)
```

只有真正的映射/keyed lookup 语义才使用 Rust 惯用的 `get` / `get_mut`。

集合读取优先使用集合名本身，例如 `players()`、`chunk_positions()`；不要机械使用 `list_players()`、`list_chunk_positions()`。

布尔方法使用 `is_`、`has_`、`can_`、`should_`，且只用于谓词。

### Options、conditions 与控制流

不要通过 `with_options`、`with_control`、`if_*`、`using_*` 等函数名排列组合参数。可配置行为应由 typed options / conditions 表达，并保留一个 canonical operation。

禁止这类句子型 API：

```rust
prepare_block_edits_if_primary_states_match_blocking(...)
get_many_ordered_with_control(...)
audit_world_integrity_blocking(...)
```

应收敛为 operation + typed argument，例如：

```rust
prepare_block_edits(..., conditions, options)
audit_world_integrity(..., options)
```

条件本身使用领域类型表达，例如 `BlockStateCondition`，不要把条件内容复制到函数名。

## 禁止实现细节式命名

文件、模块、类型和公开 API 默认禁止无实际领域含义的后缀/前缀：

- `_blocking`、`_sync`（同步本来就是 canonical 时）；
- `_repo`、`Repository`；
- `Service`、`Manager`、`Controller`、`Helper`、`Helpers`；
- `Operations`、`Utils`、`Common`；
- 无额外语义的 `Data`、`Info`、`Result`、`Record`、`Object`；
- 已由父模块或 receiver 提供的 `World`、`Bedrock`、`Storage`、`Query` 重复前缀。

这些词不是语法禁令；如果它们确实是 Minecraft 名称、Rust 标准概念或表达不可由路径推断的稳定区别，可以保留。任何保留都必须能回答“去掉这个词会丢失什么真实语义”。

## 模块与文件布局

模块按真实职责或 Minecraft 对象命名。优先：

```text
world/
storage/
chunk/
block/
biome/
entity/
player/
item/
structure/
level/
nbt/
editor/
query/
integrity/
```

`database/` 不作为 `bedrock-world` 的世界存储层名称。Mojang LevelDB 引擎实现属于 `bedrock-leveldb`；本 crate 中保存 raw Bedrock records、backend adapter 和 transaction contract 的层统一称为 `storage`。

父目录已经表达上下文时，叶文件不得重复父目录或 crate 名，例如避免：

```text
world/bedrock_world.rs
storage/storage.rs
query/query_types.rs
world/world_records.rs
```

应按真实职责收敛为短名称或重新组合职责。

标准 Rust 模块布局使用 `mod name;` / `pub mod name;` 与 `name.rs` 或 `name/mod.rs`。功能源码禁止使用 `#[path = "..."]` 绕过模块树。

`#[path]` 仅允许在以下边界，并必须在代码旁说明普通模块系统为何不能表达：

- 明确的测试 fixture/module 装配；
- build script / generated-code 接入；
- 极少数必须选择不同物理实现文件的 target-specific shim。

Minecraft world、chunk、storage、editor、query、renderer 等功能模块不得使用 `#[path]`。

`include!` 只用于构建生成 Rust 代码；静态资源使用 `include_str!` / `include_bytes!`。

## Dev 阶段破坏性迁移

当前不维护旧 API 兼容性。重命名或职责迁移时：

- 直接删除旧函数、旧 type alias、旧 re-export、deprecated wrapper 和 forwarding module；
- 不保留 `old_name -> new_name` 的隐藏转发；
- 不为旧模块路径增加兼容 `pub use`；
- 同一个概念最终只有一个公开名称和一条权威路径；
- BMCBL 应用、`bedrock-render`、测试、bench、examples、文档及其它 workspace 调用点必须同批迁移；
- 外部 dev 使用方（例如 Calcite）在新的 `bedrock-world` revision 落地后同步升级，不通过兼容层维持旧调用。

## 审计与验证门槛

每轮命名整改至少检查：

```text
_blocking
get_
list_
with_options
with_control
_repo
Repository
Helper / Helpers
Operations
#[path =
crate::database
bedrock_world::database
```

搜索结果必须逐项判断，不能机械替换 Minecraft 自身术语或 Rust 标准接口。

修改完成后需要同时验证：

1. `bedrock-world` 默认 feature；
2. `bedrock-world` 的 `bedrock-leveldb` feature；
3. async feature（如受影响）；
4. BMCBL 根应用受影响 target；
5. `bedrock-render` 及其它 workspace 调用方；
6. 测试/bench/examples；
7. 对旧 symbol/path 再搜索，确认没有残留兼容入口。

编译通过只说明类型层一致；Minecraft 数据语义变更还必须保持原始 Bedrock 表示、未知记录保留策略、版本边界和写入原子性不变。
