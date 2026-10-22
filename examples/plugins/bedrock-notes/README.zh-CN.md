# BMCBL Bedrock Notes

[English](README.md)

`BMCBL Bedrock Notes` 在 BMCBL 首页展示最近的 Minecraft Bedrock 补丁说明。

插件不直接获得网络访问权限：BMCBL 通过 host 管理的 HTTPS allowlist 缓存抓取 Mojang 补丁
说明端点，并把缓存文本快照交给插件。

## 数据源

`https://launchercontent.mojang.com/v2/bedrockPatchNotes.json`

## 构建

```powershell
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown --manifest-path examples/plugins/bedrock-notes/Cargo.toml
```
