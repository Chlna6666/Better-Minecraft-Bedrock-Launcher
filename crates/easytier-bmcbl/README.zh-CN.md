# easytier-bmcbl

[English](README.md)

`easytier-bmcbl` 是 EasyTier 上游 crate 的 BMCBL 封装：重新导出核心模块，并持有应用侧
实例管理（runtime 构造、工作线程数限制 4–6、生命周期与配置桥接）。

## 特性

`aes-gcm`、`socks5`、`websocket` 透传到上游 `easytier-core`。

## 边界

- EasyTier 上游源码在 `crates/easytier`（不在 workspace），不要在此处修改上游代码。
- 封装层只负责线程/生命周期；BMCBL UI 与房间策略留在 `src/core`。

## 构建与测试

```powershell
cargo check -p easytier-bmcbl
cargo test -p easytier-bmcbl
```

## 文档

- [crate 目录](../../docs/CRATES.md)
