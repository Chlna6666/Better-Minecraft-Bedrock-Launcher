# bmcbl_plugin_macros

[English](README.md)

BMCBL 插件过程宏：`#[bmcbl_plugin]` 为插件实现生成 ABI 入口，`#[plugin_metadata]` 在
构建期读取插件 manifest/TOML 并嵌入元数据。

## 边界

- 只包含宏实现；运行时 ABI 在 `bmcbl_plugin_api`。
- `#[bmcbl_plugin(pack)]` 已移除，请使用 `build.rs` +
  `bmcbl_plugin_api::pack::auto_pack_from_build_script()`。

## 构建与测试

```powershell
cargo check -p bmcbl_plugin_macros
```

## 文档

- [bmcbl_plugin_api](../bmcbl-plugin-api/README.zh-CN.md)
- [crate 目录](../../docs/CRATES.md)
