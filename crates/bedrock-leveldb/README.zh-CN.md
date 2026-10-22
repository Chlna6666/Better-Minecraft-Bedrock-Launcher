# bedrock-leveldb

[English](README.md)

`bedrock-leveldb` 是 Minecraft Bedrock 世界数据库的纯 Rust 原生 key/value 存储库。
性能目标以基准为准：尽可能零拷贝、短状态快照后的无锁读热路径，以及调用方显式请求时的
owned 分配。它只负责存储层，区块/实体/玩家/NBT 语义不在本 crate。

## 能力

- 读取原生 Bedrock/LevelDB manifest、WAL 记录与 table 文件；
- v0.2 写入标准 LevelDB WAL batch、flush 原生 `.ldb` table、持久化 manifest version edit；
- 旧 `BWLDB...` 文件仅保留迁移/兼容读取。

## 边界

- 不解释世界 key、NBT 或区块语义；这些属于 `bedrock-world`。
- 不做 UI/GPU/网络；不依赖应用层。

## 构建与测试

```powershell
cargo check -p bedrock-leveldb
cargo test -p bedrock-leveldb
```

## 文档

- [构建与测试](../../docs/BUILD_AND_TEST.md)
- [crate 目录](../../docs/CRATES.md)
