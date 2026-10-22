# bedrock-brarchive

[English](README.md)

`bedrock-brarchive` 是自维护的 Minecraft Bedrock `BRArchive` v1 只读解析器。
打开时只加载入口表；payload 保持任意字节，本 crate 不解码 PNG/JSON/编译 MCB、不提取文件、
不创建 runtime、不修改游戏资源。不支持的版本会显式失败。

## 格式要点

- little-endian v1：8 字节 magic、32 位 entry count、32 位 version、256 字节 entry records；
- entry 包含 1 字节 UTF-8 名称长度、247 字节名称、相对 32 位 payload offset 与 32 位长度；
- offset 相对于 `16 + count * 256`；支持去重区间与非排序表，重名取最后一条。

## 边界

- 只读、零解包；payload 解码属于消费方。
- 无 unsafe、无线程池、无 GPU 依赖；可选 `fearless_simd` 加速名称校验。

## 构建与测试

```powershell
cargo check -p bedrock-brarchive
cargo test -p bedrock-brarchive
```

## 文档

- [crate 目录](../../docs/CRATES.md)
