# BMCBL Showcase（中文说明）

BMCBL 插件 API 的功能全集示例，覆盖页面、注入、事件、状态、宿主 IO 与生命周期的主要用法。

## 构建

```powershell
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown --manifest-path examples/plugins/showcase/Cargo.toml
```

构建脚本会自动生成 `plugin.toml` 与 `.bmcblx` 包，产物在
`target/bmcbl-plugin-auto-build/bmcbl-showcase/`，复制到 BMCBL 的 `plugins/` 目录即可加载。

## 代码组织

- `src/lib.rs`：插件入口与注册表；
- `src/state.rs`：session 与 storage 状态封装；
- `src/actions.rs`：动作表，每个动作对应一组宿主能力；
- `src/views.rs`：UI DSL 渲染（主页面、窗口、模态框、两处注入）；
- `config/`、`lang/`、`assets/`：配置 schema、本地化文案与插件资源。

## 关键约定

1. 渲染阶段只读会话状态，磁盘、网络与配置写入都放在 `init` / `handle_event` / `shutdown`；
2. 读取插件资源前必须在 `permissions.resource.allow` 中声明路径；
3. HTTP 与资源读取是异步缓存，首次调用可能返回“加载中”，宿主会在结果就绪后失效缓存；
4. 所有 `tr!` 键都应同时存在于 `lang/en-US.lang` 与 `lang/zh-CN.lang`。
