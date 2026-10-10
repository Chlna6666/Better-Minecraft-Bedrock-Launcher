# BMCBL Showcase

BMCBL 插件 API 的**功能全集示例**：一个插件覆盖页面、注入、事件、状态、宿主 IO 与生命周期的
主要用法。它同时是编写插件时的参考实现——可以照抄结构，也可以只挑需要的部分。

## 构建

需要 Rust 的 `wasm32-unknown-unknown` 目标：

```powershell
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown --manifest-path examples/plugins/showcase/Cargo.toml
```

`build.rs` 会调用 `bmcbl_plugin_api::pack::auto_pack_from_build_script()`：从
`[package.metadata.bmcbl-plugin]` 生成 `plugin.toml`、打包 `.bmcblx` 并解包出可直接加载的插件
目录，产物位于 `target/bmcbl-plugin-auto-build/bmcbl-showcase/`。把该目录（或 `.bmcblx` 文件）
放进 BMCBL 的 `plugins/` 目录即可。

在非 wasm 目标上 `cargo check` 只用于类型检查，打包会被跳过。

## 目录结构

| 路径 | 内容 |
| --- | --- |
| `src/lib.rs` | 插件入口：`init` / `handle_event` / `render_page` / `render_injection` / `shutdown` 与注册表 |
| `src/state.rs` | 会话值（session）与持久值（storage）的读写封装 |
| `src/actions.rs` | 动作表：每个 action id 演示一组宿主能力 |
| `src/views.rs` | UI DSL 渲染：主页面、窗口页面、模态框页面、两处注入 |
| `config/` | 默认配置与设置页表单 schema |
| `lang/` | 本地化文案（`en-US.lang` / `zh-CN.lang`） |
| `assets/` | 通过 `read_resource_*` 读取的插件资源 |

## 覆盖的能力

- **注册与导航**：`page`（带导航入口与不带导航的窗口/模态框页面）、`InjectionSlot::MainRootOverlay`、
  `InjectionSlot::HomeSidebar`、`subscribe "route-changed"`、插件自定义事件 `emit_event` + 自订阅。
- **UI DSL**：容器（行/列）、卡片、分组、标题、正文、徽标、按钮、带值按钮、复选框、开关、下拉框、
  进度条、链接、图标、图片（含 caption/placeholder/fallback）、间隔。
- **状态**：session 即时值 + storage 持久键值（`storage_get/set/delete/list`），渲染阶段只读会话值。
- **宿主能力**：`read_config` / `config_write`、`tr!` 本地化、`http_get_text` 异步缓存、
  `read_resource_text`、`read_clipboard_text` / `write_clipboard_text`、`open_external_url`、
  `navigate_path`、`create_task` / `update_task` / `finish_task`、`show_toast`、`theme_snapshot`、
  `invalidate!`。
- **权限与限制**：`capabilities`、`permissions.network/resource/external`、`limits.*` 都在
  `Cargo.toml` 中声明，可以对照了解每项能力需要什么声明。

## 推荐写法

1. **渲染只读状态**：`render_page` / `render_injection` 不要读文件、发请求或写配置，把结果放进
   session，渲染时直接投影；需要刷新时用 `invalidate!`。
2. **持久化放在事件里**：`storage_set`、`config_write` 只在 `init`、`handle_event`、`shutdown` 调用。
3. **动作表驱动**：用 `plugin_actions!` 定义 action id，再用一张 `(action, handler)` 表分发，
   避免在一个大 `match` 里堆所有逻辑。
4. **异步能力要接受“加载中”**：`http_get_text` 首次调用通常返回 `Loading`，
   `read_resource_text` 首次可能返回 `resource-loading`，宿主在结果就绪后会自动失效缓存。
5. **本地化键集中维护**：所有 `tr!` 键都会回退到键名本身，缺失时界面会直接显示键名。

## 与其它示例的关系

`examples/plugins/hello-wasm` 是最小骨架（页面 + 注入 + 事件）适合入门；本示例是完整参考实现。
需要演示原生 sidecar 时，可参考 `Cargo.toml` 中的 `sidecar_dir` 声明与 `sidecar_call` API。
