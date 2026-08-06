# 开发指南

[English](DEVELOPMENT.md) | [简体中文](DEVELOPMENT.zh-CN.md)

本文档面向 `bedrock-leveldb` 的维护者和贡献者。README 说明面向用户的行为；
本指南说明如何在不削弱存储保证、兼容边界、诊断质量和发布质量的前提下修改本
crate。

## 项目边界

`bedrock-leveldb` 是 raw key/value 存储层 crate。它读取 Bedrock/native
LevelDB 文件，并暴露 raw key 与 raw value。不要在本 crate 中加入 NBT 解释、
玩家记录、actor 模型、chunk block state 语义、实体数据或世界编辑工作流。这些
属于应用层或更高层的领域 crate。

本 crate 是 raw native LevelDB engine 层。native manifest、WAL 和 table
文件会被直接读取。v0.2 写入标准 LevelDB WAL batch，flush 原生 `.ldb` table，
并持久化 manifest version edit。旧的 `BWLDB...` 文件仅作为迁移和向后兼容读取
保留；新增写入路径不能再创建自定义格式。

`bedrock.rs` 中的 Bedrock helper 只处理存储布局。它们可以解析文档化的
LevelDB 时代 chunk key，以及 `LegacyTerrain`、pre-paletted `SubChunkPrefix`
value 等旧 payload 布局。它们不应解析 pre-LevelDB 的 `chunks.dat` /
`entities.dat` 文件，也不应解析游戏语义层面的记录内容。

## 模块导览

- `bedrock_leveldb.rs`：crate root、公开 re-export、crate-level 文档和 lint
  策略。
- `db.rs`：`Db`、open/recovery 流程、point read、scan、overlay、snapshot、
  flush、repair 入口和 read-only 约束。
- `manifest.rs`：native manifest 解析、version edit，以及用于 range filtering 的
  table metadata。
- `table.rs`：table footer、index/data block 读取、restart array、解压、
  internal key 处理和 native table 写入。
- `wal.rs`：LevelDB WAL record framing、fragmentation、checksum 和 padding。
- `coding.rs`：varint、固定宽度编码 helper 和 CRC masking。
- `options.rs`：open/read/write/scan option，以及 progress/cancel plumbing。
- `error.rs`：结构化 `LevelDbError`、`ErrorKind`、path/source/context helper 和
  display 格式。
- `batch.rs`：write 和 WAL 路径使用的公开 write batch 表示。
- `bedrock.rs`：文档化的 Bedrock LevelDB key 和旧存储布局 helper。

模块默认保持私有。只有明确属于稳定支持面的类型才从 crate root 暴露，并且必须
有 rustdoc 说明用途和错误行为。

## 开发环境

使用 Rust 1.87 或更新版本。本 crate 使用 edition 2024，并把 Rust 1.87 作为
MSRV。不要引入需要更新编译器的 API，除非同步更新 `rust-version`、README、CI 和
本文档。

默认 feature 是 `zlib`、`snappy` 和 `async`。feature-gated 代码必须同时在默认
构建和 `--no-default-features` 构建中通过。新增可选行为时，优先使用禁用后能完全
移除依赖的 feature。`async` feature 会刻意关闭 Tokio 默认 feature，只启用
`spawn_blocking` wrapper 所需的 runtime 部分。

这是库 crate。不要初始化全局 logger，也不要在库代码里使用 `println!` 或
`eprintln!`。运行期诊断必须通过 `log` facade 发出，并保持低噪声。避免记录 raw
value，也避免记录大量 raw key。

## API 与错误策略

公开 API 变更必须是有意的、有文档的、可测试的。发布验证会使用 missing-docs，
所以每个公开类型、variant 字段、常量和可能失败的方法都需要有用的 rustdoc。返回
`Result` 的公开 API 应说明重要错误条件。

优先使用结构化错误，而不是只返回字符串。新的失败模式应尽量纳入 `LevelDbError`，
并提供稳定的 `ErrorKind`；涉及文件 I/O 时保留 path context；有底层错误时尽量保留
source。调用方应能通过 `err.kind()` 和 `err.path()` 判断错误，而不需要解析
`Display` 文本。

read-only 模式必须严格。只读句柄不能创建缺失目录、repair 文件、flush、compact、
写 WAL record 或创建 native table。任何新增的写入路径都必须检查 read-only 行为，
并返回 `ErrorKind::ReadOnly`。

取消是协作式且类型化的。scan 取消应返回专用 cancelled 错误，不要退化成 generic
invalid-argument 或 I/O 错误。

## 测试

测试应覆盖行为、feature 边界和失败模式。codec/parser 逻辑优先放单元测试；
database open/read/write/recovery 行为放集成测试。

需要持续保留的重要场景：

- read-only open 不创建、不 repair、不 flush、不修改数据。
- 缺失和损坏文件的错误带 path context。
- WAL replay 能处理 fragmented record 和 tombstone。
- varint 解码拒绝 overflow 和 truncation。
- native flush/reopen 保留 key、value、sequence number 和 deletion。
- `OpenOptions::write_buffer_size = 0` 会让写入保留在 WAL overlay 中，直到
  显式 flush 或其他原生 recovery/compaction 路径消费这些写入。
- native table point read 与 prefix scan 尊重 manifest range 和 deletion record。
- 顺序 scan 与 table-parallel scan 看到的 entry 一致。
- 压缩 feature 禁用时返回类型化 unsupported/compression 错误。
- 库不会初始化 logger。

发布前或修改 feature-gated 代码后，运行以下 feature matrix：

```text
cargo test --all-features
cargo test --no-default-features
cargo test --no-default-features --features zlib
cargo test --no-default-features --features snappy
cargo test --no-default-features --features async
cargo test --no-default-features --features mmap
```

## Benchmark

Criterion benchmark 是合成测试，应隔离被测操作。不要把建库、临时目录清理或 logger
初始化混入 hot read 测量。

benchmark 分组应明确表达测量内容：

- Overlay hot path。
- Flushed native table point read。
- Native table point read 和 prefix scan。
- WAL recovery。
- 顺序 scan 与 table-parallel scan。

更新 README benchmark 数字时，应记录操作系统、日期、Rust 版本、Criterion sample
设置、绘图后端、是否安装 logger backend，以及 fixture 的 synthetic 限制。

## 发布检查清单

公开发布或类似 release 的提交前，运行：

```text
cargo fmt --check
cargo clippy --all-features --all-targets -- -D warnings
cargo rustdoc --all-features -- -D missing_docs
cargo test --all-features
cargo test --no-default-features
cargo test --no-default-features --features zlib
cargo test --no-default-features --features snappy
cargo test --no-default-features --features async
cargo test --no-default-features --features mmap
cargo doc --all-features --no-deps
cargo package --allow-dirty
cargo bench --all-features
```

当行为、API、兼容性或发布流程发生用户/维护者需要知道的变化时，更新
`CHANGELOG.md`。纯文字修正通常不需要 changelog entry，除非它改变了已文档化的
保证。
