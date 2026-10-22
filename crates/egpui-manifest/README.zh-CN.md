# egpui-manifest

[English](README.md)

`egpui-manifest` 定义 egpui 桌面框架共享的应用 manifest schema：应用/语言标识、manifest
模型、校验问题与支持的 schema 版本（`SUPPORTED_SCHEMA_VERSION = 1`）。

## 边界

- 只做 schema 与校验；不读取资源、不打包应用、不执行下载。
- 应用专属 manifest 策略留在应用 crate。

## 构建与测试

```powershell
cargo check -p egpui-manifest
cargo test -p egpui-manifest
```

## 文档

- [架构](../../docs/ARCHITECTURE.md)
- [crate 目录](../../docs/CRATES.md)
